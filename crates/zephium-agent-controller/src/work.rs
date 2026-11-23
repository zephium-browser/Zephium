//! Product-owned execution and bounded content-free projection.

use std::collections::VecDeque;
use std::fmt;
#[cfg(feature = "probe-harness")]
use std::io::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use zephium_agent_provider_transport::{AgentProviderTransport, AgentProviderTransportConfig};
use zephium_agent_runtime::{
    AgentRuntimeBrowser, AgentRuntimeController, AgentRuntimeControllerFuture,
    AgentRuntimeControllerTerminalClass, AgentRuntimeControllerTerminalRefusal, AgentRuntimeEvent,
    AgentRuntimeStopReason, AgentRuntimeWorker,
};
use zephium_agentic::*;

use super::{
    AgentBrowserModel, AgentBrowserProviderError, AgentBrowserProviderTurn, AgentBrowserRetention,
    AgentBrowserSession, TerraControllerClock, TerraControllerIds, TerraControllerRunInput,
};

#[cfg(test)]
#[path = "work_tests.rs"]
mod tests;

#[path = "work_challenge.rs"]
mod challenge;
use challenge::{known_check, KnownCheck};
#[path = "work_compound_form.rs"]
mod compound_form;
#[path = "work_decision.rs"]
mod decision;
#[path = "work_inspection.rs"]
mod inspection;

#[path = "work_listing.rs"]
mod listing;
#[path = "work_navigation.rs"]
mod navigation;

#[path = "work_retained.rs"]
mod retained;
use retained::WorkBrowser;
pub use retained::{
    AgentWorkRetainedBrowser, AgentWorkRetainedController, AgentWorkRetainedHandle,
    AgentWorkRetainedOutcome, AgentWorkRetainedRecovery,
};

/// Trusted task-level decision from fresh independently collected state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentWorkTaskProgress {
    /// The approved task still requires work.
    Continue,
    /// Fresh trusted departure state permits the next frozen exact checkpoint.
    ReadyForNavigation,
    /// Fresh trusted state satisfies the action postcondition. Further actions
    /// are refused; extraction requires the registered schema and result predicate.
    ReadyForExtraction,
    /// The product's own task predicate is satisfied.
    Complete,
}

/// Trusted pre-model readiness of the initial document, never action progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentWorkInitialReadiness {
    /// The task can evaluate this fresh observation normally.
    Ready,
    /// The exact task context exists but has not finished becoming usable.
    /// The controller may obtain a bounded number of fresh read-only samples.
    Pending,
}

/// A document can finish navigating while its initial application shell still
/// consists of loading placeholders. This is scheduling evidence only: neither
/// its presence nor absence grants action authority or proves task readiness.
///
/// A cohort must dominate the usable controls before it delays startup. Named
/// meters, nonzero progress, and an isolated spinner are normal page content;
/// treating every progress role as a loading document would block dashboards.
fn has_dominant_loading_placeholders(observation: &SemanticObservation) -> bool {
    let mut placeholders = 0usize;
    let mut controls = 0usize;
    for node in observation
        .frames()
        .iter()
        .flat_map(SemanticSnapshot::nodes)
    {
        let anonymous = node.name().is_none_or(SemanticText::is_empty)
            && node.text().is_none_or(SemanticText::is_empty);
        if node.role() == SemanticRole::Progress
            && anonymous
            && matches!(node.value(), None | Some(SemanticValueSummary::Ordinal(0)))
        {
            placeholders += 1;
        } else if !anonymous
            && !node.states().contains(SemanticState::Disabled)
            && [
                SemanticOperationClass::Click,
                SemanticOperationClass::Fill,
                SemanticOperationClass::Select,
                SemanticOperationClass::Press,
            ]
            .into_iter()
            .any(|operation| node.operations().contains(operation))
        {
            controls += 1;
        }
    }
    (placeholders >= 3 && placeholders > controls) || says_loading(observation)
}

/// A page with next to nothing on it yet but its own "Loading…": an app
/// still booting, not a page to judge.
fn says_loading(observation: &SemanticObservation) -> bool {
    let nodes = || {
        observation
            .frames()
            .iter()
            .flat_map(SemanticSnapshot::nodes)
    };
    nodes().count() <= 16
        && nodes().any(|node| {
            [node.name(), node.text()]
                .into_iter()
                .flatten()
                .map(|words| {
                    words
                        .as_str()
                        .trim()
                        .trim_end_matches(['…', '.'])
                        .to_ascii_lowercase()
                })
                .any(|words| words == "loading" || words == "loading your workspace")
        })
}

/// Words a status line opens with while a page fetches what it shows.
const LOADING_WORDS: [&str; 9] = [
    "loading",
    "searching",
    "please wait",
    "fetching",
    "ładowanie",
    "wczytywanie",
    "wyszukiwanie",
    "wird geladen",
    "chargement",
];

/// A page that says, in a status or progress line, that it is still loading
/// what it shows (rome2rio's "Loading trip planner", a flight search's
/// results opened by their address): scheduling evidence only, like a
/// placeholder.
fn announces_loading(observation: &SemanticObservation) -> bool {
    observation
        .frames()
        .iter()
        .flat_map(SemanticSnapshot::nodes)
        .filter(|node| matches!(node.role(), SemanticRole::Status | SemanticRole::Progress))
        .flat_map(|node| [node.name(), node.text()].into_iter().flatten())
        .any(|words| {
            let words = words.as_str().trim().to_lowercase();
            words.len() <= 64 && LOADING_WORDS.iter().any(|lead| words.starts_with(lead))
        })
}

/// A page with next to nothing rendered yet: a client-side app still
/// booting, or a bot check's empty shell about to reload. Frames count as
/// content: a challenge widget is judged by its own frame.
fn unrendered(observation: &SemanticObservation) -> bool {
    let nodes = || {
        observation
            .frames()
            .iter()
            .flat_map(SemanticSnapshot::nodes)
    };
    nodes().count() <= 3
        && nodes().all(|node| node.role() != SemanticRole::FrameBoundary)
        && nodes()
            .map(|node| {
                node.text().map_or(0, |text| text.len()) + node.name().map_or(0, |name| name.len())
            })
            .sum::<usize>()
            < 64
}

/// The same context's main frame now holds another document.
fn replaced_document(prior: &SemanticObservation, next: &SemanticObservation) -> bool {
    prior.request().context() == next.request().context()
        && prior.frames().len() == next.frames().len()
        && prior
            .frames()
            .first()
            .zip(next.frames().first())
            .is_some_and(|(a, b)| a.frame() != b.frame())
}

fn finish_initial_readiness_wait(
    observation: SemanticObservation,
    readiness: AgentWorkInitialReadiness,
) -> Result<SemanticObservation, AgentWorkFailure> {
    match readiness {
        // Loading placeholders are page-controlled scheduling evidence. They
        // may buy the application bounded hydration time, but they must never
        // let a page veto a task that its trusted predicate considers ready.
        AgentWorkInitialReadiness::Ready => Ok(observation),
        AgentWorkInitialReadiness::Pending => Err(AgentWorkFailure::Observation(
            SemanticRuntimePortFailure::NotReady,
        )),
    }
}

fn validate_initial_readiness_successor(
    prior: &SemanticObservation,
    next: &SemanticObservation,
) -> Result<(), AgentWorkFailure> {
    if prior.request().context() != next.request().context()
        || prior.request().scope() != next.request().scope()
        || prior.request().generation() != next.request().generation()
        || prior.request().id() == next.request().id()
        || prior.frames().len() != next.frames().len()
        || prior.frames().iter().zip(next.frames()).any(|(a, b)| {
            a.frame() != b.frame()
                || a.invocation() == b.invocation()
                || a.generation() >= b.generation()
        })
    {
        return Err(AgentWorkFailure::Context);
    }
    Ok(())
}

/// Trusted product execution contract. Never implement this from model text,
/// page instructions, or a model-authored predicate. The UI does not receive
/// this port; it receives only the content-free handle below.
pub trait AgentWorkTask: Send {
    /// Read-only startup gate, called only before the first task evaluation or
    /// provider request. Pending grants no effect/ref authority or progress.
    /// Implementations must reject changed/ambiguous task identity rather than
    /// treating arbitrary missing content as hydration. Ready is the default;
    /// the controller's generic loading-placeholder gate still applies.
    fn initial_readiness(
        &self,
        _: &SemanticObservation,
    ) -> Result<AgentWorkInitialReadiness, AgentWorkFailure> {
        Ok(AgentWorkInitialReadiness::Ready)
    }
    /// One immutable same-origin exact destination. The first navigation task
    /// shape is read-only, initial-extraction only, with no redirects/repeats.
    /// The task must independently prove departure and arrival from fresh state.
    fn navigation_target(&self) -> Option<&ContextNavigationTarget> {
        None
    }
    /// Optional immutable finite route, exactly equal to the selected manifest
    /// node's route. Mutually exclusive with the legacy one-hop target. Every
    /// intermediate document must independently prove arrival and departure;
    /// only the final document may become ready for extraction.
    fn navigation_route(&self) -> Option<&AgentNavigationRoute> {
        None
    }
    /// Frozen public observed-link discovery authority, mutually exclusive with
    /// fixed routes. The model chooses a route and answer inside this scope.
    fn navigation_discovery(&self) -> Option<&AgentNavigationDiscovery> {
        None
    }
    /// Opts into nonterminal public reads of the exact initial baseline. Reads
    /// grant no task progress, new observation/ref or native effect authority.
    /// This setting is frozen at admission with the other task capabilities.
    fn allows_baseline_read(&self) -> bool {
        false
    }
    /// Frozen opt-in to exact-reference native inspection and fresh delivery.
    fn allows_progressive_observation(&self) -> bool {
        false
    }
    /// Explicitly permits one-shot viewport captures when semantic inspection
    /// cannot represent a visual/layout question. Captures remain bound to an
    /// acknowledged complete public observation and the provider disclosure
    /// policy; this opt-in alone grants neither pixels nor native authority.
    fn allows_viewport_screenshot(&self) -> bool {
        false
    }
    /// Enables bounded standalone waits over semantic change or one exact
    /// target-state predicate. The model cannot select timer-only success.
    fn allows_standalone_wait(&self) -> bool {
        false
    }
    /// Lets the model stop cleanly and request a person using one closed reason.
    /// This grants no resume capability; successor admission belongs to the host.
    fn allows_human_request(&self) -> bool {
        false
    }
    /// Enables a native Back step only when policy and the retained platform
    /// both prove an exact predecessor enrolled by this run.
    fn allows_history_back(&self) -> bool {
        false
    }
    /// Explicitly permits one terminal native subtree read anchored to the
    /// exact model-acknowledged observation. Frozen at admission; default is
    /// initial-scope only. This grants no action or navigation authority.
    fn allows_subtree_extraction(&self) -> bool {
        false
    }
    /// Explicitly enables verified snapshot actions before extraction. Default
    /// schema tasks remain read-only. This setting and the schema are frozen at
    /// admission; it grants no effect permission beyond the original policy.
    fn allows_actions_before_extraction(&self) -> bool {
        false
    }
    /// Enables bounded native field preparation from one model proposal. This
    /// grants no new effects; every primitive still uses the ordinary task policy.
    fn allows_compound_forms(&self) -> bool {
        false
    }
    /// Returns the task-approved operation subset for one node in the exact
    /// freshly evaluated observation. The controller intersects this with the
    /// node's semantic operations before constructing provider-visible action
    /// affordances. The default denies every action.
    fn model_action_operations(
        &self,
        _: &SemanticNode,
        _: &SemanticObservation,
    ) -> Result<SemanticOperations, AgentWorkFailure> {
        Ok(SemanticOperations::NONE)
    }
    /// Optional effect vocabulary restriction; it never replaces assessment.
    fn model_action_effect(&self) -> Option<SemanticEffectClass> {
        None
    }
    /// Advances trusted task state from one independently verified native
    /// action terminal. The default has no action-progress contract.
    fn accept_verified_action(
        &mut self,
        _: &SemanticActionBatchResult,
        _: &SemanticObservation,
    ) -> Result<(), AgentWorkFailure> {
        Ok(())
    }
    /// The assessed step never reached the page.
    fn action_not_run(&mut self) {}
    /// The assessed step reached the page and its outcome was not seen.
    fn action_outcome_unknown(&mut self) {}
    /// Whether terminal extraction is currently authorized by trusted task
    /// state. This is dynamic progress, not a frozen capability grant.
    fn terminal_extraction_ready(&self) -> bool {
        true
    }
    /// Optional single trusted extraction schema. Identity 1 is run-local;
    /// model/page content cannot register or replace it. Default is no extraction.
    fn extraction_schema(&self) -> Option<&SemanticExtractionSchema> {
        None
    }
    /// Whether `observation` is the look this task judged last; one it has
    /// not judged is judged before a read of it is accepted.
    fn judged(&self, _: &SemanticObservation) -> bool {
        true
    }
    /// Accepts a validated but explicitly model-mapped result against the
    /// trusted task contract. Default refuses; shape alone is not task authority.
    fn accept_extraction(
        &mut self,
        _: &SemanticExtractionResult<'_>,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure> {
        Err(crate::contract!())
    }
    /// Evaluates the approved task against fresh semantic state.
    fn evaluate(
        &mut self,
        observation: &SemanticObservation,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure>;
    /// Independently classifies the actual bounded native effect.
    fn assess(
        &self,
        action: &SemanticPreparedAction,
    ) -> Result<AgentEffectAssessment, AgentWorkFailure>;
    /// Assesses a proposal against the controller's exact current observation.
    /// Open objectives use this port to resolve model-selected targets without
    /// accepting caller-provided refs. Existing frozen task assessors retain
    /// their own independently bound baseline through the default implementation.
    fn assess_observed(
        &self,
        action: &SemanticPreparedAction,
        _: &SemanticObservation,
    ) -> Result<AgentEffectAssessment, AgentWorkFailure> {
        self.assess(action)
    }
    /// Supplies a code-owned recipe for a typed selection. None uses the page planner.
    /// The recipe still requires ordinary semantic binding and independent assessment.
    fn decision_action_recipe(
        &self,
        _: &DecisionOperation,
        _: &SemanticObservation,
    ) -> Result<Option<SemanticActionProposal>, AgentWorkFailure> {
        Ok(None)
    }
    /// The control that dismisses a cookie consent dialog on this observation,
    /// picked by the task's own closed name list; never a provider's choice.
    fn consent_dismissal(&self, _: &SemanticObservation) -> Option<SemanticReferenceId> {
        None
    }

    /// The look shows a consent banner or page whose controls it may have
    /// cut off: a whole look decides.
    fn consent_suspected(&self, _: &SemanticObservation) -> bool {
        false
    }

    /// The close control of a dialog the site laid over the page unasked (a
    /// sign-in, newsletter or app offer), picked by the task's own closed
    /// name list; Rust closes it on a look no model has acted on yet.
    fn overlay_dismissal(&self, _: &SemanticObservation) -> Option<SemanticReferenceId> {
        None
    }

    /// Rust presses a cookie banner's refusal (true), and the press came
    /// back (false). A page that saved the choice by loading another document
    /// ends between the two; the task may run the page again once, with the
    /// choice in place.
    fn consent_pressing(&self, _: bool) {}

    /// A collapsed disclosure the task's own closed name list says may hold
    /// the read's values; toggled once by Rust when a value is unlocated.
    fn detail_disclosure(&self, _: &SemanticObservation) -> Option<DecisionOperation> {
        None
    }
    /// A trusted page-state reading that the person must act first (a sign-in
    /// wall). Checked on the first observation, each snapshot and each verified action;
    /// the page agent can raise a wall itself but never clear one.
    fn human_wall(&self, _: &SemanticObservation) -> Option<AgentBrowserHumanReason> {
        None
    }
    /// The page shows no sign-in form of its own: a page agent's request
    /// that the person sign in is refused, and it carries on as a guest.
    fn sign_in_optional(&self, _: &SemanticObservation) -> bool {
        false
    }
    /// A script bot check is given longer to clear by itself before the
    /// person is asked: the page is the person's own request, not one
    /// source among several.
    fn patient_checks(&self) -> bool {
        false
    }
    /// A plain read may map its already captured public text directly, without
    /// paying a page planner to choose Extract. Interaction tasks stay on their loop.
    fn prefers_direct_extraction(&self) -> bool {
        false
    }
    /// The first view is judged whole before any model call.
    fn whole_first_look(&self) -> bool {
        false
    }
    /// A daily app's view (Slack, Gmail, Calendar, Linear, Notion, GitHub)
    /// is read as its records at once, before any model call.
    fn reads_app_view(&self) -> bool {
        false
    }
    /// The control that opens the next view a daily app's read goes
    /// through, pressed by Rust as a read; None when no view is left.
    fn app_view(&self, _: &SemanticObservation) -> Option<SemanticReferenceId> {
        None
    }
    /// A search engine's results page is read by the task from its whole
    /// first look and ends there: no model ever sees it.
    fn reads_results_page(&self) -> bool {
        false
    }
    /// The nodes each result the task read is cited to, or the one node
    /// that stood in the way of results.
    fn results_page(&self, _: &SemanticObservation) -> Vec<Vec<SemanticReferenceId>> {
        Vec::new()
    }
    /// Whether a results page has drawn its results or a wall yet; an engine
    /// that draws them by script shows an empty shell at first.
    fn results_drawn(&self, _: &SemanticObservation) -> bool {
        true
    }
    /// The field this fill or choice types into commits nothing by itself
    /// (a search box, a place or date picker, a filter): the page may rewrite
    /// what was typed, and an unverified result is shown to the model as the
    /// field now is rather than held for the person.
    fn rewritable_field(&self, _: &SemanticPreparedAction, _: &SemanticObservation) -> bool {
        false
    }
    /// Supplies independently sourced current account facts for this exact
    /// context. Called at startup and before each provider/effect admission,
    /// including nonterminal inspection and extraction mapping. It must be
    /// bounded and nonblocking. Do not renew a cached sample's timestamp: return
    /// its original identity/time or obtain a new sample from the trusted
    /// adapter. A changed account/context refuses the run, not a scope switch.
    fn attest_account(
        &self,
        context: ContextJoin,
        now: AgentPolicyInstant,
    ) -> Result<AgentContextAccountBinding, AgentWorkFailure>;
}

/// Explicit Work-owned page and authoritatively selected profile storage.
/// This does not accept ordinary Browse tabs or load user extensions.
pub struct AgentWorkContextSpec {
    identity: ContextIdentity,
    storage: ContextProfileStorageClass,
    target: ContextNavigationTarget,
    origin: SemanticOrigin,
    document_policy: WorkBrowserDocumentPolicy,
}

impl AgentWorkContextSpec {
    /// Validates one owned page and exact initial target; manifest scope is
    /// independently checked when constructing the run.
    pub fn try_new(
        identity: ContextIdentity,
        storage: ContextProfileStorageClass,
        target: ContextNavigationTarget,
    ) -> Result<Self, AgentWorkFailure> {
        Self::try_new_with_document_policy(
            identity,
            storage,
            target,
            WorkBrowserDocumentPolicy::Exact,
        )
    }

    /// Validates one trusted initial-document policy together with the exact
    /// owned target. This policy is application-authored before native work;
    /// it grants no model or successor-navigation authority.
    pub fn try_new_with_document_policy(
        identity: ContextIdentity,
        storage: ContextProfileStorageClass,
        target: ContextNavigationTarget,
        document_policy: WorkBrowserDocumentPolicy,
    ) -> Result<Self, AgentWorkFailure> {
        if identity.kind() != ContextKind::Owned {
            return Err(crate::contract!());
        }
        let origin =
            SemanticOrigin::parse(target.as_url().as_ref()).map_err(|_| crate::contract!())?;
        if !document_policy.admits_request(&target) {
            return Err(crate::contract!());
        }
        Ok(Self {
            identity,
            storage,
            target,
            origin,
            document_policy,
        })
    }
}

/// Bounded model/run configuration supplied by the trusted application.
pub struct AgentWorkRunSettings {
    model: AgentBrowserModel,
    ids: TerraControllerIds,
    clock: Arc<dyn TerraControllerClock>,
    deadline: Instant,
    max_model_calls: u8,
    max_actions: u64,
}

impl AgentWorkRunSettings {
    /// Selects the same catalog model, identifiers, clock and absolute deadline
    /// used by the shared provider/session architecture.
    pub fn new(
        model: AgentBrowserModel,
        ids: TerraControllerIds,
        clock: Arc<dyn TerraControllerClock>,
        deadline: Instant,
    ) -> Self {
        Self {
            model,
            ids,
            clock,
            deadline,
            max_model_calls: super::MAX_BROWSER_MODEL_TURNS,
            max_actions: super::MAX_BROWSER_ACTIONS,
        }
    }

    /// Sets this run's total provider-call allowance, including terminal
    /// mapping (2–64; default 8). This trusted application setting grants no
    /// manifest operation, token, cost, scope or deadline authority. Stateless
    /// context retention and provider input bounds remain independently enforced.
    pub fn with_max_model_calls(mut self, max_calls: u8) -> Result<Self, AgentWorkFailure> {
        if !(2..=super::MAX_WORK_MODEL_CALLS).contains(&max_calls) {
            return Err(crate::contract!());
        }
        self.max_model_calls = max_calls;
        Ok(self)
    }

    /// Sets the native action-attempt allowance (0–64; default 8). Zero
    /// disables action admission. Each attempt still needs the approved
    /// manifest's effect authority and remaining operation budget.
    pub fn with_max_actions(mut self, max_actions: u64) -> Result<Self, AgentWorkFailure> {
        if max_actions > super::MAX_WORK_ACTIONS {
            return Err(crate::contract!());
        }
        self.max_actions = max_actions;
        Ok(self)
    }
}

/// Approved product input, admitted before native or provider work begins.
#[must_use]
pub struct AgentWorkRunInput {
    model_image_host: Option<Arc<dyn zephium_agentic::SemanticModelImageHost>>,
    decision_provider: Option<super::AgentBrowserDecisionProvider>,
    decision_model: Option<zephium_agentic::AgentProviderModelClient>,
    manifest: AgentRunManifest,
    lease: AgentPlanLeaseBinding,
    context: AgentWorkContextSpec,
    objective: Option<AgentProviderObjective>,
    settings: AgentWorkRunSettings,
    durable_result: bool,
    original_anonymous_read: Option<zephium_agentic::WorkBrowserAnonymousReadAuthority>,
    isolated_store: bool,
    work_store: bool,
}

impl AgentWorkRunInput {
    /// Joins the host coordinator before admission. Native source ownership is
    /// obtained separately from the original retained adapter, never supplied here.
    pub fn bind_model_image_host(
        &mut self,
        host: Arc<dyn zephium_agentic::SemanticModelImageHost>,
    ) -> Result<(), AgentWorkFailure> {
        if self.model_image_host.is_some() {
            return Err(crate::contract!());
        }
        self.model_image_host = Some(host);
        Ok(())
    }
    /// Enables typed decisions with a trusted provider configuration. Existing
    /// OpenAI ownership supplies the per-question fallback; no scope is added.
    pub fn with_decision_provider(mut self, provider: super::AgentBrowserDecisionProvider) -> Self {
        self.decision_provider = Some(provider);
        self
    }

    /// Replaces only the trusted dormant provider selection before admission.
    pub fn set_decision_provider(&mut self, provider: super::AgentBrowserDecisionProvider) {
        self.decision_provider = Some(provider);
    }

    /// Answers typed questions no primary backend settled with the person's
    /// chosen model instead of the OpenAI emulation. No scope is added.
    pub fn set_decision_model(&mut self, model: zephium_agentic::AgentProviderModelClient) {
        self.decision_model = Some(model);
    }
    /// Requires isolated native storage without changing account or disclosure scope.
    pub fn with_isolated_website_data(mut self) -> Self {
        self.isolated_store = true;
        self
    }
    /// Opens the page in Work's own persistent website data, never the
    /// profile's. A private profile has none: its pages keep isolated storage.
    pub fn with_work_website_data(mut self) -> Self {
        self.isolated_store = true;
        self.work_store = self.context.storage == ContextProfileStorageClass::Durable;
        self
    }
    /// Whether the page opens in Work's own persistent website data.
    pub const fn uses_work_website_data(&self) -> bool {
        self.work_store
    }
    /// Marks trusted original read-only intent before resource construction.
    /// Exact anonymous manifest/node/profile/run scope is independently checked;
    /// this flag is neither cookie absence nor screenshot permission.
    pub fn mark_original_anonymous_read(&mut self) {
        if self.original_anonymous_read.is_none() && self.isolated_store {
            self.original_anonymous_read = self.settings.clock.now().ok().and_then(|now| {
                zephium_agentic::WorkBrowserAnonymousReadAuthority::from_original_read(
                    &self.manifest,
                    self.lease,
                    self.context.identity,
                    &self.context.target,
                    now,
                    self.settings.deadline,
                )
            });
        }
    }
    /// Descriptive original resource construction operands for the trusted
    /// application. These do not mint a browser, account or execution lease.
    pub fn retained_resource_spec(
        &self,
    ) -> Result<AgentWorkRetainedResourceSpec, AgentWorkFailure> {
        Ok(AgentWorkRetainedResourceSpec {
            identity: self.context.identity,
            storage: self.context.storage,
            isolated_public: self.isolated_store
                || self
                    .manifest
                    .plan_node(self.lease.node())
                    .is_some_and(|node| {
                        node.navigation_discovery()
                            .is_some_and(|scope| scope.is_public_web())
                    }),
            work_website_data: self.work_store,
            anonymous_read_authority: self.original_anonymous_read.clone(),
            target: self.context.target.clone(),
            document_policy: self.context.document_policy,
            clock: self.settings.clock.clone(),
            deadline: self.settings.deadline,
            expires_at: self
                .manifest
                .plan_node(self.lease.node())
                .ok_or_else(|| crate::contract!())?
                .expires_at(),
        })
    }
    /// Joins approved scope, explicit profile, bounded objective and model.
    pub fn try_new(
        manifest: AgentRunManifest,
        lease: AgentPlanLeaseBinding,
        context: AgentWorkContextSpec,
        objective: String,
        settings: AgentWorkRunSettings,
    ) -> Result<Self, AgentWorkFailure> {
        Self::try_new_with_monotonic_now(
            manifest,
            lease,
            context,
            objective,
            settings,
            Instant::now,
        )
    }

    fn try_new_with_monotonic_now(
        manifest: AgentRunManifest,
        lease: AgentPlanLeaseBinding,
        context: AgentWorkContextSpec,
        objective: String,
        settings: AgentWorkRunSettings,
        monotonic_now: impl FnOnce() -> Instant,
    ) -> Result<Self, AgentWorkFailure> {
        let root = manifest
            .plan_node(lease.node())
            .ok_or_else(|| crate::contract!())?;
        let now = settings.clock.now().map_err(|_| crate::contract!())?;
        // Policy time floors elapsed milliseconds. Sample it first: comparing
        // an earlier, larger wall horizon with a later, smaller policy remainder
        // can falsely reject the same absolute deadline at a millisecond edge.
        // Neither approved expiry nor the caller's absolute deadline is changed;
        // later execution still independently enforces both original bounds.
        let horizon = settings
            .deadline
            .checked_duration_since(monotonic_now())
            .ok_or(AgentWorkFailure::Deadline)?;
        if horizon.is_zero() || horizon > super::MAX_TERRA_CONTROLLER_HARD_DEADLINE {
            return Err(AgentWorkFailure::Deadline);
        }
        let remaining = root
            .expires_at()
            .millis()
            .checked_sub(now.millis())
            .ok_or(AgentWorkFailure::Deadline)?;
        if now < manifest.issued_at() || horizon > Duration::from_millis(remaining) {
            return Err(AgentWorkFailure::Deadline);
        }
        if manifest.plan_nodes().len() != 1
            || manifest.run() != context.identity.owner()
            || !root.profiles().contains(&context.identity.profile())
            || !root.origins().contains(&context.origin)
        {
            return Err(crate::contract!());
        }
        let config = match settings.model {
            AgentBrowserModel::Terra => super::try_terra_provider_exact_call_config(
                super::TERRA_CONTROLLER_MAX_OUTPUT_TOKENS,
            )
            .map_err(|_| crate::contract!())?,
            AgentBrowserModel::Luna => super::try_luna_provider_exact_call_config(
                super::TERRA_CONTROLLER_MAX_OUTPUT_TOKENS,
            )
            .map_err(|_| crate::contract!())?,
            AgentBrowserModel::Gpt6Luna => super::try_gpt6_luna_provider_exact_call_config(
                super::TERRA_CONTROLLER_MAX_OUTPUT_TOKENS,
            )
            .map_err(|_| crate::contract!())?,
        };
        let objective =
            AgentProviderObjective::try_admit_conservative_utf8(objective, config.tokenizer())
                .map_err(|_| crate::contract!())?;
        Ok(Self {
            model_image_host: None,
            decision_provider: None,
            decision_model: None,
            manifest,
            lease,
            context,
            objective: Some(objective),
            settings,
            durable_result: false,
            original_anonymous_read: None,
            isolated_store: false,
            work_store: false,
        })
    }
    /// Explicitly opts a trusted extraction task into profile-owned result
    /// persistence. Private/ephemeral contexts cannot silently write artifacts.
    pub fn persist_extraction_result(mut self) -> Result<Self, AgentWorkFailure> {
        if self.context.storage != ContextProfileStorageClass::Durable {
            return Err(crate::contract!());
        }
        self.durable_result = true;
        Ok(self)
    }
    fn admission(
        &self,
        owner: AgentWorkIncarnation,
    ) -> Result<AgentWorkJournalMutation, AgentWorkFailure> {
        if self.durable_result {
            AgentWorkJournalMutation::admit_with_result(
                &self.manifest,
                owner,
                self.context.identity.profile(),
            )
            .map_err(|_| crate::contract!())
        } else {
            Ok(AgentWorkJournalMutation::admit(&self.manifest, owner))
        }
    }
}

/// Original approved input facts, not resource or execution authority.
pub struct AgentWorkRetainedResourceSpec {
    /// Exact context/run/profile identity in the approved input.
    pub identity: ContextIdentity,
    /// Original selected session persistence class.
    pub storage: ContextProfileStorageClass,
    /// Frozen public discovery must never inherit profile cookies.
    pub isolated_public: bool,
    /// Work's own persistent store for the profile, set only with isolation.
    pub work_website_data: bool,
    /// Original anonymous task proof, never signed-out storage/pixel authority.
    pub anonymous_read_authority: Option<zephium_agentic::WorkBrowserAnonymousReadAuthority>,
    /// Exact approved initial document, not navigation authority.
    pub target: ContextNavigationTarget,
    /// Trusted initial-document construction policy, frozen before native work.
    pub document_policy: WorkBrowserDocumentPolicy,
    /// Original policy clock, shared with controller admission.
    pub clock: Arc<dyn TerraControllerClock>,
    /// Original absolute execution deadline.
    pub deadline: Instant,
    /// Original plan-node policy expiry ceiling for resource acquisition.
    pub expires_at: AgentPolicyInstant,
}

/// Content-free shell/application observation port. It exposes no browser,
/// native handles, credentials, page data or mutable policy authority.
pub struct AgentWorkHandle {
    events: Arc<Mutex<WorkEvents>>,
    terminal: Arc<Mutex<Option<AgentWorkOutcome>>>,
}

impl AgentWorkHandle {
    /// Registers one content-free, nonblocking application wake. The consumer
    /// must enqueue work without re-entering this handle from its waker.
    pub fn set_waker(&self, waker: std::task::Waker) {
        let mut events = lock(&self.events);
        events.waker = Some(waker);
        if !events.queue.is_empty() {
            events.wake();
        }
    }
    /// Removes the oldest bounded event, preserving its stable sequence.
    pub fn take_event(&self) -> Option<AgentWorkEvent> {
        lock(&self.events).queue.pop_front()
    }
    /// Read-only drain status. This is not terminal proof; the application
    /// also retains the original outcome and joined runtime lifecycle.
    pub fn has_pending_events(&self) -> bool {
        !lock(&self.events).queue.is_empty()
    }
    /// Moves the sole terminal/recovery owner out exactly once.
    pub fn take_outcome(&mut self) -> Option<AgentWorkOutcome> {
        lock(&self.terminal).take()
    }
}

/// Product outcome; recovery explicitly retains every unresolved core owner.
#[must_use]
pub enum AgentWorkOutcome {
    /// Trusted task completion with durable policy/accounting closure. Native
    /// and provider shutdown proofs are consumed by the runtime lifecycle.
    Succeeded(AgentWorkSuccess),
    /// The current run closed cleanly after the model requested a person. All
    /// execution authority is revoked; only a trusted host may admit a fresh
    /// successor run.
    WaitingForHuman(AgentWorkWaitingForHuman),
    /// Task failed or was cancelled, but all original execution owners drained.
    /// This never carries an extraction result or authorizes another run.
    ClosedUnsuccessfully(AgentWorkClosedUnsuccessfully),
    /// Execution stopped without enough evidence for clean closure.
    Recovery(AgentWorkRecovery),
}

/// Clean execution ownership plus an optional bounded model-mapped result.
/// Result data is not a native-effect, factual-verification or durability proof.
pub struct AgentWorkSuccess {
    settlement: AgentRunPolicySettlement,
    extraction: Option<Box<SemanticOwnedExtractionResult>>,
}

/// Unsuccessful business outcome with original policy/audit closure. Native
/// and provider proofs remain owned by the exact runtime lifecycle join.
pub struct AgentWorkClosedUnsuccessfully {
    settlement: AgentRunPolicySettlement,
    failure: AgentWorkFailure,
    human_review: Option<AgentNeedsHumanTransition>,
}

/// Durable, non-authorizing model request for human intervention.
///
/// It is bound to the exact delivered observation at which the run stopped.
/// It carries no provider continuation, old ref authority, or resume token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentWorkHumanRequest {
    context: ContextJoin,
    observation: SemanticObservationId,
    generation: SemanticObservationGeneration,
    reason: AgentBrowserHumanReason,
    retained_resource: Option<WorkBrowserResourceIdentity>,
}

impl AgentWorkHumanRequest {
    /// Exact document/cancellation authority at the handoff request.
    pub const fn context(self) -> ContextJoin {
        self.context
    }

    /// Exact model-delivered observation at the handoff request.
    pub const fn observation(self) -> SemanticObservationId {
        self.observation
    }

    /// Exact progressive-observation generation at the handoff request.
    pub const fn generation(self) -> SemanticObservationGeneration {
        self.generation
    }

    /// Closed model-selected reason for requesting a person.
    pub const fn reason(self) -> AgentBrowserHumanReason {
        self.reason
    }

    /// Exact retained resource whose run lease was revoked at handoff. Legacy
    /// owned contexts return `None` because clean closure destroys that page.
    pub const fn retained_resource(self) -> Option<WorkBrowserResourceIdentity> {
        self.retained_resource
    }
}

/// Clean terminal handoff. This contains durable accounting and descriptive
/// correlation only: no provider continuation, native lease, refs, or model
/// authority survives the stopped run.
pub struct AgentWorkWaitingForHuman {
    settlement: AgentRunPolicySettlement,
    request: AgentWorkHumanRequest,
}

impl AgentWorkWaitingForHuman {
    /// Closed model request bound to the last delivered observation.
    pub const fn request(&self) -> AgentWorkHumanRequest {
        self.request
    }

    /// Original clean policy/accounting closure for the stopped actor.
    pub const fn policy_settlement(&self) -> AgentRunPolicySettlement {
        self.settlement
    }

    /// Content-free metric closure for product persistence and diagnostics.
    pub const fn closure(&self) -> AgentRunMetricClosure {
        self.settlement.closure()
    }
}

impl AgentWorkClosedUnsuccessfully {
    /// Original typed cause; clean resource drain does not mean task success.
    pub const fn failure(&self) -> AgentWorkFailure {
        self.failure
    }
    /// Exact failed/cancelled policy closure, never a replacement proof.
    pub const fn policy_settlement(&self) -> AgentRunPolicySettlement {
        self.settlement
    }
    /// Exact never-dispatched refusal after original resource/policy closure.
    /// This classifies review; it grants no action or continuation authority.
    pub const fn human_review(&self) -> Option<AgentNeedsHumanTransition> {
        self.human_review
    }
}
impl AgentWorkSuccess {
    /// Borrowed private result for exact terminal/artifact publication only.
    pub fn extraction(&self) -> Option<&SemanticOwnedExtractionResult> {
        self.extraction.as_deref()
    }
    /// Original content-free closure metrics, not result factual verification.
    pub const fn closure(&self) -> AgentRunMetricClosure {
        self.settlement.closure()
    }
    /// Original successful policy/audit closure; never a replacement proof.
    pub const fn policy_settlement(&self) -> AgentRunPolicySettlement {
        self.settlement
    }
    /// Moves the result once. Application adapters must additionally gate
    /// publication on their original clean lifecycle and durable terminal ACK.
    pub fn take_extraction(&mut self) -> Option<SemanticOwnedExtractionResult> {
        self.extraction.take().map(|result| *result)
    }
}

/// Trusted public-data extraction task, initially limited to the initial read.
/// The product may opt into one acknowledged-ref subtree capture. The schema
/// is frozen before admission; no native action is allowed.
pub struct AgentWorkExtractionTask {
    direct_extraction: bool,
    baseline_read: bool,
    progressive_observation: bool,
    schema: SemanticExtractionSchema,
    account: AgentAccountScope,
    account_sample: std::cell::Cell<Option<AgentContextAccountBinding>>,
    subtree: bool,
}
impl AgentWorkExtractionTask {
    /// Prefer direct mapping for a public fact read. The controller still
    /// requires a whole-page findings schema and checks walls and sensitivity.
    pub fn with_direct_extraction(mut self) -> Self {
        self.direct_extraction = true;
        self
    }

    /// Registers one run-local schema and explicitly trusted account scope.
    pub fn try_new(
        fields: Vec<SemanticExtractionFieldSchema>,
        account: AgentAccountScope,
    ) -> Result<Self, AgentWorkFailure> {
        let id = SemanticExtractionSchemaId::new(1).ok_or_else(|| crate::contract!())?;
        let schema =
            SemanticExtractionSchema::try_new(id, fields).map_err(|_| crate::contract!())?;
        Ok(Self {
            schema,
            account,
            account_sample: std::cell::Cell::new(None),
            subtree: false,
            direct_extraction: false,
            baseline_read: false,
            progressive_observation: false,
        })
    }

    /// Allows initial scope or one exact acknowledged-ref native subtree read.
    /// Field, sensitivity, read and provider ceilings remain unchanged.
    pub fn with_subtree_extraction(mut self) -> Self {
        self.subtree = true;
        self
    }

    /// Narrows extraction evidence to trusted semantic roles after the existing
    /// native capture. Frozen with the schema; all privacy and byte ceilings
    /// remain unchanged. Baseline inspection and native scope are unaffected.
    pub fn with_source_roles(mut self, roles: SemanticReadRoleSelection) -> Self {
        self.schema = self.schema.with_source_roles(roles);
        self
    }

    /// Allows nonterminal public inspection of the existing initial baseline
    /// before selecting the terminal extraction scope. No fresh capture or
    /// mapping result is implied by a read.
    pub fn with_baseline_read(mut self) -> Self {
        self.baseline_read = true;
        self
    }

    /// Allows bounded, reference-anchored expansion of the acknowledged
    /// current document before terminal mapping. This grants neither
    /// navigation nor native action authority and requires baseline reads so
    /// the model can inspect the evidence from which it selects a scope.
    pub fn with_progressive_observation(mut self) -> Self {
        self.progressive_observation = true;
        self
    }
}
impl AgentWorkTask for AgentWorkExtractionTask {
    fn prefers_direct_extraction(&self) -> bool {
        self.direct_extraction
    }

    fn allows_baseline_read(&self) -> bool {
        self.baseline_read
    }
    fn allows_progressive_observation(&self) -> bool {
        self.progressive_observation
    }
    fn allows_subtree_extraction(&self) -> bool {
        self.subtree
    }
    fn extraction_schema(&self) -> Option<&SemanticExtractionSchema> {
        Some(&self.schema)
    }
    fn evaluate(
        &mut self,
        _: &SemanticObservation,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure> {
        Ok(AgentWorkTaskProgress::Continue)
    }
    fn assess(
        &self,
        _: &SemanticPreparedAction,
    ) -> Result<AgentEffectAssessment, AgentWorkFailure> {
        Err(crate::contract!())
    }
    fn attest_account(
        &self,
        context: ContextJoin,
        now: AgentPolicyInstant,
    ) -> Result<AgentContextAccountBinding, AgentWorkFailure> {
        if let Some(sample) = self.account_sample.get() {
            return if sample.context() == context {
                Ok(sample)
            } else {
                Err(crate::contract!())
            };
        }
        // Static product scope must not become synthetic freshness authority.
        let sample = AgentContextAccountBinding::new(
            AgentAccountAttestationId::generate(),
            context,
            self.account,
            now,
        );
        self.account_sample.set(Some(sample));
        Ok(sample)
    }
    fn accept_extraction(
        &mut self,
        result: &SemanticExtractionResult<'_>,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure> {
        if result.schema() != self.schema.id() || result.stats().sensitive_source_edges() != 0 {
            return Err(crate::contract!());
        }
        // Core validated schema, types, bounds, source delivery and secret
        // rejection. Completion means a model-mapped result, not proven truth.
        Ok(AgentWorkTaskProgress::Complete)
    }
}

/// Opaque, content-redacted recovery ownership; never permission to replay.
#[must_use]
pub struct AgentWorkRecovery {
    failure: AgentWorkFailure,
    state: Box<WorkState>,
}

impl AgentWorkRecovery {
    /// Content-free admission identity retained when execution never started.
    /// Recovery of a consumed input cannot create a replacement admission.
    pub fn journal_admission(
        &self,
        owner: AgentWorkIncarnation,
    ) -> Result<AgentWorkJournalMutation, AgentWorkFailure> {
        let input = self
            .state
            .input
            .as_ref()
            .ok_or_else(|| crate::contract!())?;
        input.admission(owner)
    }
    /// Seals the original retained ledger and prepares its exact pending batch,
    /// or the next never-dispatched batch. This grants no browser/provider work.
    pub fn prepare_audit_reconciliation(
        &mut self,
    ) -> Result<Option<AgentAuditDelivery>, AgentWorkFailure> {
        let audit = &mut self.state.journal_mut()?.audit;
        audit
            .seal_for_shutdown()
            .map_err(|_| AgentWorkFailure::Audit)?;
        if let Some(delivery) = audit
            .current_delivery()
            .map_err(|_| AgentWorkFailure::Audit)?
        {
            return Ok(Some(delivery));
        }
        if audit.is_quiescent() {
            return Ok(None);
        }
        audit
            .begin_next_delivery(MAX_AGENT_AUDIT_DELIVERY_EVENTS)
            .map(Some)
            .map_err(|_| AgentWorkFailure::Audit)
    }

    /// Applies only the exact original ledger's delivery receipt. Audit drain
    /// never clears native, provider, policy or runtime recovery obligations.
    pub fn settle_audit_reconciliation(
        &mut self,
        settlement: AgentAuditDeliverySettlement,
    ) -> Result<AgentAuditDeliveryOutcome, AgentWorkFailure> {
        self.state
            .journal_mut()?
            .audit
            .settle_delivery(settlement)
            .map_err(|_| AgentWorkFailure::Audit)
    }

    /// Content-free status of the original retained audit ledger.
    pub fn audit_reconciliation_status(
        &mut self,
    ) -> Result<AgentAuditLedgerStatus, AgentWorkFailure> {
        Ok(self.state.journal_mut()?.audit.status())
    }
    /// Exact closed stop reason.
    pub const fn failure(&self) -> AgentWorkFailure {
        self.failure
    }
    /// Count of retained unsolicited/terminal callbacks requiring reconciliation.
    pub fn retained_callbacks(&self) -> usize {
        self.state.native.deferred.len()
    }
}

impl fmt::Debug for AgentWorkOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded(_) => formatter.write_str("AgentWorkOutcome::Succeeded"),
            Self::WaitingForHuman(value) => formatter
                .debug_tuple("AgentWorkOutcome::WaitingForHuman")
                .field(&value.request.reason)
                .finish(),
            Self::ClosedUnsuccessfully(value) => formatter
                .debug_tuple("AgentWorkOutcome::ClosedUnsuccessfully")
                .field(&value.failure)
                .finish(),
            Self::Recovery(value) => formatter
                .debug_tuple("AgentWorkOutcome::Recovery")
                .field(&value.failure)
                .finish(),
        }
    }
}

/// Shipping Work execution actor on the existing runtime worker and mailbox.
/// Construction is idle: provider/native requests begin only after RunStarted.
#[must_use]
pub struct AgentWorkController {
    state: Option<WorkState>,
    terminal: Arc<Mutex<Option<AgentWorkOutcome>>>,
    retained_terminal: Option<Arc<Mutex<Option<AgentWorkRetainedOutcome>>>>,
}

/// Landmarks an app view read opens whole before it gives up.
const MAX_APP_LANDMARKS: usize = 4;
/// Views one app read goes through at most (Activity, DMs, Home).
const MAX_APP_VIEWS: usize = 3;
/// A view's list settles once its pane stops changing this long.
const APP_VIEW_QUIET_MILLIS: u32 = 300;
/// Virtual lists render their rows a beat after the pane itself.
const APP_VIEW_RENDER_MILLIS: u64 = 400;
/// Below this many nodes a look is of a page still drawing, and the pause
/// before the look a refused request for the person gets on it.
const UNFINISHED_PAGE_NODES: usize = 24;
const UNFINISHED_PAGE_WAIT_MILLIS: u64 = 1_500;
/// Looks a results page gets to draw what its engine fills in by script,
/// and the pause before each.
const RESULTS_DRAW_LOOKS: usize = 5;
const RESULTS_DRAW_LOOK_MILLIS: u64 = 500;

impl AgentWorkController {
    /// Immutable session storage selected in the trusted input, while dormant.
    pub fn profile_storage_binding(
        &self,
    ) -> Result<(AgentWorkProfileId, ContextProfileStorageClass), AgentWorkFailure> {
        let context = &self
            .state
            .as_ref()
            .and_then(|state| state.input.as_ref())
            .ok_or_else(|| crate::contract!())?
            .context;
        Ok((context.identity.profile(), context.storage))
    }
    /// Trusted durable destination, fixed before native/provider admission.
    pub fn durable_result_profile(&self) -> Result<Option<AgentWorkProfileId>, AgentWorkFailure> {
        let input = self
            .state
            .as_ref()
            .and_then(|state| state.input.as_ref())
            .ok_or_else(|| crate::contract!())?;
        Ok(input
            .durable_result
            .then_some(input.context.identity.profile()))
    }
    /// Prepares content-free durable admission while this controller is still
    /// dormant. This does not start a provider request, runtime or native page.
    pub fn journal_admission(
        &self,
        owner: AgentWorkIncarnation,
    ) -> Result<AgentWorkJournalMutation, AgentWorkFailure> {
        let input = self
            .state
            .as_ref()
            .and_then(|state| state.input.as_ref())
            .ok_or_else(|| crate::contract!())?;
        input.admission(owner)
    }

    /// Stable content-free run identity selected by the trusted input.
    pub fn run_identity(&self) -> Result<ContextRunId, AgentWorkFailure> {
        self.state
            .as_ref()
            .and_then(|state| state.input.as_ref())
            .map(|input| input.manifest.run())
            .ok_or_else(|| crate::contract!())
    }

    /// Original bounded absolute deadline, never extended by persistence waits.
    pub fn deadline(&self) -> Result<Instant, AgentWorkFailure> {
        self.state
            .as_ref()
            .and_then(|state| state.input.as_ref())
            .map(|input| input.settings.deadline)
            .ok_or_else(|| crate::contract!())
    }
}

impl AgentWorkController {
    /// Constructs the actor and content-free shell port. The application moves
    /// this actor into `PendingAgentRuntime::spawn_suspended_with_controller`,
    /// then binds the real engine port and owns the existing runtime controls.
    pub fn try_new(
        input: AgentWorkRunInput,
        transport: AgentProviderTransportConfig,
        access: impl Into<super::AgentBrowserModelAccess>,
        audit: Arc<dyn AgentAuditPort>,
        task: Box<dyn AgentWorkTask>,
    ) -> Result<(Self, AgentWorkHandle), AgentWorkFailure> {
        let transport = AgentProviderTransport::try_new(transport)
            .map_err(|_| AgentWorkFailure::Browser(AgentBrowserProviderError::Transport))?;
        Self::with_transport(
            input,
            transport,
            access.into(),
            audit,
            task,
            AgentBrowserRetention::Stateless,
            None,
        )
    }

    /// Release-excluded qualification adapter. The caller supplies a fresh
    /// dedicated transport and explicitly public retention mode; all runtime,
    /// policy, browser, audit and task-completion semantics remain identical.
    #[cfg(feature = "probe-harness")]
    pub fn try_new_for_probe(
        input: AgentWorkRunInput,
        transport: AgentProviderTransport,
        access: impl Into<super::AgentBrowserModelAccess>,
        audit: Arc<dyn AgentAuditPort>,
        task: Box<dyn AgentWorkTask>,
        retention: AgentBrowserRetention,
    ) -> Result<(Self, AgentWorkHandle), AgentWorkFailure> {
        let snapshot = transport.snapshot().map_err(|_| crate::contract!())?;
        if snapshot.is_sealed() || !snapshot.is_idle() {
            return Err(crate::contract!());
        }
        Self::with_transport(
            input,
            transport,
            access.into(),
            audit,
            task,
            retention,
            None,
        )
    }

    fn with_transport(
        input: AgentWorkRunInput,
        transport: AgentProviderTransport,
        access: super::AgentBrowserModelAccess,
        audit: Arc<dyn AgentAuditPort>,
        task: Box<dyn AgentWorkTask>,
        retention: AgentBrowserRetention,
        retained: Option<Box<dyn AgentWorkRetainedBrowser>>,
    ) -> Result<(Self, AgentWorkHandle), AgentWorkFailure> {
        if input.retained_resource_spec()?.isolated_public
            && !retained
                .as_ref()
                .is_some_and(|browser| browser.binding().isolated_public())
        {
            return Err(crate::contract!());
        }
        let extraction_schema = task.extraction_schema().cloned();
        let actions_before_extraction = task.allows_actions_before_extraction();
        let subtree_extraction = task.allows_subtree_extraction();
        let compound_forms = task.allows_compound_forms();
        let baseline_read = task.allows_baseline_read();
        let progressive_observation = task.allows_progressive_observation();
        let viewport_screenshot = task.allows_viewport_screenshot();
        let standalone_wait = task.allows_standalone_wait();
        let human_request = task.allows_human_request();
        let history_back_requested = task.allows_history_back();
        let history_back = history_back_requested
            && retained
                .as_ref()
                .is_some_and(|browser| browser.supports_history_back());
        let navigation_target = task.navigation_target().cloned();
        let navigation_route = task.navigation_route().cloned();
        let navigation_discovery = task.navigation_discovery().cloned();
        if progressive_observation
            && (extraction_schema.is_none()
                || !baseline_read
                || retained
                    .as_ref()
                    .is_some_and(|browser| !browser.supports_expansion()))
        {
            return Err(crate::contract!());
        }
        if retained.is_some()
            && (extraction_schema.is_none()
                || (actions_before_extraction
                    && !retained
                        .as_ref()
                        .is_some_and(|browser| browser.supports_actions()))
                || subtree_extraction
                || navigation_target.is_some()
                || navigation_route.is_some()
                || (navigation_discovery.is_some()
                    && !retained
                        .as_ref()
                        .is_some_and(|browser| browser.supports_navigation()))
                || (viewport_screenshot
                    && !retained
                        .as_ref()
                        .is_some_and(|browser| browser.supports_screenshots())))
        {
            return Err(crate::contract!());
        }
        let approved_route = input
            .manifest
            .plan_node(input.lease.node())
            .ok_or_else(|| crate::contract!())?
            .navigation_route();
        if navigation_route.as_ref() != approved_route
            || (navigation_target.is_some() && navigation_route.is_some())
            || navigation_route.as_ref().is_some_and(|route| {
                route.departure() != &input.context.target
                    || route.origin() != &input.context.origin
            })
        {
            return Err(crate::contract!());
        }
        let approved_discovery = input
            .manifest
            .plan_node(input.lease.node())
            .ok_or_else(|| crate::contract!())?
            .navigation_discovery();
        if navigation_discovery.as_ref() != approved_discovery
            || navigation_discovery.as_ref().is_some_and(|scope| {
                navigation_target.is_some()
                    || navigation_route.is_some()
                    || scope.departure() != &input.context.target
                    || scope.origin() != &input.context.origin
                    || extraction_schema.is_none()
                    || subtree_extraction
            })
        {
            return Err(crate::contract!());
        }
        if let Some(target) = &navigation_target {
            Self::validate_navigation_target(target, &input.context)?;
        }
        if (navigation_target.is_some() || navigation_route.is_some())
            && (extraction_schema.is_none()
                || actions_before_extraction
                || subtree_extraction
                || baseline_read)
        {
            return Err(crate::contract!());
        }
        if ((input.durable_result || actions_before_extraction || subtree_extraction)
            && extraction_schema.is_none())
            || task
                .extraction_schema()
                .is_some_and(|schema| schema.id().get() != 1)
        {
            return Err(crate::contract!());
        }
        let events = Arc::new(Mutex::new(WorkEvents::new(input.manifest.run())?));
        let journal = WorkJournal::new(
            &input.manifest,
            input.lease.node(),
            input.settings.ids.supervisor,
            input.settings.ids.cancellation,
            Arc::clone(&input.settings.clock),
            Arc::clone(&events),
        )?;
        let mut native = WorkNative::new(
            input.context.identity,
            input.context.origin.clone(),
            input.settings.deadline,
            retained,
        )?;
        native.clock = Some(input.settings.clock.clone());
        let terminal = Arc::new(Mutex::new(None));
        Ok((
            Self {
                state: Some(WorkState {
                    input: Some(input),
                    session: None,
                    drained: None,
                    journal: Some(journal),
                    native,
                    access: Some(access),
                    transport: Some(super::BrowserSessionTransport(transport)),
                    retention,
                    audit,
                    task,
                    extraction_schema,
                    actions_before_extraction,
                    compound_forms,
                    subtree_extraction,
                    baseline_read,
                    progressive_observation,
                    viewport_screenshot,
                    standalone_wait,
                    human_request,
                    history_back_requested,
                    history_back,
                    navigation_target,
                    navigation_route,
                    navigation_discovery,
                    navigation_hops: 0,
                    extraction: None,
                    retained_read_evidence: SemanticRetainedReadEvidence::default(),
                    follow: zephium_agentic::SemanticActionFollow::default(),
                    consent_press: false,
                    keep_follow: false,
                    covered_controls: Vec::new(),
                    no_progress: NoProgress::default(),
                    scroll_streak: 0,
                    setbacks: Setbacks::default(),
                    look_streak: 0,
                    look_run: LookRun::default(),
                    sign_in_refusals: 0,
                    loading_extract_waits: 0,
                    failure: None,
                    cleanup_failure: None,
                    observation: None,
                    native_terminal: None,
                    model_human_request: None,
                    decision_answers: None,
                    terminal_intent: None,
                }),
                terminal: Arc::clone(&terminal),
                retained_terminal: None,
            },
            AgentWorkHandle { events, terminal },
        ))
    }
}

/// What a record crop needs from the live session to carry its permission.
struct RecordCropGrant<'a> {
    baseline: &'a SemanticObservationAcknowledgement,
    config: &'a AgentProviderCallConfig,
    account: AgentContextAccountBinding,
}

struct WorkState {
    retained_read_evidence: SemanticRetainedReadEvidence,
    /// A same-site load the page started during the last action.
    follow: zephium_agentic::SemanticActionFollow,
    /// The action in flight is Rust's press of a cookie banner's refusal.
    consent_press: bool,
    /// Rust's own read keeps the load its press diverted, for following.
    keep_follow: bool,
    /// Controls of this document a click found covered, by role and name
    /// digest: the model is offered them for a key press only.
    covered_controls: Vec<u64>,
    no_progress: NoProgress,
    /// Verified scrolls in a row with nothing else done.
    scroll_streak: u8,
    setbacks: Setbacks,
    /// Whole looks of this page in a row with nothing done between them.
    look_streak: u8,
    /// Turns in a row that only asked for a look, refused or not.
    look_run: LookRun,
    /// Requests that the person sign in refused on pages with no sign-in form.
    sign_in_refusals: u8,
    /// Extractions answered with a later look because the page still loaded.
    loading_extract_waits: u8,
    navigation_target: Option<ContextNavigationTarget>,
    navigation_route: Option<AgentNavigationRoute>,
    navigation_discovery: Option<AgentNavigationDiscovery>,
    navigation_hops: usize,
    baseline_read: bool,
    progressive_observation: bool,
    viewport_screenshot: bool,
    standalone_wait: bool,
    human_request: bool,
    history_back_requested: bool,
    history_back: bool,
    extraction_schema: Option<SemanticExtractionSchema>,
    actions_before_extraction: bool,
    compound_forms: bool,
    subtree_extraction: bool,
    extraction: Option<SemanticOwnedExtractionResult>,
    input: Option<AgentWorkRunInput>,
    session: Option<AgentBrowserSession>,
    drained: Option<WorkDrained>,
    journal: Option<WorkJournal>,
    native: WorkNative,
    access: Option<super::AgentBrowserModelAccess>,
    transport: Option<super::BrowserSessionTransport>,
    retention: AgentBrowserRetention,
    audit: Arc<dyn AgentAuditPort>,
    task: Box<dyn AgentWorkTask>,
    failure: Option<AgentWorkFailure>,
    cleanup_failure: Option<AgentWorkFailure>,
    observation: Option<SemanticObservation>,
    native_terminal: Option<SemanticActionNativeSettlement>,
    model_human_request: Option<AgentWorkHumanRequest>,
    decision_answers: Option<DecisionObservationAnswers>,
    terminal_intent: Option<WorkTerminalIntent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "One Copy terminal intent per bounded worker; keep the exact human handoff inline without a separate allocation"
)]
enum WorkTerminalIntent {
    Succeeded,
    WaitingForHuman(AgentWorkHumanRequest),
    ClosedUnsuccessfully(AgentWorkFailure),
}

impl WorkState {
    fn looked(&mut self) {
        if self.look_run.looked() {
            self.withhold_snapshot(true);
        }
    }

    fn moved_on(&mut self) {
        self.look_run.reset();
        self.withhold_snapshot(false);
    }

    fn withhold_snapshot(&mut self, withheld: bool) {
        if let Some(session) = self.session.as_mut() {
            session.config = session.config.clone().with_snapshot_withheld(withheld);
        }
    }

    fn record_failure(&mut self, failure: AgentWorkFailure) {
        let failure = if failure == AgentWorkFailure::Browser(AgentBrowserProviderError::Journal) {
            self.journal_mut()
                .ok()
                .and_then(|journal| journal.failure)
                .unwrap_or(failure)
        } else {
            failure
        };
        self.failure = Some(failure);
    }

    fn navigation_length(&self) -> usize {
        if let Some(scope) = &self.navigation_discovery {
            return scope.max_hops();
        }
        self.navigation_route
            .as_ref()
            .map_or(usize::from(self.navigation_target.is_some()), |route| {
                route.destinations().len()
            })
    }

    fn has_navigation(&self) -> bool {
        self.navigation_target.is_some()
            || self.navigation_route.is_some()
            || self.navigation_discovery.is_some()
    }

    fn observation_capability(&self) -> WorkBrowserObservationCapability {
        let capability = WorkBrowserObservationCapability::for_navigation_discovery(
            self.navigation_discovery.as_ref(),
        );
        // A site task's looks hold a listing's records below the fold too,
        // after everything in view, so the planner extracts from its look
        // instead of asking for the list region by region.
        if self
            .extraction_schema
            .as_ref()
            .is_some_and(SemanticExtractionSchema::reads_whole_page)
            || self
                .navigation_discovery
                .as_ref()
                .is_some_and(AgentNavigationDiscovery::is_site_session)
        {
            capability.for_whole_page_read()
        } else {
            capability
        }
    }

    fn requires_decision_budget(&self) -> bool {
        self.extraction_schema.is_some()
            && (self.has_navigation()
                || self.baseline_read
                || self.progressive_observation
                || self.viewport_screenshot
                || self.standalone_wait
                || self.human_request
                || self.history_back
                || self.actions_before_extraction
                || self.subtree_extraction)
    }

    fn navigation_complete(&self) -> bool {
        self.navigation_hops == self.navigation_length()
    }

    fn current_navigation_target(&self) -> Option<&ContextNavigationTarget> {
        if let Some(route) = &self.navigation_route {
            route.destinations().get(self.navigation_hops)
        } else if self.navigation_hops == 0 {
            self.navigation_target.as_ref()
        } else {
            None
        }
    }

    fn refresh_account(
        &mut self,
        worker: &AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(), AgentWorkFailure> {
        self.check_task_contract()?;
        self.native.check_control(worker, browser)?;
        let context = self.native.context()?;
        let session = self.session.as_mut().ok_or_else(|| crate::contract!())?;
        session.check_live().map_err(AgentWorkFailure::Browser)?;
        let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
        let account = self.task.attest_account(context, now);
        self.native.check_control(worker, browser)?;
        let account = account?;
        let session = self.session.as_mut().ok_or_else(|| crate::contract!())?;
        session
            .refresh_account(account)
            .map_err(AgentWorkFailure::Browser)?;
        self.check_task_contract()
    }

    fn task_progress(
        &mut self,
        observation: &SemanticObservation,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure> {
        self.check_task_contract()?;
        let progress = self.task.evaluate(observation)?;
        self.check_task_contract()?;
        if self.navigation_discovery.is_some() {
            if progress != AgentWorkTaskProgress::Continue {
                return Err(crate::contract!());
            }
        } else if self.has_navigation() {
            let expected = if self.navigation_complete() {
                AgentWorkTaskProgress::ReadyForExtraction
            } else {
                AgentWorkTaskProgress::ReadyForNavigation
            };
            if progress != expected {
                return Err(crate::contract!());
            }
        } else if progress == AgentWorkTaskProgress::ReadyForNavigation {
            return Err(crate::contract!());
        }
        if (progress == AgentWorkTaskProgress::ReadyForExtraction
            && self.extraction_schema.is_none())
            || (progress == AgentWorkTaskProgress::Complete && self.actions_before_extraction)
        {
            return Err(crate::contract!());
        }
        Ok(progress)
    }

    fn action_authority(
        &self,
        observation: &SemanticObservation,
    ) -> Result<AgentProviderActionAuthority, AgentWorkFailure> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(usize::from(observation.node_count()))
            .map_err(|_| crate::contract!())?;
        for node in observation
            .frames()
            .iter()
            .flat_map(SemanticSnapshot::nodes)
        {
            let approved = self.task.model_action_operations(node, observation)?;
            let covered = !self.covered_controls.is_empty()
                && self.covered_controls.contains(&control_digest(node));
            let operations = [
                SemanticOperationClass::Click,
                SemanticOperationClass::Fill,
                SemanticOperationClass::Select,
                SemanticOperationClass::Press,
                SemanticOperationClass::Scroll,
            ]
            .into_iter()
            .filter(|operation| {
                node.operations().contains(*operation)
                    && approved.contains(*operation)
                    && !(covered && *operation == SemanticOperationClass::Click)
            })
            .collect::<Vec<_>>();
            let operations =
                SemanticOperations::try_new(&operations).map_err(|_| crate::contract!())?;
            if !operations.is_empty() {
                entries.push((node.reference(), operations));
            }
        }
        let authority = AgentProviderActionAuthority::try_new(observation, &entries)
            .ok_or_else(|| crate::contract!())?;
        Ok(match self.task.model_action_effect() {
            Some(effect) => authority.with_required_effect(effect),
            None => authority,
        })
    }

    fn check_task_contract(&self) -> Result<(), AgentWorkFailure> {
        if self.task.extraction_schema() != self.extraction_schema.as_ref()
            || self.task.navigation_target() != self.navigation_target.as_ref()
            || self.task.navigation_route() != self.navigation_route.as_ref()
            || self.task.navigation_discovery() != self.navigation_discovery.as_ref()
            || self.task.allows_actions_before_extraction() != self.actions_before_extraction
            || self.task.allows_compound_forms() != self.compound_forms
            || self.task.allows_subtree_extraction() != self.subtree_extraction
            || self.task.allows_baseline_read() != self.baseline_read
            || self.task.allows_progressive_observation() != self.progressive_observation
            || self.task.allows_viewport_screenshot() != self.viewport_screenshot
            || self.task.allows_standalone_wait() != self.standalone_wait
            || self.task.allows_human_request() != self.human_request
            || self.task.allows_history_back() != self.history_back_requested
        {
            return Err(crate::contract!());
        }
        Ok(())
    }
    fn journal_mut(&mut self) -> Result<&mut WorkJournal, AgentWorkFailure> {
        if let Some(session) = self.session.as_mut() {
            return session.journal.as_mut().ok_or_else(|| crate::contract!());
        }
        if let Some(drained) = self.drained.as_mut() {
            return drained.journal.as_mut().ok_or_else(|| crate::contract!());
        }
        self.journal.as_mut().ok_or_else(|| crate::contract!())
    }
}

struct WorkDrained {
    proposal_refusal: Option<crate::action::AgentBrowserActionProposalRefusal>,
    policy: Option<AgentRunPolicy>,
    journal: Option<WorkJournal>,
    provider: Option<AgentProviderShutdownProof>,
    native: Option<AgentNativeShutdownCoordinator>,
    resources: Option<AgentNativeShutdownResources>,
    proof: Option<AgentNativeShutdownProof>,
    delivery: Option<WorkBrowserLeaseDeliveryProof>,
    scoped_refusal: Option<Box<zephium_agent_runtime::AgentRuntimeScopedCommitRefusal>>,
}

struct WorkNative {
    resources: Option<WorkContextResources>,
    retained: Option<Box<dyn AgentWorkRetainedBrowser>>,
    retained_delivery: Option<WorkBrowserLeaseDeliveryProof>,
    clock: Option<Arc<dyn TerraControllerClock>>,
    identity: ContextIdentity,
    origin: SemanticOrigin,
    profile: Option<ContextProfileLease>,
    operation: Option<ContextOperationJoin>,
    recovery_close: Option<ContextOperationJoin>,
    observation: Option<SemanticRuntimeCorrelation>,
    snapshot_generation: Option<SemanticSnapshotGeneration>,
    screenshots: SemanticScreenshotCoordinator,
    screenshot_pending: Option<SemanticScreenshotPending>,
    action_pending: bool,
    cancellation: Option<ContextJoin>,
    shutdown_audit: Option<ContextResourceAuditId>,
    close_attempted: bool,
    deferred: Vec<AgentRuntimeEvent>,
    next: u64,
    deadline: Instant,
    revoked: bool,
}

struct WorkContextResources {
    contexts: ContextRegistry,
    profiles: ContextProfileLeaseRegistry,
    cookies: ContextCookieTransferRegistry,
}

impl WorkNative {
    // A synchronous trusted adapter cannot yield to the provider/event pump.
    // Recheck sticky controls after it returns, before admitting more work.
    fn check_control(
        &mut self,
        worker: &AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(), AgentWorkFailure> {
        self.check_stop(worker, browser)?;
        self.check_retained_health()
    }

    // Exact in-flight terminal reconciliation must obey sticky controls even
    // when document/resource authority is already gone.
    fn check_stop(
        &mut self,
        worker: &AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(), AgentWorkFailure> {
        let failure = if worker.shutdown_deadline().is_some() {
            Some(AgentWorkFailure::Shutdown)
        } else if worker.status().cancelled() {
            Some(match worker.stop_reason() {
                Some(AgentRuntimeStopReason::HumanTakeover) => AgentWorkFailure::HumanTakeover,
                Some(AgentRuntimeStopReason::Suspend) => AgentWorkFailure::SuspendRequested,
                Some(AgentRuntimeStopReason::PolicyRevoked) => AgentWorkFailure::PolicyRevoked,
                _ => AgentWorkFailure::Cancelled,
            })
        } else if worker.status().mailbox_fault().is_some() {
            Some(AgentWorkFailure::Mailbox)
        } else if Instant::now() >= self.deadline {
            Some(AgentWorkFailure::Deadline)
        } else {
            None
        };
        if let Some(failure) = failure {
            self.revoke(browser)?;
            return Err(failure);
        }
        Ok(())
    }
    fn new(
        identity: ContextIdentity,
        origin: SemanticOrigin,
        deadline: Instant,
        retained: Option<Box<dyn AgentWorkRetainedBrowser>>,
    ) -> Result<Self, AgentWorkFailure> {
        let mut deferred = Vec::new();
        deferred
            .try_reserve_exact(super::MAX_DEFERRED_RUNTIME_EVENTS)
            .map_err(|_| AgentWorkFailure::Backpressure)?;
        Ok(Self {
            resources: retained.is_none().then(|| WorkContextResources {
                contexts: ContextRegistry::new(),
                profiles: ContextProfileLeaseRegistry::new(),
                cookies: ContextCookieTransferRegistry::new(),
            }),
            retained,
            retained_delivery: None,
            clock: None,
            identity,
            origin,
            profile: None,
            operation: None,
            recovery_close: None,
            observation: None,
            snapshot_generation: None,
            screenshots: SemanticScreenshotCoordinator::new(),
            screenshot_pending: None,
            action_pending: false,
            cancellation: None,
            shutdown_audit: None,
            close_attempted: false,
            deferred,
            next: 1,
            deadline,
            revoked: false,
        })
    }

    fn id(&mut self) -> Result<u64, AgentWorkFailure> {
        let id = self.next;
        self.next = id.checked_add(1).ok_or_else(|| crate::contract!())?;
        Ok(id)
    }

    fn contexts(&mut self) -> Result<&mut ContextRegistry, AgentWorkFailure> {
        self.resources
            .as_mut()
            .map(|resources| &mut resources.contexts)
            .ok_or(AgentWorkFailure::Context)
    }
    fn context(&mut self) -> Result<ContextJoin, AgentWorkFailure> {
        if let Some(retained) = &self.retained {
            Ok(retained.binding().frame().context())
        } else {
            let id = self.identity.id();
            self.contexts()?
                .join(id)
                .map_err(|_| AgentWorkFailure::Context)
        }
    }
    fn profiles(&mut self) -> Result<&mut ContextProfileLeaseRegistry, AgentWorkFailure> {
        self.resources
            .as_mut()
            .map(|resources| &mut resources.profiles)
            .ok_or(AgentWorkFailure::Context)
    }

    fn retain(&mut self, event: AgentRuntimeEvent) -> Result<(), AgentWorkFailure> {
        if self.deferred.len() >= super::MAX_DEFERRED_RUNTIME_EVENTS {
            return Err(AgentWorkFailure::Backpressure);
        }
        self.deferred.push(event);
        Ok(())
    }

    fn revoke(&mut self, browser: &WorkBrowser<'_>) -> Result<(), AgentWorkFailure> {
        if self.revoked {
            return Ok(());
        }
        self.revoked = true;
        if let Some(retained) = self.retained.as_mut() {
            return retained.begin_revocation();
        }
        let id = self.identity.id();
        let registry = self.contexts()?;
        let Ok(prior) = registry.join(id) else {
            return Ok(());
        };
        let current = registry
            .cancel_run(id, prior)
            .map_err(|_| AgentWorkFailure::Context)?;
        // Rust invalidation precedes native dispatch, including provider wait.
        if browser.dispatch(ContextNativeRequest::Cancel(
            ContextCancellationRequest::new(current),
        )) == ContextDispatch::Scheduled
        {
            self.cancellation = Some(current);
        } else {
            return Err(AgentWorkFailure::Context);
        }
        Ok(())
    }

    async fn next_event(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<AgentRuntimeEvent, AgentWorkFailure> {
        let deadline = worker
            .shutdown_deadline()
            .map_or(self.deadline, |at| at.min(self.deadline));
        if Instant::now() >= deadline {
            self.revoke(browser)?;
            return Err(AgentWorkFailure::Deadline);
        }
        let health = if self.revoked {
            None
        } else {
            self.retained.as_ref()
        };
        let clock = &self.clock;
        let event = {
            let event = worker.next_event();
            tokio::pin!(event);
            tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                std::future::poll_fn(|cx| {
                    if let Some(retained) = health {
                        let result = clock
                            .as_ref()
                            .ok_or_else(|| crate::contract!())
                            .and_then(|clock| clock.now().map_err(|_| crate::contract!()))
                            .and_then(|now| retained.check_health(now));
                        if let Err(error) = result {
                            return std::task::Poll::Ready(Err(error));
                        }
                    }
                    std::future::Future::poll(event.as_mut(), cx)
                        .map(|event| event.map_err(|_| AgentWorkFailure::Mailbox))
                }),
            )
            .await
        };
        let event = match event {
            Ok(Ok(event)) => event,
            Ok(Err(failure)) => {
                self.revoke(browser)?;
                return Err(failure);
            }
            Err(_) => {
                self.revoke(browser)?;
                return Err(AgentWorkFailure::Deadline);
            }
        };
        match event {
            AgentRuntimeEvent::CancellationRequested => {
                self.revoke(browser)?;
                Err(match worker.stop_reason() {
                    Some(AgentRuntimeStopReason::HumanTakeover) => AgentWorkFailure::HumanTakeover,
                    Some(AgentRuntimeStopReason::Suspend) => AgentWorkFailure::SuspendRequested,
                    Some(AgentRuntimeStopReason::PolicyRevoked) => AgentWorkFailure::PolicyRevoked,
                    _ => AgentWorkFailure::Cancelled,
                })
            }
            AgentRuntimeEvent::ShutdownRequested => {
                self.revoke(browser)?;
                Err(AgentWorkFailure::Shutdown)
            }
            AgentRuntimeEvent::NavigationReplaced(replacement) => {
                let id = self.identity.id();
                if replacement.prior().identity() == self.identity {
                    self.contexts()?
                        .observe_navigation_replacement(id, replacement.prior())
                        .map_err(|_| AgentWorkFailure::Context)?;
                }
                self.revoke(browser)?;
                self.retain(AgentRuntimeEvent::NavigationReplaced(replacement))?;
                Err(AgentWorkFailure::ContextLost)
            }
            AgentRuntimeEvent::RendererLost(loss) => {
                let id = self.identity.id();
                if loss.prior().identity() == self.identity {
                    self.contexts()?
                        .renderer_lost(id, loss.prior())
                        .map_err(|_| AgentWorkFailure::Context)?;
                }
                self.revoke(browser)?;
                self.retain(AgentRuntimeEvent::RendererLost(loss))?;
                Err(AgentWorkFailure::ContextLost)
            }
            event if worker.status().mailbox_fault().is_some() => {
                self.revoke(browser)?;
                self.retain(event)?;
                Err(AgentWorkFailure::Mailbox)
            }
            event => Ok(event),
        }
    }

    async fn cleanup_event(
        worker: &mut AgentRuntimeWorker,
        deadline: Instant,
    ) -> Result<AgentRuntimeEvent, AgentWorkFailure> {
        loop {
            let deadline = worker
                .shutdown_deadline()
                .map_or(deadline, |at| at.min(deadline));
            if Instant::now() >= deadline {
                return Err(AgentWorkFailure::Deadline);
            }
            let event = tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                worker.next_event_for_terminal_cleanup(),
            )
            .await
            .map_err(|_| AgentWorkFailure::Deadline)?;
            if !matches!(
                event,
                AgentRuntimeEvent::CancellationRequested | AgentRuntimeEvent::ShutdownRequested
            ) {
                return Ok(event);
            }
        }
    }

    async fn terminal_event(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        cleanup: Option<Instant>,
    ) -> Result<AgentRuntimeEvent, AgentWorkFailure> {
        match cleanup {
            Some(deadline) => Self::cleanup_event(worker, deadline).await,
            None => self.next_event(worker, browser).await,
        }
    }
}

impl AgentRuntimeController for AgentWorkController {
    fn run(
        self: Box<Self>,
        mut worker: AgentRuntimeWorker,
        browser: AgentRuntimeBrowser,
    ) -> AgentRuntimeControllerFuture {
        Box::pin(async move {
            let browser = WorkBrowser::Legacy(&browser);
            let mut controller = *self;
            if let Err(failure) = controller.execute(&mut worker, &browser).await {
                if let Some(state) = controller.state.as_mut() {
                    state.record_failure(failure);
                    let _ = state.native.revoke(&browser);
                    if let Some(session) = state.session.as_ref() {
                        session.cancel();
                    }
                    let deadline = controller.drain_recovery(&mut worker, &browser).await;
                    let cleanup = controller
                        .close_unsuccessful(&mut worker, &browser, deadline)
                        .await;
                    if let (Err(failure), Some(state)) = (cleanup, controller.state.as_mut()) {
                        state.cleanup_failure = Some(failure);
                    }
                }
            }
        })
    }
}

impl AgentWorkController {
    async fn execute(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_mut().ok_or_else(|| crate::contract!())?;
        let started = state.native.next_event(worker, browser).await?;
        if !matches!(started, AgentRuntimeEvent::RunStarted(_)) {
            state.native.retain(started)?;
            return Err(AgentWorkFailure::Mailbox);
        }
        let input = state.input.as_ref().ok_or_else(|| crate::contract!())?;
        let journal = state.journal.as_mut().ok_or_else(|| crate::contract!())?;
        journal.start(input.settings.ids.attempt)?;
        if state.native.retained.is_some() {
            journal.emit(AgentWorkEventKind::ContextActive)?;
            self.start_session()?;
            self.browser_loop(worker, browser).await?;
            return self.close_retained(worker, None).await;
        }
        let caps = ContextCapabilities::try_new(
            ContextKind::Owned,
            &[
                ContextCapability::Navigate,
                ContextCapability::Observe,
                ContextCapability::Act,
            ],
        )
        .map_err(|_| crate::contract!())?;
        let id = state.native.identity.id();
        journal
            .supervisor
            .reserve_context(
                journal
                    .execution
                    .as_ref()
                    .ok_or_else(|| crate::contract!())?,
                &input.manifest,
                state.native.contexts()?,
                input.context.identity,
                caps,
            )
            .map_err(|_| AgentWorkFailure::Context)?;
        journal.record()?;
        journal.emit(AgentWorkEventKind::ContextActive)?;
        let lease_id =
            ContextProfileLeaseId::new(state.native.id()?).ok_or_else(|| crate::contract!())?;
        let lease = state
            .native
            .profiles()?
            .acquire(
                lease_id,
                input.context.identity,
                input.context.storage,
                ContextProfileLeasePurpose::Owned,
            )
            .map_err(|_| AgentWorkFailure::Context)?;
        state.native.profile = Some(lease);
        let op = ContextOperationId::new(state.native.id()?).ok_or_else(|| crate::contract!())?;
        let operation = state
            .native
            .contexts()?
            .begin_context(id, op)
            .map_err(|_| AgentWorkFailure::Context)?;
        let request = ContextConstructionRequest::try_new(
            operation,
            caps,
            lease,
            ContextConstructionSource::Owned,
        )
        .map_err(|_| AgentWorkFailure::Context)?;
        state.native.operation = Some(operation);
        if browser.dispatch(ContextNativeRequest::Construct(request)) != ContextDispatch::Scheduled
        {
            state.native.operation = None;
            state
                .native
                .contexts()?
                .settle_construction(id, operation, ContextSettlement::Refused)
                .map_err(|_| AgentWorkFailure::Context)?;
            return Err(AgentWorkFailure::Context);
        }
        match state.native.next_event(worker, browser).await? {
            AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::ConstructionSettled(
                settlement,
            )) if settlement.operation() == operation => {
                state.native.operation = None;
                let applied = settlement.outcome().is_ok();
                state
                    .native
                    .contexts()?
                    .settle_construction(
                        id,
                        operation,
                        if applied {
                            ContextSettlement::Applied
                        } else {
                            ContextSettlement::Refused
                        },
                    )
                    .map_err(|_| AgentWorkFailure::Context)?;
                if !applied {
                    return Err(AgentWorkFailure::Native(
                        settlement
                            .outcome()
                            .err()
                            .ok_or(AgentWorkFailure::Context)?,
                    ));
                }
            }
            event => {
                state.native.retain(event)?;
                return Err(AgentWorkFailure::Mailbox);
            }
        }
        let op = ContextOperationId::new(state.native.id()?).ok_or_else(|| crate::contract!())?;
        let operation = state
            .native
            .contexts()?
            .begin_navigation(id, op)
            .map_err(|_| AgentWorkFailure::Context)?;
        let input = state.input.as_ref().ok_or_else(|| crate::contract!())?;
        let request = ContextNavigationRequest::try_new(operation, input.context.target.clone())
            .map_err(|_| AgentWorkFailure::Context)?;
        state.native.operation = Some(operation);
        if browser.dispatch(ContextNativeRequest::Navigate(request)) != ContextDispatch::Scheduled {
            state.native.operation = None;
            state
                .native
                .contexts()?
                .settle_navigation(id, operation, ContextSettlement::Refused)
                .map_err(|_| AgentWorkFailure::Context)?;
            return Err(AgentWorkFailure::Context);
        }
        match state.native.next_event(worker, browser).await? {
            AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::NavigationSettled(
                settlement,
            )) if settlement.operation() == operation => {
                state.native.operation = None;
                let applied = settlement.outcome().as_ref().is_ok_and(|target| {
                    Some(target) == state.input.as_ref().map(|input| &input.context.target)
                });
                state
                    .native
                    .contexts()?
                    .settle_navigation(
                        id,
                        operation,
                        if applied {
                            ContextSettlement::Applied
                        } else {
                            ContextSettlement::Refused
                        },
                    )
                    .map_err(|_| AgentWorkFailure::Context)?;
                if !applied {
                    return Err(AgentWorkFailure::Context);
                }
            }
            event => {
                state.native.retain(event)?;
                return Err(AgentWorkFailure::Mailbox);
            }
        }
        self.start_session()?;
        self.browser_loop(worker, browser).await?;
        self.close_success(worker, browser).await
    }

    fn start_session(&mut self) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_mut().ok_or_else(|| crate::contract!())?;
        let mut input = state.input.take().ok_or_else(|| crate::contract!())?;
        let context = state.native.context()?;
        let now = input.settings.clock.now().map_err(|_| crate::contract!())?;
        let account = state.task.attest_account(context, now)?;
        let frame = SemanticFrameJoin::try_new(
            context,
            FrameId::MAIN,
            context.frame_generation(),
            state.native.origin.clone(),
            SemanticFrameTrust::SameOrigin,
        )
        .map_err(|_| AgentWorkFailure::Context)?;
        let run = TerraControllerRunInput {
            manifest: input.manifest,
            lease: input.lease,
            account,
            observation: SemanticObservationRequest::initial(
                SemanticObservationId::new(1).ok_or_else(|| crate::contract!())?,
                context,
                SemanticObservationBudget::INITIAL_FILTERED,
            ),
            frame,
            invocation: SemanticInvocationId::new(1).ok_or_else(|| crate::contract!())?,
            snapshot_generation: SemanticSnapshotGeneration::INITIAL,
            objective: input.objective.take().ok_or_else(|| crate::contract!())?,
            ids: input.settings.ids,
            clock: input.settings.clock,
            deadline: input.settings.deadline,
            max_model_calls: input.settings.max_model_calls,
            max_actions: input.settings.max_actions,
        };
        let decision_budget = state.requires_decision_budget();
        let mut session = AgentBrowserSession::try_new_with_transport(
            run,
            state.transport.take().ok_or_else(|| crate::contract!())?,
            state.access.take().ok_or_else(|| crate::contract!())?,
            input.settings.model,
            state.retention,
        )
        .map_err(AgentWorkFailure::Browser)?;
        if let Some(host) = input.model_image_host.take() {
            let source = state
                .native
                .retained
                .as_ref()
                .and_then(|retained| retained.screenshot_source_port())
                .ok_or_else(|| crate::contract!())?;
            session
                .policy
                .bind_model_image_owners(
                    session.lease.lease(),
                    source,
                    host,
                    input.original_anonymous_read.clone(),
                )
                .map_err(|_| crate::contract!())?;
        }
        if let Some(provider) = input.decision_provider.take() {
            if let Err(error) = session.configure_decisions(provider, input.decision_model.take()) {
                session.journal = state.journal.take();
                state.session = Some(session);
                return Err(AgentWorkFailure::Browser(error));
            }
        }
        if state.navigation_discovery.is_some() {
            if let Some(retained) = &state.native.retained {
                session
                    .policy
                    .bind_retained_initial_document(retained.binding())
                    .map_err(|_| crate::contract!())?;
            }
        }
        session.journal = state.journal.take();
        if state.extraction_schema.is_some() {
            session.config = match (state.actions_before_extraction, state.subtree_extraction) {
                (true, true) => session.config.restrict_to_actions_and_scoped_extraction(),
                (false, true) => session.config.restrict_to_scoped_extraction(),
                (true, false) => session.config.restrict_to_actions_and_extraction(),
                (false, false) => session.config.restrict_to_extraction(),
            };
        }
        if state.has_navigation() {
            session.config = if state.actions_before_extraction
                && state
                    .navigation_discovery
                    .as_ref()
                    .is_some_and(AgentNavigationDiscovery::is_site_session)
            {
                session.config.restrict_to_site_actions_and_extraction()
            } else if state.actions_before_extraction {
                session
                    .config
                    .restrict_to_navigation_actions_and_extraction()
            } else {
                session.config.restrict_to_navigation_and_extraction()
            };
        }
        session.config = session
            .config
            .with_navigation_available(!state.navigation_complete());
        if state.compound_forms {
            session.config = session.config.with_compound_form();
        }
        if state.baseline_read {
            session.config = session.config.with_baseline_read();
        }
        if state.progressive_observation {
            session.config = session.config.with_progressive_observation();
        }
        if state.viewport_screenshot && session.supports_vision() {
            session.config = session.config.with_viewport_screenshot();
        }
        if state.standalone_wait {
            session.config = session.config.with_standalone_wait();
        }
        if state.human_request {
            session.config = session.config.with_human_request();
        }
        if state.history_back {
            session.config = session.config.with_history_back();
        }
        session.site_session = state
            .navigation_discovery
            .as_ref()
            .is_some_and(AgentNavigationDiscovery::is_site_session);
        if decision_budget {
            session.config = session
                .config
                .with_decision_budget(
                    AgentModelCallId::new(session.next_call).ok_or_else(|| crate::contract!())?,
                    session.max_model_calls,
                )
                .map_err(|_| crate::contract!())?;
        }
        state.session = Some(session);
        Ok(())
    }

    async fn classify_human_challenge(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        observation: &SemanticObservation,
        locate_only: bool,
    ) -> Result<bool, AgentWorkFailure> {
        state.decision_answers = None;
        // A well-known check is read by Rust alone: no decision is bought.
        if known_check(observation).is_some() {
            return Ok(true);
        }
        let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
        if session.decisions.is_none() || session.actions_only_decisions() {
            return Ok(looks_like_human_challenge(observation));
        }
        state.refresh_account(worker, browser)?;
        let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
        let authority = if session.config.permits_tool(AgentBrowserToolKind::Act) {
            state.action_authority(observation)?
        } else {
            AgentProviderActionAuthority::try_new(observation, &[])
                .ok_or_else(|| crate::contract!())?
        };
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let answers = Self::provider(
            &mut state.native,
            worker,
            browser,
            session.cancellation.clone(),
            session.decide_observation(
                observation,
                &authority,
                state.extraction_schema.as_ref(),
                locate_only,
            ),
        )
        .await?;
        let Some(mut answers) = answers else {
            return Ok(looks_like_human_challenge(observation));
        };
        let challenge = answers
            .take_challenge(observation, session.account)
            .map_err(AgentWorkFailure::DecisionChallenge)?;
        state.decision_answers = Some(answers);
        Ok(challenge.unwrap_or_else(|| looks_like_human_challenge(observation)))
    }

    /// Let automatic verification settle inside the original deadline before takeover.
    async fn settle_human_challenge(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        mut observation: SemanticObservation,
    ) -> Result<(SemanticObservation, bool), AgentWorkFailure> {
        if state.human_request && Self::raise_human_wall(state, &observation)? {
            return Ok((observation, true));
        }
        if !state.human_request
            || !Self::classify_human_challenge(state, worker, browser, &observation, false).await?
        {
            return Ok((observation, false));
        }
        // A widget only a person can pass goes to them from the first look; a
        // well-known script check is looked at again each second until it
        // clears, never shown to the person when it does.
        let known = known_check(&observation);
        let schedule: &[u64] = match known {
            Some(KnownCheck::Person) => &[],
            Some(KnownCheck::Automatic) if state.task.patient_checks() => {
                &AUTOMATIC_CHECK_SETTLE_MILLIS
            }
            Some(KnownCheck::Automatic) => &AUTOMATIC_CHECK_SETTLE_MILLIS[..2],
            None => &HUMAN_CHALLENGE_SETTLE_MILLIS,
        };
        for millis in schedule {
            let wake = Instant::now()
                .checked_add(Duration::from_millis(*millis))
                .ok_or(AgentWorkFailure::Deadline)?;
            if wake >= state.native.deadline {
                break;
            }
            tokio::select! {
                biased;
                event = state.native.next_event(worker, browser) => {
                    state.native.retain(event?)?;
                    return Err(AgentWorkFailure::Mailbox);
                }
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
            }
            state.refresh_account(worker, browser)?;
            // A check that cleared reloads its page: the new document gets a
            // first look of its own.
            observation = match Self::observe(state, worker, browser).await {
                Err(AgentWorkFailure::Observation(
                    SemanticRuntimePortFailure::NotReady
                    | SemanticRuntimePortFailure::Stale
                    | SemanticRuntimePortFailure::DocumentReplaced,
                )) => Box::pin(Self::observe_initial_ready(state, worker, browser)).await?,
                look => look?,
            };
            observation = Self::fit_model_observation(observation)?;
            let still = match known {
                Some(_) => match known_check(&observation) {
                    Some(KnownCheck::Person) => break,
                    Some(KnownCheck::Automatic) => true,
                    None => looks_like_human_challenge(&observation),
                },
                None => {
                    Self::classify_human_challenge(state, worker, browser, &observation, false)
                        .await?
                }
            };
            if !still {
                return Ok((observation, false));
            }
        }
        let reason = AgentBrowserHumanReason::HumanChallenge;
        state
            .journal_mut()?
            .emit(AgentWorkEventKind::ModelRequestedHuman(reason))?;
        state.model_human_request = Some(AgentWorkHumanRequest {
            context: observation.request().context(),
            observation: observation.request().id(),
            generation: observation.request().generation(),
            reason,
            retained_resource: state
                .native
                .retained
                .as_ref()
                .map(|browser| browser.binding().lease().resource().identity()),
        });
        Ok((observation, true))
    }

    /// Stops for the person when the task reads a wall on this page, or the
    /// page is a well-known check only a person can pass. That check is read
    /// the same way after any step as at the first look: a page the model
    /// meets it on later is not another page to spend model calls on.
    fn raise_human_wall(
        state: &mut WorkState,
        observation: &SemanticObservation,
    ) -> Result<bool, AgentWorkFailure> {
        let Some(reason) = state
            .task
            .human_wall(observation)
            .or_else(|| {
                state
                    .native
                    .retained
                    .as_ref()
                    .filter(|browser| {
                        challenge::sign_in_route(browser.binding().document(), observation)
                    })
                    .map(|_| AgentBrowserHumanReason::SignIn)
            })
            .or_else(|| {
                (known_check(observation) == Some(KnownCheck::Person))
                    .then_some(AgentBrowserHumanReason::HumanChallenge)
            })
        else {
            return Ok(false);
        };
        state
            .journal_mut()?
            .emit(AgentWorkEventKind::ModelRequestedHuman(reason))?;
        state.model_human_request = Some(AgentWorkHumanRequest {
            context: observation.request().context(),
            observation: observation.request().id(),
            generation: observation.request().generation(),
            reason,
            retained_resource: state
                .native
                .retained
                .as_ref()
                .map(|browser| browser.binding().lease().resource().identity()),
        });
        Ok(true)
    }

    async fn observe(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        let look = Self::observe_page(state, worker, browser).await?;
        Self::capture_app_view(state, &look);
        Ok(look)
    }

    /// QA builds keep a daily app's views as reader fixtures.
    fn capture_app_view(state: &WorkState, look: &SemanticObservation) {
        let Some(browser) = state.native.retained.as_ref() else {
            return;
        };
        let page = browser.binding().document().as_url();
        if let Some(app) = page.host_str().and_then(zephium_agentic::DailyApp::of) {
            zephium_agentic::captured_app_view(app, page, look);
        }
    }

    async fn observe_page(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        state.journal_mut()?.emit(AgentWorkEventKind::Observing)?;
        if state
            .native
            .retained
            .as_ref()
            .is_some_and(|browser| !browser.allows_readiness_retry())
        {
            return Self::observe_once(state, worker, browser).await;
        }
        // Only an explicit pre-dispatch NotReady receipt, or the runtime's own
        // "still parsing" refusal (a sign-in redirect in flight), can start
        // another read-only readiness check. Never retry a mutation, stale
        // reference, replaced document, malformed callback or provider turn. Another
        // page's look holds the one presentation for up to its own budget, so
        // readiness is waited for by time, not by a count of quick refusals.
        let started = Instant::now();
        let mut delay = Duration::from_millis(50);
        // A document still parsing is looked at again only a few times, a
        // second and more apart: each look shows the page, and one that never
        // settles (a sign-in redirect) is better reported than watched.
        let mut parsing = 0u32;
        // A look a busy page never took timed out without running: the page
        // keeps its channel, and one more look is read-only.
        let mut timed_out = false;
        while started.elapsed() < NOT_READY_PATIENCE {
            let result = Self::observe_once(state, worker, browser).await;
            if !timed_out
                && matches!(
                    result,
                    Err(AgentWorkFailure::Observation(
                        SemanticRuntimePortFailure::TimedOut
                    ))
                )
            {
                timed_out = true;
                continue;
            }
            let still_parsing = matches!(
                result,
                Err(AgentWorkFailure::Observation(
                    SemanticRuntimePortFailure::Result(SemanticRuntimeResultError::Runtime(
                        SemanticRuntimeFault::DocumentLoading
                    ),)
                ))
            );
            if still_parsing {
                parsing += 1;
                if parsing > DOCUMENT_LOADING_LOOKS {
                    return result;
                }
                delay = Duration::from_millis(800) * parsing;
            }
            match result {
                Err(AgentWorkFailure::Observation(
                    SemanticRuntimePortFailure::NotReady
                    | SemanticRuntimePortFailure::Result(SemanticRuntimeResultError::Runtime(
                        SemanticRuntimeFault::DocumentLoading,
                    )),
                )) => {
                    tokio::select! {
                        biased;
                        event = state.native.next_event(worker, browser) => {
                            state.native.retain(event?)?;
                            return Err(AgentWorkFailure::Mailbox);
                        }
                        () = tokio::time::sleep(delay) => {}
                    }
                    if !still_parsing {
                        delay = (delay * 2).min(Duration::from_millis(400));
                    }
                }
                result => return result,
            }
        }
        Err(AgentWorkFailure::Observation(
            SemanticRuntimePortFailure::NotReady,
        ))
    }

    async fn observe_once(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        if state.native.retained.is_some() {
            let capability = state.observation_capability();
            return state.native.observe_retained(worker, capability).await;
        }
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        session.check_live().map_err(AgentWorkFailure::Browser)?;
        let id = state.native.identity.id();
        let context = state
            .native
            .contexts()?
            .join(id)
            .map_err(|_| AgentWorkFailure::Context)?;
        let frame = SemanticFrameJoin::try_new(
            context,
            FrameId::MAIN,
            context.frame_generation(),
            state.native.origin.clone(),
            SemanticFrameTrust::SameOrigin,
        )
        .map_err(|_| AgentWorkFailure::Context)?;
        let next = state.native.id()?;
        let request = SemanticObservationRequest::initial(
            SemanticObservationId::new(next).ok_or_else(|| crate::contract!())?,
            context,
            SemanticObservationBudget::INITIAL_FILTERED,
        );
        Self::capture_once(state, worker, browser, request, frame).await
    }

    async fn capture_once(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        request: SemanticObservationRequest,
        frame: SemanticFrameJoin,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        let id = state.native.identity.id();
        let context = state
            .native
            .contexts()?
            .join(id)
            .map_err(|_| AgentWorkFailure::Context)?;
        if context != request.context()
            || frame.context() != context
            || frame.origin() != &state.native.origin
        {
            return Err(AgentWorkFailure::Context);
        }
        let next = state.native.id()?;
        let generation = state
            .native
            .snapshot_generation
            .map_or(
                Some(SemanticSnapshotGeneration::INITIAL),
                SemanticSnapshotGeneration::next,
            )
            .ok_or_else(|| crate::contract!())?;
        let runtime_budget = if state
            .navigation_discovery
            .as_ref()
            .is_some_and(AgentNavigationDiscovery::is_production)
        {
            SemanticRuntimeBudget::INITIAL_FILTERED.with_link_url_state()
        } else {
            SemanticRuntimeBudget::INITIAL_FILTERED
        };
        let invocation = encode_semantic_runtime_invocation(
            &request,
            frame,
            SemanticInvocationId::new(next).ok_or_else(|| crate::contract!())?,
            generation,
            runtime_budget,
        )
        .map_err(|_| AgentWorkFailure::Context)?;
        let correlation = invocation.correlation();
        state.native.observation = Some(correlation.clone());
        if browser.invoke_semantic(invocation) != ContextDispatch::Scheduled {
            state.native.observation = None;
            return Err(AgentWorkFailure::Context);
        }
        match state.native.next_event(worker, browser).await? {
            AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::SemanticRuntimeSettled(
                settlement,
            )) if settlement.correlation() == &correlation => {
                state.native.observation = None;
                let snapshot = settlement
                    .into_outcome()
                    .map_err(AgentWorkFailure::Observation)?;
                state.native.snapshot_generation = Some(generation);
                // This controller admits only its exact main frame. Keep each
                // encountered child boundary visible with an explicit refusal;
                // do not infer a child origin, capture it, or lose the usable
                // main document merely because it embeds an iframe.
                let boundaries = snapshot
                    .nodes()
                    .iter()
                    .filter(|node| node.role() == SemanticRole::FrameBoundary)
                    .map(SemanticNode::reference)
                    .collect::<Vec<_>>();
                let mut assembler = SemanticObservationAssembler::new(request, snapshot)
                    .map_err(|_| AgentWorkFailure::Context)?;
                for boundary in boundaries {
                    assembler
                        .mark_frame_unsupported(
                            FrameId::MAIN,
                            boundary,
                            SemanticFrameUnsupported::PolicyBlocked,
                        )
                        .map_err(|_| AgentWorkFailure::Context)?;
                }
                let observation = assembler.finish().map_err(|_| AgentWorkFailure::Context)?;
                state
                    .native
                    .contexts()?
                    .acknowledge_observation(id, context)
                    .map_err(|_| AgentWorkFailure::Context)?;
                Ok(observation)
            }
            event => {
                state.native.retain(event)?;
                Err(AgentWorkFailure::Mailbox)
            }
        }
    }

    async fn provider<T>(
        native: &mut WorkNative,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        cancellation: AgentProviderCancellation,
        future: impl std::future::Future<Output = Result<T, AgentBrowserProviderError>>,
    ) -> Result<T, AgentWorkFailure> {
        tokio::pin!(future);
        tokio::select! {
            biased;
            event = native.next_event(worker, browser) => {
                cancellation.cancel();
                match event { Ok(event) => { native.retain(event)?; Err(AgentWorkFailure::Mailbox) }, Err(failure) => Err(failure) }
            }
            result = &mut future => result.map_err(AgentWorkFailure::Browser),
        }
    }

    async fn observe_initial_ready(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        // Seven samples at most; backoff schedules work, it is never evidence
        // of readiness. The original run deadline/control lane remains live.
        const DELAYS_MS: [u64; 6] = [250, 500, 1000, 2000, 2000, 2000];
        let mut observation = Self::observe(state, worker, browser).await?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(AgentWorkFailure::Deadline)?
            .min(state.native.deadline);
        let mut delays = DELAYS_MS.into_iter();
        // A site task's first page may still be an empty shell: an app
        // booting, or a bot check about to reload its page.
        let site = state
            .navigation_discovery
            .as_ref()
            .is_some_and(AgentNavigationDiscovery::is_site_session);
        loop {
            state.check_task_contract()?;
            let readiness = state.task.initial_readiness(&observation)?;
            state.check_task_contract()?;
            state.native.check_control(worker, browser)?;
            if readiness == AgentWorkInitialReadiness::Ready
                && !has_dominant_loading_placeholders(&observation)
                && !(site && (unrendered(&observation) || announces_loading(&observation)))
            {
                return Ok(observation);
            }
            // A bot check is not a page still loading: its own settling
            // decides how long to wait and whether the person is asked.
            if known_check(&observation).is_some() {
                return Ok(observation);
            }
            state.refresh_account(worker, browser)?;
            if state
                .native
                .retained
                .as_ref()
                .is_some_and(|b| !b.allows_readiness_retry())
            {
                return finish_initial_readiness_wait(observation, readiness);
            }
            let Some(delay) = delays.next() else {
                return finish_initial_readiness_wait(observation, readiness);
            };
            let wake = Instant::now()
                .checked_add(Duration::from_millis(delay))
                .ok_or(AgentWorkFailure::Deadline)?;
            if wake >= deadline {
                return finish_initial_readiness_wait(observation, readiness);
            }
            tokio::select! {
                biased;
                event = state.native.next_event(worker, browser) => {
                    state.native.retain(event?)?;
                    return Err(AgentWorkFailure::Mailbox);
                }
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
            }
            state.refresh_account(worker, browser)?;
            let next = Self::observe(state, worker, browser).await?;
            // A first page that reloaded itself (a bot check that cleared)
            // is a new document: its look continues the wait.
            if !replaced_document(&observation, &next) {
                validate_initial_readiness_successor(&observation, &next)?;
            }
            if Instant::now() >= deadline {
                state.check_task_contract()?;
                let readiness = state.task.initial_readiness(&next)?;
                state.check_task_contract()?;
                state.native.check_control(worker, browser)?;
                return finish_initial_readiness_wait(next, readiness);
            }
            observation = next;
        }
    }

    /// One code-owned capture of the whole document under the whole-page
    /// capability. The initial capture holds the viewport and the headings
    /// below it; this one holds the sections' own text. No provider is asked.
    async fn whole_page_capture(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        initial: SemanticObservation,
        kind: SemanticExpansionKind,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        let Some((baseline, root)) = state
            .native
            .retained
            .is_some()
            .then(|| SemanticObservationAcknowledgement::whole_page_scope(&initial))
            .flatten()
        else {
            return Ok(initial);
        };
        state.native.check_control(worker, browser)?;
        state.refresh_account(worker, browser)?;
        state.journal_mut()?.emit(AgentWorkEventKind::Observing)?;
        let capability = state.observation_capability();
        match state
            .native
            .observe_retained_scope(worker, Some((&initial, &baseline, root, kind)), capability)
            .await
        {
            // A catalog's records may sit under landmarks the region capture
            // leaves truncated; it keeps whichever look holds more of the page.
            Ok(page)
                if state
                    .extraction_schema
                    .as_ref()
                    .is_some_and(SemanticExtractionSchema::is_row_collection)
                    && node_count(&page) < node_count(&initial) =>
            {
                Ok(initial)
            }
            Ok(page) => {
                Self::capture_app_view(state, &page);
                Ok(page)
            }
            Err(AgentWorkFailure::InspectionAnchorLost) => Ok(initial),
            // The page replaced itself while it was read whole (Allegro's bot
            // check reloads its page twice): its new document gets a first
            // look of its own, waited for like any first page.
            Err(AgentWorkFailure::Observation(
                SemanticRuntimePortFailure::NotReady
                | SemanticRuntimePortFailure::Stale
                | SemanticRuntimePortFailure::DocumentReplaced,
            )) => Box::pin(Self::observe_initial_ready(state, worker, browser)).await,
            Err(error) => Err(error),
        }
    }

    async fn browser_loop(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_mut().ok_or_else(|| crate::contract!())?;
        // A whole-page findings or catalog read decides over its whole
        // capture; only the page planner, if it is needed, sees the fitted
        // prefix.
        let whole_page = state
            .extraction_schema
            .as_ref()
            .is_some_and(SemanticExtractionSchema::reads_whole_page);
        let observation = Self::observe_initial_ready(state, worker, browser).await?;
        let first_look = state.task.whole_first_look();
        let single_record = state
            .extraction_schema
            .as_ref()
            .is_some_and(SemanticExtractionSchema::is_single_record);
        // A single product needs the document budget, but recursive gallery
        // expansion can crowd its later identity metadata out of that budget.
        let observation = if (whole_page && !single_record) || first_look {
            // Boxed: the read loop's future must stay well inside the
            // runtime worker's stack.
            // A first look reads the whole document, landmarks and all.
            // A whole-page read must materialize content under main/landmark
            // boundaries, within the same bounded native observation budget.
            let kind = SemanticExpansionKind::Subtree;
            Box::pin(Self::whole_page_capture(
                state,
                worker,
                browser,
                observation,
                kind,
            ))
            .await?
        } else {
            Self::fit_model_observation(observation)?
        };
        let (mut observation, challenged) =
            Self::settle_human_challenge(state, worker, browser, observation).await?;
        if challenged {
            state.observation = Some(observation);
            return Ok(());
        }
        if state.task.reads_results_page() {
            return Box::pin(Self::read_results_page(state, worker, browser, observation)).await;
        }
        if state.task.reads_app_view() {
            if let Some(look) = Box::pin(Self::read_app_rows(state, worker, browser)).await? {
                state.observation = Some(look);
                return Ok(());
            }
        }
        // The model starts from an ordinary fitted look.
        if first_look && !whole_page {
            state.refresh_account(worker, browser)?;
            observation = Self::fit_model_observation(
                Box::pin(Self::observe(state, worker, browser)).await?,
            )?;
        }
        observation = Box::pin(Self::clear_offers(state, worker, browser, observation)).await?;
        let mut captured_at = SemanticCaptureInstant::from_millis(
            state
                .journal_mut()?
                .clock
                .now()
                .map_err(|_| crate::contract!())?
                .millis(),
        );
        let mut progress = state.task_progress(&observation)?;
        if progress == AgentWorkTaskProgress::Complete {
            state.observation = Some(observation);
            return Ok(());
        }
        let direct = state.task.prefers_direct_extraction()
            && state.navigation_length() == 0
            && state
                .extraction_schema
                .as_ref()
                .is_some_and(|schema| schema.is_whole_page_findings() || schema.is_single_record())
            && !zephium_agentic::semantic_observation_reads_sparse(&observation)
            && observation
                .frames()
                .iter()
                .flat_map(|frame| frame.nodes())
                .all(|node| {
                    node.sensitivity() == SemanticSensitivity::Public
                        && node.role() != SemanticRole::Dialog
                });
        if direct {
            Self::finish_owned_extraction(state, worker, browser, &observation, captured_at, false)
                .await?;
            state.observation = Some(observation);
            return Ok(());
        }
        let (current, at, next_progress, challenged) =
            Self::run_decision_actions(state, worker, browser, observation, captured_at, progress)
                .await?;
        observation = current;
        captured_at = at;
        progress = next_progress;
        if challenged || progress == AgentWorkTaskProgress::Complete {
            state.observation = Some(observation);
            return Ok(());
        }
        // Fit the exact current bounded whole-page capture. Replacing it with
        // another compact initial look loses content beneath landmark anchors
        // and leaves a title-only extraction after a legitimate human resume.
        if whole_page {
            state.refresh_account(worker, browser)?;
            observation = Self::fit_model_observation(observation)?;
            // A banner or offer the page put up after its first look (a
            // consent dialog its scripts add late) is cleared the same way.
            observation = Box::pin(Self::clear_offers(state, worker, browser, observation)).await?;
            captured_at = SemanticCaptureInstant::from_millis(
                state
                    .journal_mut()?
                    .clock
                    .now()
                    .map_err(|_| crate::contract!())?
                    .millis(),
            );
            progress = state.task_progress(&observation)?;
        }
        state.refresh_account(worker, browser)?;
        let action_authority = state
            .session
            .as_ref()
            .ok_or_else(|| crate::contract!())?
            .config
            .permits_tool(AgentBrowserToolKind::Act)
            .then(|| state.action_authority(&observation))
            .transpose()?;
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let mut turn: AgentBrowserProviderTurn = Self::provider(
            &mut state.native,
            worker,
            browser,
            session.cancellation.clone(),
            session.start_initial_with_action_authority(&observation, action_authority.as_ref()),
        )
        .await?;
        if turn.turn.proposal().kind() == AgentBrowserToolKind::ShowForHuman
            && !Self::sign_in_optional(state, &turn, &observation)
            && !Self::own_decision(state, &turn, &observation)
        {
            Self::accept_model_human_request(state, turn, &observation)?;
            state.observation = Some(observation);
            return Ok(());
        }
        if state.extraction_schema.is_some()
            && !state.has_navigation()
            && !state.actions_before_extraction
            && !state.subtree_extraction
            && !state.baseline_read
        {
            Self::extract_current(state, worker, browser, turn, &observation, captured_at).await?;
            state.observation = Some(observation);
            return Ok(());
        }
        let mut frames = observation
            .frames()
            .iter()
            .map(|snapshot| snapshot.frame().clone())
            .collect::<Vec<_>>();
        // Exact rejected operation/target/baseline keys, content-free so no
        // fill value enters controller state. A repeat is told to change
        // approach; it is not the end of the page by itself.
        let mut action_refusals = Vec::<AgentProviderActionRefusalKey>::new();
        let mut no_progress = std::mem::take(&mut state.no_progress);
        // A control a single-page app re-renders comes back under a new
        // reference: counted by a digest of its role and name, a second
        // refusal tells the model to change approach.
        let mut refused_controls = Vec::<(u64, u8)>::new();
        // Refusals in a row with nothing done between them. Only a long run of
        // them ends the page: every refusal reaches the model as feedback.
        let mut fruitless = Fruitless::default();
        // The last refused proposal's exact key and how often in a row the
        // model proposed it: a third time, the model is shown the page anew.
        let mut last_refused: Option<(AgentProviderActionRefusalKey, u8)> = None;
        // Looks refused in a row: from the third, the host takes one itself.
        let mut refused_looks = 0u8;
        let mut compound_form_offered = false;
        loop {
            state.check_task_contract()?;
            // Deliver acknowledged audit batches while idle so long runs keep
            // their fixed pending-event bound and leave room for terminal debt.
            if state.journal_mut()?.audit.status().pending() >= 32 {
                Self::drain_audit(state, worker, browser, None).await?;
            }
            // The last decision was advertised as Extract-only. Independently
            // enforce that narrowing before any native capture, navigation or
            // local inspection can consume the reserved mapping call.
            let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
            if state.requires_decision_budget()
                && (session.turns.saturating_add(1) >= session.max_model_calls
                    || session
                        .policy
                        .remaining_operations(session.lease.lease())
                        .map_err(|_| {
                            AgentWorkFailure::Browser(AgentBrowserProviderError::Authority)
                        })?
                        <= 1)
                && turn.turn.proposal().kind() != AgentBrowserToolKind::Extract
                && !(state.human_request
                    && turn.turn.proposal().kind() == AgentBrowserToolKind::ShowForHuman)
            {
                return Err(AgentWorkFailure::Browser(
                    AgentBrowserProviderError::TurnLimit,
                ));
            }
            // A page that loaded another address of its own site by itself (a
            // bot check that cleared, a timed refresh, a script redirect) is
            // followed as this run's own navigation before the model works on
            // the document that page left.
            if matches!(
                turn.turn.proposal().kind(),
                AgentBrowserToolKind::Act
                    | AgentBrowserToolKind::Snapshot
                    | AgentBrowserToolKind::Locate
                    | AgentBrowserToolKind::Read
                    | AgentBrowserToolKind::Wait
                    | AgentBrowserToolKind::Screenshot
            ) {
                if let Some(target) = Self::page_load(state) {
                    let continuation = turn.into_tool_turn().into_parts().1;
                    let next = Self::navigate_current(
                        state,
                        worker,
                        browser,
                        navigation::NavigationStart::Follow(Box::new((continuation, target))),
                        &observation,
                        captured_at,
                        progress,
                    )
                    .await?;
                    fruitless.progressed();
                    let next_turn;
                    (observation, captured_at, progress, next_turn) = next;
                    let Some(next_turn) = next_turn else {
                        state.observation = Some(observation);
                        return Ok(());
                    };
                    turn = next_turn;
                    frames.clear();
                    frames.extend(
                        observation
                            .frames()
                            .iter()
                            .map(|snapshot| snapshot.frame().clone()),
                    );
                    continue;
                }
            }
            if !compound_form_offered
                && state.compound_forms
                && state
                    .navigation_discovery
                    .as_ref()
                    .is_some_and(AgentNavigationDiscovery::is_site_session)
                && Self::can_batch_search_fill(turn.turn.proposal(), &observation)
            {
                compound_form_offered = true;
                state.native.check_control(worker, browser)?;
                state.refresh_account(worker, browser)?;
                let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                let refusal = turn
                    .into_tool_turn()
                    .into_parts()
                    .1
                    .prefer_compound_form(&observation, &session.config)
                    .map_err(|_| crate::contract!())?;
                turn = Self::provider(
                    &mut state.native,
                    worker,
                    browser,
                    session.cancellation.clone(),
                    session.continue_after_scope_refusal(refusal, &observation),
                )
                .await?;
                continue;
            }
            let unoffered_look = turn.turn.proposal().kind() == AgentBrowserToolKind::Snapshot
                && state.look_run.withheld();
            if turn.turn.proposal().kind() == AgentBrowserToolKind::Snapshot {
                state.looked();
            }
            let progress_key = turn.turn.proposal().progress_fingerprint(&observation);
            if no_progress.refuses(progress_key) {
                state.native.check_control(worker, browser)?;
                state.refresh_account(worker, browser)?;
                state
                    .journal_mut()?
                    .emit(AgentWorkEventKind::InspectionRefused)?;
                fruitless.refused(state)?;
                let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                let refusal = turn
                    .into_tool_turn()
                    .into_parts()
                    .1
                    .refuse_no_progress(&observation, &session.config)
                    .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
                turn = Self::provider(
                    &mut state.native,
                    worker,
                    browser,
                    session.cancellation.clone(),
                    session.continue_after_scope_refusal(refusal, &observation),
                )
                .await?;
                continue;
            }
            let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
            if turn.turn.proposal().kind() == AgentBrowserToolKind::Snapshot {
                if session.turns.saturating_add(2) > session.max_model_calls
                    || session
                        .policy
                        .remaining_operations(session.lease.lease())
                        .map_err(|_| {
                            AgentWorkFailure::Browser(AgentBrowserProviderError::Authority)
                        })?
                        < 2
                {
                    return Err(AgentWorkFailure::Browser(
                        AgentBrowserProviderError::TurnLimit,
                    ));
                }
                let whole = whole_look(turn.turn.proposal(), &observation);
                // Planners swung between the whole page and itself for many
                // calls (sidebar-heavy listings): a third whole look in a
                // row with nothing done between is refused unrun, once per
                // streak, and pointed at narrower looks, extract or acting.
                if unoffered_look || (whole && state.look_streak >= 2) {
                    state.look_streak = 0;
                    state.native.check_control(worker, browser)?;
                    state.refresh_account(worker, browser)?;
                    state.journal_mut()?.emit(AgentWorkEventKind::ToolProposed(
                        AgentBrowserToolKind::Snapshot,
                    ))?;
                    state
                        .journal_mut()?
                        .emit(AgentWorkEventKind::InspectionRefused)?;
                    fruitless.refused(state)?;
                    let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                    let continuation = turn.into_tool_turn().into_parts().1;
                    let refusal = if unoffered_look {
                        continuation.refuse_withheld_look(&observation, &session.config)
                    } else {
                        continuation.refuse_whole_look(&observation, &session.config)
                    }
                    .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
                    turn = Self::provider(
                        &mut state.native,
                        worker,
                        browser,
                        session.cancellation.clone(),
                        session.continue_after_scope_refusal(refusal, &observation),
                    )
                    .await?;
                    continue;
                }
                let resolution = turn
                    .into_tool_turn()
                    .into_parts()
                    .1
                    .resolve_observation(&observation, &session.config)
                    .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
                let checkpoint = match resolution {
                    AgentProviderObservationResolution::Capture(checkpoint) => {
                        let checkpoint = *checkpoint;
                        if let Some(schema) = &state.extraction_schema {
                            let session =
                                state.session.as_ref().ok_or_else(|| crate::contract!())?;
                            session.check_live().map_err(AgentWorkFailure::Browser)?;
                            let read = read_semantic_observation_for_schema(
                                &observation,
                                SemanticReadAuthority::Acknowledged(checkpoint.baseline()),
                                captured_at,
                                SemanticReadSensitivityLimit::PublicOnly,
                                schema.extraction_read_budget(),
                                schema,
                            )
                            .map_err(|error| {
                                AgentWorkFailure::Browser(AgentBrowserProviderError::Read(error))
                            })?;
                            state
                                .retained_read_evidence
                                .retain(&read, checkpoint.baseline())
                                .map_err(|error| {
                                    AgentWorkFailure::Browser(AgentBrowserProviderError::Read(
                                        error,
                                    ))
                                })?;
                        }
                        checkpoint
                    }
                    AgentProviderObservationResolution::Refused(refusal) => {
                        state.native.check_control(worker, browser)?;
                        state.refresh_account(worker, browser)?;
                        state.journal_mut()?.emit(AgentWorkEventKind::ToolProposed(
                            AgentBrowserToolKind::Snapshot,
                        ))?;
                        state
                            .journal_mut()?
                            .emit(AgentWorkEventKind::InspectionRefused)?;
                        fruitless.refused(state)?;
                        refused_looks = refused_looks.saturating_add(1);
                        let refusal = if refused_looks >= 3 && state.progressive_observation {
                            let session =
                                state.session.as_ref().ok_or_else(|| crate::contract!())?;
                            match refusal.into_refresh(&observation, &session.config) {
                                Ok(checkpoint) => {
                                    refused_looks = 0;
                                    // A page still filling in gets a moment.
                                    let wake = Instant::now() + Duration::from_millis(1500);
                                    if wake < state.native.deadline {
                                        tokio::select! {
                                            biased;
                                            event = state.native.next_event(worker, browser) => {
                                                state.native.retain(event?)?;
                                                return Err(AgentWorkFailure::Mailbox);
                                            }
                                            () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
                                        }
                                    }
                                    let next = Self::inspect_current(
                                        state,
                                        worker,
                                        browser,
                                        checkpoint,
                                        &observation,
                                    )
                                    .await?;
                                    observation = next.0;
                                    captured_at = next.1;
                                    progress = next.2;
                                    let Some(next_turn) = next.3 else {
                                        state.observation = Some(observation);
                                        return Ok(());
                                    };
                                    turn = next_turn;
                                    frames.clear();
                                    frames.extend(
                                        observation
                                            .frames()
                                            .iter()
                                            .map(|snapshot| snapshot.frame().clone()),
                                    );
                                    continue;
                                }
                                Err(refusal) => refusal,
                            }
                        } else {
                            refusal
                        };
                        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                        turn = Self::provider(
                            &mut state.native,
                            worker,
                            browser,
                            session.cancellation.clone(),
                            session.continue_after_scope_refusal(*refusal, &observation),
                        )
                        .await?;
                        continue;
                    }
                };
                refused_looks = 0;
                // A narrower look between them breaks the streak: the whole
                // page is then the way back from a section.
                state.look_streak = if whole {
                    state.look_streak.saturating_add(1)
                } else {
                    0
                };
                no_progress.remember(progress_key);
                let next =
                    Self::inspect_current(state, worker, browser, checkpoint, &observation).await?;
                observation = next.0;
                captured_at = next.1;
                progress = next.2;
                let Some(next_turn) = next.3 else {
                    state.observation = Some(observation);
                    return Ok(());
                };
                turn = next_turn;
                frames = observation
                    .frames()
                    .iter()
                    .map(|snapshot| snapshot.frame().clone())
                    .collect();
                continue;
            }
            if matches!(
                turn.turn.proposal().kind(),
                AgentBrowserToolKind::Locate | AgentBrowserToolKind::Read
            ) {
                let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                if session.turns >= session.max_model_calls {
                    return Err(AgentWorkFailure::Browser(
                        AgentBrowserProviderError::TurnLimit,
                    ));
                }
                state.refresh_account(worker, browser)?;
                let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                no_progress.remember(progress_key);
                turn = Self::provider(
                    &mut state.native,
                    worker,
                    browser,
                    session.cancellation.clone(),
                    session.continue_inspection(
                        turn.into_tool_turn(),
                        &observation,
                        &frames,
                        Some(captured_at),
                    ),
                )
                .await?;
                continue;
            }
            if turn.turn.proposal().kind() == AgentBrowserToolKind::Screenshot {
                state.journal_mut()?.emit(AgentWorkEventKind::ToolProposed(
                    AgentBrowserToolKind::Screenshot,
                ))?;
                if Self::screenshot_possible(state, &turn, &observation)? {
                    match Self::screenshot_current(state, worker, browser, turn, &observation)
                        .await?
                    {
                        Ok(Some(next)) => {
                            no_progress.pending = None;
                            turn = next;
                            continue;
                        }
                        Ok(None) => {
                            state.observation = Some(observation);
                            return Ok(());
                        }
                        Err(failed) => {
                            let failure;
                            (turn, failure) = *failed;
                            let page = state.native.retained.as_ref().map_or(0, |browser| {
                                navigation::address_digest(&navigation::page_address(
                                    browser.binding().document(),
                                ))
                            });
                            state.setbacks.count(Setback::Screenshot, page, failure)?;
                            fruitless.refused(state)?;
                        }
                    }
                }
                // A picture refused for this look, or one whose capture
                // failed, goes back as a fresh text look rather than ending
                // the page.
                let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                let checkpoint = turn
                    .into_tool_turn()
                    .into_parts()
                    .1
                    .into_screenshot_refresh(&observation, &session.config)
                    .map_err(|_| {
                        AgentWorkFailure::Browser(AgentBrowserProviderError::UnsupportedTool(
                            AgentBrowserToolKind::Screenshot,
                        ))
                    })?;
                let next =
                    Self::inspect_current(state, worker, browser, checkpoint, &observation).await?;
                observation = next.0;
                captured_at = next.1;
                progress = next.2;
                let Some(next_turn) = next.3 else {
                    state.observation = Some(observation);
                    return Ok(());
                };
                turn = next_turn;
                frames.clear();
                frames.extend(
                    observation
                        .frames()
                        .iter()
                        .map(|snapshot| snapshot.frame().clone()),
                );
                continue;
            }
            if turn.turn.proposal().kind() == AgentBrowserToolKind::ShowForHuman {
                let sign_in = Self::sign_in_optional(state, &turn, &observation);
                let unfinished = !sign_in && Self::unfinished_page(state, &turn, &observation);
                if sign_in || unfinished || Self::own_decision(state, &turn, &observation) {
                    state.sign_in_refusals = state.sign_in_refusals.saturating_add(1);
                    let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                    let continuation = turn.into_tool_turn().into_parts().1;
                    let checkpoint = if sign_in {
                        continuation.into_optional_sign_in_refresh(&observation, &session.config)
                    } else if unfinished {
                        continuation.into_unfinished_page_refresh(&observation, &session.config)
                    } else {
                        continuation.into_own_decision_refresh(&observation, &session.config)
                    }
                    .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
                    fruitless.refused(state)?;
                    if unfinished {
                        let wake =
                            Instant::now() + Duration::from_millis(UNFINISHED_PAGE_WAIT_MILLIS);
                        if wake < state.native.deadline {
                            tokio::select! {
                                biased;
                                event = state.native.next_event(worker, browser) => {
                                    state.native.retain(event?)?;
                                    return Err(AgentWorkFailure::Mailbox);
                                }
                                () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
                            }
                        }
                    }
                    let next =
                        Self::inspect_current(state, worker, browser, checkpoint, &observation)
                            .await?;
                    observation = next.0;
                    captured_at = next.1;
                    progress = next.2;
                    let Some(next_turn) = next.3 else {
                        state.observation = Some(observation);
                        return Ok(());
                    };
                    turn = next_turn;
                    frames.clear();
                    frames.extend(
                        observation
                            .frames()
                            .iter()
                            .map(|snapshot| snapshot.frame().clone()),
                    );
                    continue;
                }
                Self::accept_model_human_request(state, turn, &observation)?;
                state.observation = Some(observation);
                return Ok(());
            }
            if turn.turn.proposal().kind() == AgentBrowserToolKind::Wait
                && !matches!(observation.request().scope(), SemanticScope::Initial)
            {
                // A wait compares whole looks of the page; after a look of
                // one section (Skyscanner's results loading) it ended the
                // page. It is answered with a fresh whole look a moment later.
                let pause = match turn.turn.proposal() {
                    AgentBrowserToolProposal::Wait { timeout, .. } => {
                        Duration::from_millis(u64::from(timeout.millis()).min(3000))
                    }
                    _ => return Err(crate::contract!()),
                };
                let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                let checkpoint = turn
                    .into_tool_turn()
                    .into_parts()
                    .1
                    .into_wait_refresh(&observation, &session.config)
                    .map_err(|_| AgentWorkFailure::Browser(AgentBrowserProviderError::Authority))?;
                let wake = Instant::now() + pause;
                if wake < state.native.deadline {
                    tokio::select! {
                        biased;
                        event = state.native.next_event(worker, browser) => {
                            state.native.retain(event?)?;
                            return Err(AgentWorkFailure::Mailbox);
                        }
                        () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
                    }
                }
                let next =
                    Self::inspect_current(state, worker, browser, checkpoint, &observation).await?;
                observation = next.0;
                captured_at = next.1;
                progress = next.2;
                let Some(next_turn) = next.3 else {
                    state.observation = Some(observation);
                    return Ok(());
                };
                turn = next_turn;
                frames.clear();
                frames.extend(
                    observation
                        .frames()
                        .iter()
                        .map(|snapshot| snapshot.frame().clone()),
                );
                continue;
            }
            if turn.turn.proposal().kind() == AgentBrowserToolKind::Wait {
                let next = match Self::wait_current(state, worker, browser, turn, observation)
                    .await?
                {
                    Ok(next) => next,
                    // The page moved under the wait (a results page that
                    // replaced its document while it loaded): the wait is
                    // answered with a fresh whole look, as after a section look.
                    Err(unwaited) => {
                        let (unwaited, original) = *unwaited;
                        let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                        let checkpoint = unwaited
                            .into_tool_turn()
                            .into_parts()
                            .1
                            .into_wait_refresh(&original, &session.config)
                            .map_err(|_| {
                                AgentWorkFailure::Browser(AgentBrowserProviderError::Authority)
                            })?;
                        let wake = Instant::now() + Duration::from_millis(1_500);
                        if wake < state.native.deadline {
                            tokio::select! {
                                biased;
                                event = state.native.next_event(worker, browser) => {
                                    state.native.retain(event?)?;
                                    return Err(AgentWorkFailure::Mailbox);
                                }
                                () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
                            }
                        }
                        let next =
                            Self::inspect_current(state, worker, browser, checkpoint, &original)
                                .await?;
                        observation = next.0;
                        captured_at = next.1;
                        progress = next.2;
                        let Some(next_turn) = next.3 else {
                            state.observation = Some(observation);
                            return Ok(());
                        };
                        turn = next_turn;
                        frames.clear();
                        frames.extend(
                            observation
                                .frames()
                                .iter()
                                .map(|snapshot| snapshot.frame().clone()),
                        );
                        continue;
                    }
                };
                observation = next.0;
                captured_at = next.1;
                turn = next.2;
                // The task reads the waited-for page like any fresh one, so a
                // later extraction binds to it.
                progress = state.task_progress(&observation)?;
                frames.clear();
                frames.extend(
                    observation
                        .frames()
                        .iter()
                        .map(|snapshot| snapshot.frame().clone()),
                );
                continue;
            }
            let step = turn;
            // A model can miscopy a public link. Refuse before policy admission,
            // retaining the exact observation/call and the original budgets.
            // Broken authority and out-of-scope targets still fail closed.
            let refused_navigation = match step.turn.proposal() {
                AgentBrowserToolProposal::Navigate(target) => {
                    Self::navigation_refusal(state, target, &observation)
                }
                _ => None,
            };
            if let Some(reason) = refused_navigation {
                state.native.check_control(worker, browser)?;
                state.refresh_account(worker, browser)?;
                state.journal_mut()?.emit(AgentWorkEventKind::ToolProposed(
                    AgentBrowserToolKind::Navigate,
                ))?;
                state
                    .journal_mut()?
                    .emit(AgentWorkEventKind::NavigationRefused(reason))?;
                fruitless.refused(state)?;
                let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                if session.turns.saturating_add(2) > session.max_model_calls {
                    return Err(AgentWorkFailure::Browser(
                        AgentBrowserProviderError::TurnLimit,
                    ));
                }
                let refusal = step
                    .into_tool_turn()
                    .into_parts()
                    .1
                    .refuse_navigation(&observation, &session.config, reason)
                    .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
                turn = Self::provider(
                    &mut state.native,
                    worker,
                    browser,
                    session.cancellation.clone(),
                    session.continue_after_navigation_refusal(refusal, &observation),
                )
                .await?;
                continue;
            }
            if matches!(
                step.turn.proposal().kind(),
                AgentBrowserToolKind::Navigate | AgentBrowserToolKind::Back
            ) {
                if state.navigation_discovery.is_none() {
                    state.retained_read_evidence.clear();
                }
                let next = Self::navigate_current(
                    state,
                    worker,
                    browser,
                    navigation::NavigationStart::Model(Box::new(step)),
                    &observation,
                    captured_at,
                    progress,
                )
                .await?;
                fruitless.progressed();
                observation = next.0;
                captured_at = next.1;
                progress = next.2;
                let Some(next_turn) = next.3 else {
                    state.observation = Some(observation);
                    return Ok(());
                };
                turn = next_turn;
                frames.clear();
                frames.extend(
                    observation
                        .frames()
                        .iter()
                        .map(|snapshot| snapshot.frame().clone()),
                );
                continue;
            }
            if step.turn.proposal().kind() == AgentBrowserToolKind::Extract
                && state.actions_before_extraction
                && state.loading_extract_waits < MAX_LOADING_EXTRACT_WAITS
                && (has_dominant_loading_placeholders(&observation)
                    || announces_loading(&observation))
            {
                state.loading_extract_waits = state.loading_extract_waits.saturating_add(1);
                let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                let checkpoint = step
                    .into_tool_turn()
                    .into_parts()
                    .1
                    .into_loading_extract_refresh(&observation, &session.config)
                    .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
                let wake = Instant::now() + Duration::from_millis(LOADING_EXTRACT_PAUSE_MILLIS);
                if wake < state.native.deadline {
                    tokio::select! {
                        biased;
                        event = state.native.next_event(worker, browser) => {
                            state.native.retain(event?)?;
                            return Err(AgentWorkFailure::Mailbox);
                        }
                        () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
                    }
                }
                let next =
                    Self::inspect_current(state, worker, browser, checkpoint, &observation).await?;
                observation = next.0;
                captured_at = next.1;
                progress = next.2;
                let Some(next_turn) = next.3 else {
                    state.observation = Some(observation);
                    return Ok(());
                };
                turn = next_turn;
                frames.clear();
                frames.extend(
                    observation
                        .frames()
                        .iter()
                        .map(|snapshot| snapshot.frame().clone()),
                );
                continue;
            }
            if step.turn.proposal().kind() == AgentBrowserToolKind::FillForm {
                no_progress.remember(progress_key);
                if !state.compound_forms || !state.progressive_observation {
                    return Err(crate::contract!());
                }
                let prepared = Box::pin(Self::prepare_compound_form(
                    state,
                    worker,
                    browser,
                    step,
                    &observation,
                ))
                .await?;
                let checkpoint = match prepared {
                    compound_form::PreparedForm::Refresh(checkpoint) => {
                        state.moved_on();
                        *checkpoint
                    }
                    compound_form::PreparedForm::Follow(follow) => {
                        state.moved_on();
                        let next = Self::navigate_current(
                            state,
                            worker,
                            browser,
                            navigation::NavigationStart::Follow(follow),
                            &observation,
                            captured_at,
                            progress,
                        )
                        .await?;
                        (observation, captured_at, progress, turn) = match next {
                            (observation, at, progress, Some(turn)) => {
                                (observation, at, progress, turn)
                            }
                            (observation, _, _, None) => {
                                state.observation = Some(observation);
                                return Ok(());
                            }
                        };
                        frames.clear();
                        frames.extend(
                            observation
                                .frames()
                                .iter()
                                .map(|snapshot| snapshot.frame().clone()),
                        );
                        continue;
                    }
                    compound_form::PreparedForm::Ready(current) => {
                        let captured = SemanticCaptureInstant::from_millis(
                            state
                                .journal_mut()?
                                .clock
                                .now()
                                .map_err(|_| crate::contract!())?
                                .millis(),
                        );
                        Self::finish_owned_extraction(
                            state, worker, browser, &current, captured, true,
                        )
                        .await?;
                        state.observation = Some(*current);
                        return Ok(());
                    }
                };
                let next =
                    Self::inspect_current(state, worker, browser, checkpoint, &observation).await?;
                observation = next.0;
                captured_at = next.1;
                progress = next.2;
                let Some(next_turn) = next.3 else {
                    state.observation = Some(observation);
                    return Ok(());
                };
                turn = next_turn;
                frames.clear();
                frames.extend(
                    observation
                        .frames()
                        .iter()
                        .map(|snapshot| snapshot.frame().clone()),
                );
                continue;
            }
            let navigation_incomplete = state.has_navigation() && !state.navigation_complete();
            let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
            let (proposal, assessment) = match step.turn.proposal().kind() {
                AgentBrowserToolKind::Extract => {
                    if !state.task.terminal_extraction_ready() {
                        return Err(AgentWorkFailure::TaskPhase {
                            expected: progress,
                            proposed: AgentBrowserToolKind::Extract,
                        });
                    }
                    if state.has_navigation()
                        && state.navigation_discovery.is_none()
                        && (navigation_incomplete
                            || progress != AgentWorkTaskProgress::ReadyForExtraction)
                    {
                        return Err(AgentWorkFailure::TaskPhase {
                            expected: progress,
                            proposed: AgentBrowserToolKind::Extract,
                        });
                    }
                    if state.actions_before_extraction
                        && state.navigation_discovery.is_none()
                        && progress != AgentWorkTaskProgress::ReadyForExtraction
                    {
                        return Err(AgentWorkFailure::TaskPhase {
                            expected: progress,
                            proposed: AgentBrowserToolKind::Extract,
                        });
                    }
                    let loaded = Self::extract_current(
                        state,
                        worker,
                        browser,
                        step,
                        &observation,
                        captured_at,
                    )
                    .await?;
                    state.observation = Some(loaded.unwrap_or(observation));
                    return Ok(());
                }
                AgentBrowserToolKind::Act => {
                    if state.extraction_schema.is_some() && !state.actions_before_extraction {
                        return Err(crate::contract!());
                    }
                    if progress == AgentWorkTaskProgress::ReadyForExtraction {
                        return Err(AgentWorkFailure::TaskPhase {
                            expected: progress,
                            proposed: AgentBrowserToolKind::Act,
                        });
                    }
                    let binding = session
                        .bind_action_turn(step, &observation, &frames)
                        .map_err(AgentWorkFailure::Browser)?;
                    // Reserve the effect, the next decision and terminal mapping
                    // before starting a mutation in an open objective. A page
                    // that cannot afford them, or an action the assignment
                    // does not permit, is refused to the model, not failed.
                    let admitted = match binding {
                        // Planners scrolled up and down for many calls instead
                        // of extracting: past three scrolls in a row the
                        // next is refused, said so, once per streak.
                        crate::action::AgentBrowserActionBinding::Prepared(proposal)
                            if state.scroll_streak >= 3
                                && proposal.action().kind() == SemanticActionKind::Scroll =>
                        {
                            state.scroll_streak = 0;
                            Err(proposal
                                .into_refusal(SemanticActionBindingError::AssignmentDenied)
                                .ok_or_else(|| crate::contract!())?
                                .scrolled_enough())
                        }
                        crate::action::AgentBrowserActionBinding::Prepared(proposal) => {
                            let starved = state.navigation_discovery.is_some()
                                && session
                                    .policy
                                    .remaining_operations(session.lease.lease())
                                    .map_err(|_| {
                                        AgentWorkFailure::Browser(
                                            AgentBrowserProviderError::Authority,
                                        )
                                    })?
                                    < 3;
                            if starved {
                                Err(proposal
                                    .into_refusal(SemanticActionBindingError::BudgetExhausted)
                                    .ok_or_else(|| crate::contract!())?)
                            } else {
                                match state.task.assess_observed(proposal.action(), &observation) {
                                    Ok(assessment) => Ok((*proposal, assessment)),
                                    Err(AgentWorkFailure::ActionDenied) => Err(proposal
                                        .into_refusal(SemanticActionBindingError::AssignmentDenied)
                                        .ok_or_else(|| crate::contract!())?),
                                    Err(AgentWorkFailure::EffectRequired(effect)) => Err(proposal
                                        .into_refusal(
                                            SemanticActionBindingError::TaskEffectMismatch(effect),
                                        )
                                        .ok_or_else(|| crate::contract!())?),
                                    Err(AgentWorkFailure::AlreadyDone) => Err(proposal
                                        .into_refusal(
                                            SemanticActionBindingError::OutcomeAlreadySatisfied,
                                        )
                                        .ok_or_else(|| crate::contract!())?),
                                    Err(AgentWorkFailure::OutcomeUnknown) => Err(proposal
                                        .into_refusal(SemanticActionBindingError::OutcomeUnknown)
                                        .ok_or_else(|| crate::contract!())?),
                                    Err(AgentWorkFailure::WrongRecipient) => Err(proposal
                                        .into_refusal(SemanticActionBindingError::WrongRecipient)
                                        .ok_or_else(|| crate::contract!())?),
                                    Err(failure) => return Err(failure),
                                }
                            }
                        }
                        crate::action::AgentBrowserActionBinding::Refused(refusal) => Err(*refusal),
                    };
                    match admitted {
                        Ok(admitted) => admitted,
                        Err(refusal) => {
                            if refusal.reason()
                                == SemanticActionBindingError::OutcomeAlreadySatisfied
                            {
                                no_progress.remember(progress_key);
                            }
                            state.native.check_control(worker, browser)?;
                            state.refresh_account(worker, browser)?;
                            state.journal_mut()?.emit(
                                AgentWorkEventKind::ActionProposalRefused(refusal.reason()),
                            )?;
                            // A step held for the person's decision stops here,
                            // the page kept as it is.
                            if state.human_request && Self::raise_human_wall(state, &observation)? {
                                state.observation = Some(observation);
                                return Ok(());
                            }
                            fruitless.refused(state)?;
                            let stuck = Self::refused_again(&mut last_refused, &refusal);
                            let refusal = Self::repeated_refusal(&mut action_refusals, refusal)?;
                            let refusal = Self::count_refused_control(
                                &mut refused_controls,
                                &mut state.covered_controls,
                                refusal,
                                &observation,
                                &frames,
                            )?;
                            let refusal = if stuck && state.progressive_observation {
                                let session =
                                    state.session.as_ref().ok_or_else(|| crate::contract!())?;
                                match refusal.stuck().into_refresh(&observation, &session.config) {
                                    Ok(checkpoint) => {
                                        let next = Self::inspect_current(
                                            state,
                                            worker,
                                            browser,
                                            checkpoint,
                                            &observation,
                                        )
                                        .await?;
                                        observation = next.0;
                                        captured_at = next.1;
                                        progress = next.2;
                                        let Some(next_turn) = next.3 else {
                                            state.observation = Some(observation);
                                            return Ok(());
                                        };
                                        turn = next_turn;
                                        frames.clear();
                                        frames.extend(
                                            observation
                                                .frames()
                                                .iter()
                                                .map(|snapshot| snapshot.frame().clone()),
                                        );
                                        continue;
                                    }
                                    Err(refusal) => *refusal,
                                }
                            } else {
                                refusal
                            };
                            let session =
                                state.session.as_mut().ok_or_else(|| crate::contract!())?;
                            turn = Self::provider(
                                &mut state.native,
                                worker,
                                browser,
                                session.cancellation.clone(),
                                session.continue_after_action_refusal(refusal, &observation),
                            )
                            .await?;
                            continue;
                        }
                    }
                }
                kind => {
                    return Err(AgentWorkFailure::Browser(
                        AgentBrowserProviderError::UnsupportedTool(kind),
                    ))
                }
            };
            no_progress.remember(progress_key);
            Self::retain_action_read_evidence(state, &proposal, &observation, captured_at)?;
            let link = Self::clicked_link(state, proposal.action(), &observation);
            let scrolled = proposal.action().kind() == SemanticActionKind::Scroll;
            let commits = !matches!(
                proposal.action().effect(),
                SemanticEffectClass::Read | SemanticEffectClass::LocalWrite
            );
            let typed = (proposal.action().kind() == SemanticActionKind::Fill)
                .then(|| {
                    proposal
                        .action()
                        .fill_text()
                        .map(|text| text.as_str().to_owned())
                })
                .flatten();
            let (current, current_at, transition) = match Self::execute_prepared_action(
                state,
                worker,
                browser,
                proposal,
                assessment,
                &observation,
            )
            .await
            {
                Ok(verified) => verified,
                Err(AgentWorkFailure::Browser(
                    error @ (AgentBrowserProviderError::ActionRejected(_)
                    | AgentBrowserProviderError::ActionUnverified),
                )) => {
                    let refusal = state
                        .session
                        .as_mut()
                        .and_then(|session| session.take_rejected_refusal())
                        .ok_or_else(|| crate::contract!())?;
                    if matches!(
                        refusal.reason(),
                        SemanticActionBindingError::Unverified
                            | SemanticActionBindingError::OutcomeUnknown
                    ) {
                        state.task.action_outcome_unknown();
                    } else {
                        state.task.action_not_run();
                    }
                    // The page ran its checks for a step it then refused
                    // (covered, changed): the context needs a fresh look
                    // before the next step, as after any step it ran.
                    if matches!(
                        error,
                        AgentBrowserProviderError::ActionRejected(
                            crate::AgentBrowserActionError::Failed(_)
                        )
                    ) {
                        state.refresh_account(worker, browser)?;
                        // A step that ran unverified may have left for
                        // another document still loading.
                        match Box::pin(Self::observe(state, worker, browser)).await {
                            Err(AgentWorkFailure::Observation(
                                SemanticRuntimePortFailure::NotReady
                                | SemanticRuntimePortFailure::Stale
                                | SemanticRuntimePortFailure::DocumentReplaced,
                            )) => {
                                Box::pin(Self::observe_initial_ready(state, worker, browser))
                                    .await?;
                            }
                            look => {
                                look?;
                            }
                        }
                    }
                    // A link whose click went nowhere (a new-window link, a
                    // covered or script-only one) is followed to its observed
                    // destination rather than handed back to the model.
                    let diverted = state
                        .follow
                        .take()
                        .and_then(|target| Self::followable(state, target));
                    if let Some(target) = diverted.or(link) {
                        let next = Self::navigate_current(
                            state,
                            worker,
                            browser,
                            navigation::NavigationStart::Follow(Box::new((
                                refusal.into_continuation(),
                                target,
                            ))),
                            &observation,
                            captured_at,
                            progress,
                        )
                        .await?;
                        fruitless.progressed();
                        let next_turn;
                        (observation, captured_at, progress, next_turn) = next;
                        let Some(next_turn) = next_turn else {
                            state.observation = Some(observation);
                            return Ok(());
                        };
                        turn = next_turn;
                        frames.clear();
                        frames.extend(
                            observation
                                .frames()
                                .iter()
                                .map(|snapshot| snapshot.frame().clone()),
                        );
                        continue;
                    }
                    state.native.check_control(worker, browser)?;
                    state.refresh_account(worker, browser)?;
                    state
                        .journal_mut()?
                        .emit(AgentWorkEventKind::ActionProposalRefused(refusal.reason()))?;
                    // A commit, or a step whose outcome is unknown, is held to
                    // the tighter bound of native failures.
                    if commits || refusal.reason() == SemanticActionBindingError::OutcomeUnknown {
                        state.setbacks.count(
                            Setback::Action(refusal.reason()),
                            Self::refused_control(&refusal, &observation, &frames)
                                .unwrap_or_default(),
                            AgentWorkFailure::Browser(error),
                        )?;
                    }
                    fruitless.refused(state)?;
                    let refusal = Self::repeated_refusal(&mut action_refusals, refusal)?;
                    let refusal = Self::count_refused_control(
                        &mut refused_controls,
                        &mut state.covered_controls,
                        refusal,
                        &observation,
                        &frames,
                    )?;
                    // The step reached the page: the model gets the page as
                    // it is now with the refusal, not a stale look and a
                    // request to ask for a new one.
                    let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                    let refusal = if state.progressive_observation {
                        match refusal.into_refresh(&observation, &session.config) {
                            Ok(checkpoint) => {
                                let next = Self::inspect_current(
                                    state,
                                    worker,
                                    browser,
                                    checkpoint,
                                    &observation,
                                )
                                .await?;
                                observation = next.0;
                                captured_at = next.1;
                                progress = next.2;
                                let Some(next_turn) = next.3 else {
                                    state.observation = Some(observation);
                                    return Ok(());
                                };
                                turn = next_turn;
                                frames.clear();
                                frames.extend(
                                    observation
                                        .frames()
                                        .iter()
                                        .map(|snapshot| snapshot.frame().clone()),
                                );
                                continue;
                            }
                            Err(refusal) => *refusal,
                        }
                    } else {
                        refusal
                    };
                    let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                    turn = Self::provider(
                        &mut state.native,
                        worker,
                        browser,
                        session.cancellation.clone(),
                        session.continue_after_action_refusal(refusal, &observation),
                    )
                    .await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            state.task.accept_verified_action(
                transition
                    .batch_result()
                    .ok_or_else(|| crate::contract!())?,
                &current,
            )?;
            state.look_streak = 0;
            state.moved_on();
            state.setbacks.progressed();
            state.scroll_streak = if scrolled {
                state.scroll_streak.saturating_add(1)
            } else {
                0
            };
            fruitless.progressed();
            if let Some(target) = state
                .follow
                .take()
                .and_then(|target| Self::followable(state, target))
            {
                let (continuation, _) =
                    transition.into_parts().ok_or_else(|| crate::contract!())?;
                let next = Self::navigate_current(
                    state,
                    worker,
                    browser,
                    navigation::NavigationStart::Follow(Box::new((continuation, target))),
                    &observation,
                    captured_at,
                    progress,
                )
                .await?;
                fruitless.progressed();
                let next_turn;
                (observation, captured_at, progress, next_turn) = next;
                let Some(next_turn) = next_turn else {
                    state.observation = Some(observation);
                    return Ok(());
                };
                turn = next_turn;
                frames.clear();
                frames.extend(
                    observation
                        .frames()
                        .iter()
                        .map(|snapshot| snapshot.frame().clone()),
                );
                continue;
            }
            // The one suggestion that clearly is what was typed is chosen
            // with the fill: planners typed JFK and scrolled on, leaving the
            // field unset.
            if let Some(choice) = typed
                .as_deref()
                .and_then(|typed| zephium_agentic::semantic_sole_choice(&current, typed))
                .filter(|_| state.progressive_observation)
            {
                let next = Box::pin(Self::commit_suggestion(
                    state,
                    worker,
                    browser,
                    transition,
                    &observation,
                    current,
                    choice,
                ))
                .await?;
                fruitless.progressed();
                let next_turn;
                (observation, captured_at, progress, next_turn) = next;
                let Some(next_turn) = next_turn else {
                    state.observation = Some(observation);
                    return Ok(());
                };
                turn = next_turn;
                frames.clear();
                frames.extend(
                    observation
                        .frames()
                        .iter()
                        .map(|snapshot| snapshot.frame().clone()),
                );
                continue;
            }
            observation = current;
            captured_at = current_at;
            progress = state.task_progress(&observation)?;
            if progress == AgentWorkTaskProgress::Complete
                || (state.human_request && Self::raise_human_wall(state, &observation)?)
            {
                state.observation = Some(observation);
                return Ok(());
            }
            let action_authority = state.action_authority(&observation)?;
            // Reads/locates retain this exact frame cohort. Reuse its bounded
            // storage; only an independently captured observation replaces it.
            frames.clear();
            frames.extend(
                observation
                    .frames()
                    .iter()
                    .map(|snapshot| snapshot.frame().clone()),
            );
            let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
            if session.turns >= session.max_model_calls {
                return Err(AgentWorkFailure::Browser(
                    AgentBrowserProviderError::TurnLimit,
                ));
            }
            state.refresh_account(worker, browser)?;
            let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
            turn = Self::provider(
                &mut state.native,
                worker,
                browser,
                session.cancellation.clone(),
                session.continue_after_verified_action_with_authority(
                    transition,
                    Some(&action_authority),
                ),
            )
            .await?;
        }
    }

    /// The field the planner typed into and has been told to run, on this
    /// look, once a scroll was refused for it.
    /// Whether the model proposed this exact refused action for the third
    /// time in a row.
    fn refused_again(
        last: &mut Option<(AgentProviderActionRefusalKey, u8)>,
        refusal: &AgentProviderActionRefusal,
    ) -> bool {
        let Some(key) = refusal.key() else {
            *last = None;
            return false;
        };
        let count = match last {
            Some((previous, count)) if *previous == key => count.saturating_add(1),
            _ => 1,
        };
        *last = Some((key, count));
        count >= 3
    }

    /// A diverted load the run can follow as its own navigation: a section
    /// anchor loads its page, a link to an anchor of the page it is on is
    /// only a scroll, and a load the site session would not admit is not
    /// followed.
    fn followable(
        state: &WorkState,
        target: ContextNavigationTarget,
    ) -> Option<ContextNavigationTarget> {
        let scope = state
            .navigation_discovery
            .as_ref()
            .filter(|scope| scope.is_site_session())?;
        let target = match target.as_url().fragment() {
            Some(fragment) if !fragment.starts_with(['/', '!']) => {
                let mut page = target.as_url().clone();
                page.set_fragment(None);
                ContextNavigationTarget::parse(&String::from(page)).ok()?
            }
            _ => target,
        };
        let here = state
            .native
            .retained
            .as_ref()
            .is_some_and(|browser| browser.binding().document() == &target);
        // A load the run's own ledger would refuse (a destination visited its
        // limit) is not followed: the page stays and the model goes on.
        let refused = state.session.as_ref().is_none_or(|session| {
            session
                .policy
                .navigation_ledger_refusal(session.lease.lease(), &target)
                .is_some()
        });
        (!here
            && !refused
            && scope.admits(&target)
            && zephium_agentic::WorkBrowserDocumentPolicy::SiteSession.admits_request(&target))
        .then_some(target)
    }

    /// A same-site load the page started by itself since the last look, when
    /// the run can still afford to follow it; otherwise it is dropped.
    fn page_load(state: &WorkState) -> Option<ContextNavigationTarget> {
        let scope = state
            .navigation_discovery
            .as_ref()
            .filter(|scope| scope.is_site_session())?;
        let target = Self::followable(state, state.follow.take()?)?;
        let session = state.session.as_ref()?;
        let headroom = usize::from(session.turns) + 2 <= usize::from(session.max_model_calls)
            && session
                .policy
                .remaining_operations(session.lease.lease())
                .is_ok_and(|operations| operations >= 3);
        (headroom && scope.admits(&target)).then_some(target)
    }

    /// The in-scope observed destination of a link this action clicks, for
    /// following when the click itself changes nothing.
    fn clicked_link(
        state: &WorkState,
        action: &SemanticPreparedAction,
        observation: &SemanticObservation,
    ) -> Option<ContextNavigationTarget> {
        let session = state.session.as_ref()?;
        let headroom = usize::from(session.turns) + 2 <= usize::from(session.max_model_calls)
            && session
                .policy
                .remaining_operations(session.lease.lease())
                .is_ok_and(|operations| operations >= 3);
        if action.kind() != SemanticActionKind::Click
            || !headroom
            || !state
                .navigation_discovery
                .as_ref()
                .is_some_and(AgentNavigationDiscovery::is_site_session)
        {
            return None;
        }
        let target = observation
            .frames()
            .iter()
            .flat_map(SemanticSnapshot::nodes)
            .find(|node| node.reference() == action.target_reference())
            .filter(|node| node.role() == SemanticRole::Link)?
            .link_destination()?
            .clone();
        Self::navigation_refusal(state, &target, observation)
            .is_none()
            .then_some(target)
    }

    /// The daily app the page's admitted document is on.
    fn daily_app(state: &WorkState) -> Option<zephium_agentic::DailyApp> {
        let browser = state.native.retained.as_ref()?;
        let document = browser.binding().document();
        zephium_agentic::DailyApp::of(document.as_url().host_str()?)
    }

    /// Reads an app's views as their records, with no model call: the view
    /// the task opens first (Slack's Activity, Linear's Inbox), each next one
    /// when a view lists nothing, and in each the page's look, else each of
    /// its landmarks opened whole in turn. None when no view held records,
    /// leaving the page planner.
    async fn read_app_rows(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<Option<SemanticObservation>, AgentWorkFailure> {
        let Some(schema) = state.extraction_schema.clone() else {
            return Ok(None);
        };
        state.refresh_account(worker, browser)?;
        let mut look = Box::pin(Self::observe(state, worker, browser)).await?;
        let mut tried = false;
        for _ in 0..=MAX_APP_VIEWS {
            let last = match state.task.app_view(&look) {
                Some(target) => {
                    look =
                        Box::pin(Self::open_app_view(state, worker, browser, look, target)).await?;
                    false
                }
                // No view is left: the one shown answers, empty or not.
                None => tried,
            };
            if let Some(read) = Box::pin(Self::read_app_look(
                state, worker, browser, &schema, look, last,
            ))
            .await?
            {
                return Ok(Some(read));
            }
            if last {
                return Ok(None);
            }
            tried = true;
            state.refresh_account(worker, browser)?;
            look = Box::pin(Self::observe(state, worker, browser)).await?;
        }
        Ok(None)
    }

    /// A results page read as the task cites it: its results, or what stood
    /// in their way, as a findings list with no model call.
    async fn read_results_page(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        look: SemanticObservation,
    ) -> Result<(), AgentWorkFailure> {
        let schema = state
            .extraction_schema
            .clone()
            .ok_or_else(|| crate::contract!())?;
        let mut look = look;
        for _ in 0..RESULTS_DRAW_LOOKS {
            if state.task.results_drawn(&look) {
                break;
            }
            let wake = Instant::now() + Duration::from_millis(RESULTS_DRAW_LOOK_MILLIS);
            if wake >= state.native.deadline {
                break;
            }
            tokio::select! {
                biased;
                event = state.native.next_event(worker, browser) => {
                    state.native.retain(event?)?;
                    return Err(AgentWorkFailure::Mailbox);
                }
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
            }
            look = Box::pin(Self::observe(state, worker, browser)).await?;
        }
        let cited = state.task.results_page(&look);
        state.refresh_account(worker, browser)?;
        let captured_at = SemanticCaptureInstant::from_millis(
            state
                .journal_mut()?
                .clock
                .now()
                .map_err(|_| crate::contract!())?
                .millis(),
        );
        let account = state
            .session
            .as_ref()
            .ok_or_else(|| crate::contract!())?
            .account;
        let located =
            zephium_agentic::read_cited_rows(&look, account, captured_at, &schema, &cited)
                .ok_or_else(|| crate::contract!())?;
        state.task_progress(&look)?;
        state.journal_mut()?.emit(AgentWorkEventKind::RowRead {
            found: u8::try_from(located.rows_read()).unwrap_or(u8::MAX),
            cells: false,
            refused: None,
        })?;
        Box::pin(Self::finish_located_read(state, worker, browser, located)).await?;
        state.observation = Some(look);
        Ok(())
    }

    /// Presses a view's control as a read and waits for its list: a view
    /// that renders its rows after the press gets one more look.
    async fn open_app_view(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        look: SemanticObservation,
        target: SemanticReferenceId,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        let press = SemanticActionProposal::try_new(
            SemanticActionIntent::Click { target },
            SemanticEffectClass::Read,
            SemanticWaitCondition::MutationQuiet(
                SemanticMutationQuietPeriod::try_new(APP_VIEW_QUIET_MILLIS)
                    .map_err(|_| crate::contract!())?,
            ),
            SemanticVerification::PageChanged,
            SemanticSettleBudget::try_new(MIN_AGENT_BROWSER_SNAPSHOT_SETTLE_MILLIS)
                .map_err(|_| crate::contract!())?,
        )
        .map_err(|_| crate::contract!())?;
        state
            .journal_mut()?
            .emit(AgentWorkEventKind::AppViewOpened)?;
        let _ = Box::pin(Self::code_owned_read(state, worker, browser, &look, press)).await?;
        tokio::time::sleep(Duration::from_millis(APP_VIEW_RENDER_MILLIS)).await;
        state.refresh_account(worker, browser)?;
        Box::pin(Self::observe(state, worker, browser)).await
    }

    /// One view's records: its look, else each landmark opened whole. The
    /// last view is read as it stands, its empty-state line an answer.
    async fn read_app_look(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        schema: &SemanticExtractionSchema,
        fresh: SemanticObservation,
        last: bool,
    ) -> Result<Option<SemanticObservation>, AgentWorkFailure> {
        let app = Self::daily_app(state);
        let landmarks: Vec<SemanticReferenceId> = fresh
            .frames()
            .first()
            .filter(|_| !last)
            .map(|frame| {
                frame
                    .nodes()
                    .iter()
                    .filter(|node| node.role() == SemanticRole::Landmark)
                    .map(SemanticNode::reference)
                    .take(MAX_APP_LANDMARKS)
                    .collect()
            })
            .unwrap_or_default();
        let capability = state.observation_capability();
        let mut look = fresh;
        let mut next = landmarks.into_iter();
        loop {
            state.refresh_account(worker, browser)?;
            let captured_at = SemanticCaptureInstant::from_millis(
                state
                    .journal_mut()?
                    .clock
                    .now()
                    .map_err(|_| crate::contract!())?
                    .millis(),
            );
            let account = state
                .session
                .as_ref()
                .ok_or_else(|| crate::contract!())?
                .account;
            if let Some(located) =
                zephium_agentic::read_app_view(&look, account, captured_at, schema, app, last)
            {
                // The task judges the look its records come from.
                state.task_progress(&look)?;
                state.journal_mut()?.emit(AgentWorkEventKind::RowRead {
                    found: u8::try_from(located.rows_read()).unwrap_or(u8::MAX),
                    cells: false,
                    refused: None,
                })?;
                Box::pin(Self::finish_located_read(state, worker, browser, located)).await?;
                return Ok(Some(look));
            }
            let Some(landmark) = next.next() else {
                return Ok(None);
            };
            // An expansion starts from the look the page delivered last.
            let base = Box::pin(Self::observe(state, worker, browser)).await?;
            let Some((acknowledgement, _)) =
                SemanticObservationAcknowledgement::whole_page_scope(&base)
            else {
                return Ok(None);
            };
            state.native.check_control(worker, browser)?;
            state.journal_mut()?.emit(AgentWorkEventKind::Observing)?;
            look = match state
                .native
                .observe_retained_scope(
                    worker,
                    Some((
                        &base,
                        &acknowledgement,
                        landmark,
                        SemanticExpansionKind::Subtree,
                    )),
                    capability,
                )
                .await
            {
                Ok(look) => {
                    Self::capture_app_view(state, &look);
                    look
                }
                Err(AgentWorkFailure::InspectionAnchorLost) => base,
                Err(error) => return Err(error),
            };
        }
    }

    /// Clicks a consent banner's refusal as a read. A page that refuses the
    /// click or does not change keeps the model's ordinary first look.
    /// Refuses a cookie banner and closes a site's own offer on a page worked
    /// for the person, before the planner spends a call on either.
    async fn clear_offers(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        mut observation: SemanticObservation,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        // A cookie banner on a page worked for the person is refused by Rust
        // before any model sees the page. Rust presses on a whole look of the
        // page: the model's fitted look may shorten the banner's fields or cut
        // its buttons off, and a press needs them whole.
        if state.actions_before_extraction
            && state
                .navigation_discovery
                .as_ref()
                .is_some_and(AgentNavigationDiscovery::is_site_session)
            && (state.task.consent_dismissal(&observation).is_some()
                || state.task.consent_suspected(&observation))
        {
            state.refresh_account(worker, browser)?;
            let initial = Box::pin(Self::observe(state, worker, browser)).await?;
            let whole = Box::pin(Self::whole_page_capture(
                state,
                worker,
                browser,
                initial,
                SemanticExpansionKind::Subtree,
            ))
            .await?;
            if let Some(target) = state.task.consent_dismissal(&whole) {
                observation =
                    Box::pin(Self::dismiss_consent(state, worker, browser, whole, target)).await?;
            } else {
                // The looks taken to find the banner replaced the one held:
                // acting on that one is refused by the page's runtime.
                state.refresh_account(worker, browser)?;
                observation = Self::fit_model_observation(
                    Box::pin(Self::observe(state, worker, browser)).await?,
                )?;
            }
        }
        // So is a sign-in, newsletter or app offer the site laid over the
        // page with its own close control.
        if state.actions_before_extraction
            && state
                .navigation_discovery
                .as_ref()
                .is_some_and(AgentNavigationDiscovery::is_site_session)
        {
            if let Some(target) = state.task.overlay_dismissal(&observation) {
                observation = Box::pin(Self::close_overlay(
                    state,
                    worker,
                    browser,
                    observation,
                    target,
                ))
                .await?;
            }
        }
        Ok(observation)
    }

    async fn dismiss_consent(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        look: SemanticObservation,
        target: SemanticReferenceId,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        let press = SemanticActionProposal::try_new(
            SemanticActionIntent::Click { target },
            SemanticEffectClass::Read,
            SemanticWaitCondition::MutationQuiet(
                SemanticMutationQuietPeriod::try_new(100).map_err(|_| crate::contract!())?,
            ),
            SemanticVerification::PageChanged,
            SemanticSettleBudget::try_new(MIN_AGENT_BROWSER_SNAPSHOT_SETTLE_MILLIS)
                .map_err(|_| crate::contract!())?,
        )
        .map_err(|_| crate::contract!())?;
        state.consent_press = true;
        state.task.consent_pressing(true);
        let pressed = Box::pin(Self::code_owned_read(state, worker, browser, &look, press)).await;
        state.consent_press = false;
        state.task.consent_pressing(false);
        match pressed? {
            Some(after) => Self::fit_model_observation(after),
            None => {
                state.refresh_account(worker, browser)?;
                Self::fit_model_observation(Box::pin(Self::observe(state, worker, browser)).await?)
            }
        }
    }

    /// Clicks the suggestion a verified fill opened, then answers the fill
    /// with a fresh look taken after it.
    async fn commit_suggestion(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        transition: crate::AgentBrowserVerifiedTransition,
        proposed_on: &SemanticObservation,
        filled: SemanticObservation,
        choice: SemanticReferenceId,
    ) -> Result<
        (
            SemanticObservation,
            SemanticCaptureInstant,
            AgentWorkTaskProgress,
            Option<AgentBrowserProviderTurn>,
        ),
        AgentWorkFailure,
    > {
        let (continuation, verified) = transition.into_parts().ok_or_else(|| crate::contract!())?;
        // Only the probe build has another verified state.
        #[allow(irrefutable_let_patterns)]
        let crate::action::AgentBrowserVerifiedState::Accounted(result) = verified
        else {
            return Err(crate::contract!());
        };
        let continuation = continuation
            .with_verified_action_progress(&result)
            .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
        let click = SemanticActionProposal::try_new(
            SemanticActionIntent::Click { target: choice },
            SemanticEffectClass::Read,
            SemanticWaitCondition::MutationQuiet(
                SemanticMutationQuietPeriod::try_new(100).map_err(|_| crate::contract!())?,
            ),
            SemanticVerification::PageChanged,
            SemanticSettleBudget::try_new(MIN_AGENT_BROWSER_SNAPSHOT_SETTLE_MILLIS)
                .map_err(|_| crate::contract!())?,
        )
        .map_err(|_| crate::contract!())?;
        let chosen = Box::pin(Self::code_owned_read(
            state, worker, browser, &filled, click,
        ))
        .await?
        .is_some();
        let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
        let note = if chosen {
            "suggestion_chosen: your fill matched exactly one of the suggestions the field opened, and the host clicked it. This fresh look shows the field afterwards: check it shows your choice, then go on with the next field."
        } else {
            "suggestion_not_chosen: your fill matched one of the field's suggestions, but clicking it did not change the page. This is a fresh look: choose the suggestion yourself if it is still listed."
        };
        let checkpoint = continuation
            .into_post_action_refresh(proposed_on, &session.config, note)
            .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
        Self::inspect_current(state, worker, browser, checkpoint, proposed_on).await
    }

    async fn close_overlay(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        look: SemanticObservation,
        target: SemanticReferenceId,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        let close = SemanticActionProposal::try_new(
            SemanticActionIntent::Click { target },
            SemanticEffectClass::Read,
            SemanticWaitCondition::MutationQuiet(
                SemanticMutationQuietPeriod::try_new(100).map_err(|_| crate::contract!())?,
            ),
            SemanticVerification::PageChanged,
            SemanticSettleBudget::try_new(MIN_AGENT_BROWSER_SNAPSHOT_SETTLE_MILLIS)
                .map_err(|_| crate::contract!())?,
        )
        .map_err(|_| crate::contract!())?;
        match Box::pin(Self::code_owned_read(state, worker, browser, &look, close)).await? {
            Some(after) => Self::fit_model_observation(after),
            None => {
                state.refresh_account(worker, browser)?;
                Self::fit_model_observation(Box::pin(Self::observe(state, worker, browser)).await?)
            }
        }
    }

    /// One read step Rust chose on `observation`, assessed by the task like
    /// any other: the page after it, or None when it was refused or failed
    /// without effect. Any other failure ends the page as before.
    async fn code_owned_read(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        observation: &SemanticObservation,
        recipe: SemanticActionProposal,
    ) -> Result<Option<SemanticObservation>, AgentWorkFailure> {
        let frames = observation
            .frames()
            .iter()
            .map(|snapshot| snapshot.frame().clone())
            .collect::<Vec<_>>();
        // The task assesses only the look it last evaluated.
        if state.task_progress(observation)? == AgentWorkTaskProgress::Complete {
            #[cfg(feature = "probe-harness")]
            let _ = writeln!(std::io::stderr(), "owned-read: none=complete");
            return Ok(None);
        }
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let account = session.account;
        session
            .policy
            .admit_owned_observation(observation, account)
            .map_err(|_| crate::contract!())?;
        let batch =
            SemanticActionBatchId::new(session.next_action).ok_or_else(|| crate::contract!())?;
        let Ok(proposal) =
            crate::AgentBrowserActionProposal::bind_code_owned(recipe, observation, &frames, batch)
        else {
            #[cfg(feature = "probe-harness")]
            let _ = writeln!(std::io::stderr(), "owned-read: none=bind");
            return Ok(None);
        };
        let assessment = match state.task.assess_observed(proposal.action(), observation) {
            Ok(assessment) => assessment,
            Err(
                error @ (AgentWorkFailure::ActionDenied
                | AgentWorkFailure::EffectRequired(_)
                | AgentWorkFailure::AlreadyDone
                | AgentWorkFailure::OutcomeUnknown
                | AgentWorkFailure::WrongRecipient),
            ) => {
                #[cfg(feature = "probe-harness")]
                let _ = writeln!(std::io::stderr(), "owned-read: none=assess {error:?}");
                let _ = error;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let captured_at = SemanticCaptureInstant::from_millis(
            state
                .journal_mut()?
                .clock
                .now()
                .map_err(|_| crate::contract!())?
                .millis(),
        );
        Self::retain_action_read_evidence(state, &proposal, observation, captured_at)?;
        state
            .journal_mut()?
            .emit(AgentWorkEventKind::ToolProposed(AgentBrowserToolKind::Act))?;
        match Self::execute_prepared_action(
            state,
            worker,
            browser,
            proposal,
            assessment,
            observation,
        )
        .await
        {
            Ok((current, _, transition)) => {
                if !state.keep_follow {
                    let _ = state.follow.take();
                }
                if let Some(batch) = transition.batch_result() {
                    state.task.accept_verified_action(batch, &current)?;
                }
                Ok(Some(current))
            }
            Err(AgentWorkFailure::Browser(error)) => {
                let _ = state.follow.take();
                let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                let _ = session.take_rejected_refusal();
                if !session
                    .close_failed_owned_action()
                    .map_err(AgentWorkFailure::Browser)?
                {
                    return Err(AgentWorkFailure::Browser(error));
                }
                #[cfg(feature = "probe-harness")]
                let _ = writeln!(std::io::stderr(), "owned-read: none=browser {error:?}");
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// A page agent's request that the person sign in, on a page worked
    /// toward its goal that shows no sign-in form, is refused a couple of
    /// times: an optional account offer is no reason to hold the page.
    fn sign_in_optional(
        state: &WorkState,
        turn: &AgentBrowserProviderTurn,
        observation: &SemanticObservation,
    ) -> bool {
        matches!(
            turn.turn.proposal(),
            AgentBrowserToolProposal::ShowForHuman(AgentBrowserHumanReason::SignIn)
        ) && state.actions_before_extraction
            && state.sign_in_refusals < MAX_SIGN_IN_REFUSALS
            && state.task.sign_in_optional(observation)
    }

    /// A page agent's request for an interaction it cannot perform, made on
    /// a look of a page that had barely drawn (an app's shell before its
    /// view loads), is refused a couple of times with a later look.
    fn unfinished_page(
        state: &WorkState,
        turn: &AgentBrowserProviderTurn,
        observation: &SemanticObservation,
    ) -> bool {
        matches!(
            turn.turn.proposal(),
            AgentBrowserToolProposal::ShowForHuman(AgentBrowserHumanReason::UnsupportedInteraction)
        ) && state.sign_in_refusals < MAX_SIGN_IN_REFUSALS
            && observation
                .frames()
                .iter()
                .map(|frame| frame.nodes().len())
                .sum::<usize>()
                < UNFINISHED_PAGE_NODES
            && state.task.human_wall(observation).is_none()
    }

    /// A page agent's request for the person's decision while the page
    /// holds no step for them: the goal already says what to do, and a
    /// choice it leaves open (a variant, a fulfillment option, an
    /// added-to-cart sheet) is the agent's own, refused a couple of times.
    fn own_decision(
        state: &WorkState,
        turn: &AgentBrowserProviderTurn,
        observation: &SemanticObservation,
    ) -> bool {
        matches!(
            turn.turn.proposal(),
            AgentBrowserToolProposal::ShowForHuman(AgentBrowserHumanReason::UserDecision)
        ) && state.actions_before_extraction
            && state.sign_in_refusals < MAX_SIGN_IN_REFUSALS
            && state.task.human_wall(observation).is_none()
    }

    fn accept_model_human_request(
        state: &mut WorkState,
        turn: AgentBrowserProviderTurn,
        observation: &SemanticObservation,
    ) -> Result<(), AgentWorkFailure> {
        state.check_task_contract()?;
        if !state.human_request || state.model_human_request.is_some() {
            return Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::UnsupportedTool(AgentBrowserToolKind::ShowForHuman),
            ));
        }
        let (proposal, continuation) = turn.into_tool_turn().into_parts();
        let AgentBrowserToolProposal::ShowForHuman(reason) = proposal else {
            return Err(crate::contract!());
        };
        if !continuation.baseline().authenticates(observation) {
            return Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::Authority,
            ));
        }
        let request = AgentWorkHumanRequest {
            context: observation.request().context(),
            observation: observation.request().id(),
            generation: observation.request().generation(),
            reason,
            retained_resource: state
                .native
                .retained
                .as_ref()
                .map(|browser| browser.binding().lease().resource().identity()),
        };
        state
            .journal_mut()?
            .emit(AgentWorkEventKind::ModelRequestedHuman(reason))?;
        state.model_human_request = Some(request);
        Ok(())
    }

    /// Whether a viewport picture of this look may be taken at all.
    fn screenshot_possible(
        state: &mut WorkState,
        turn: &AgentBrowserProviderTurn,
        observation: &SemanticObservation,
    ) -> Result<bool, AgentWorkFailure> {
        if !state.viewport_screenshot
            || state.native.screenshot_pending.is_some()
            || state
                .native
                .retained
                .as_ref()
                .is_some_and(|browser| !browser.supports_screenshots())
        {
            #[cfg(feature = "probe-harness")]
            let _ = writeln!(std::io::stderr(), "browser-screenshot: stage=preflight capability_or_busy=true enabled={} pending={} content=redacted", state.viewport_screenshot, state.native.screenshot_pending.is_some());
            return Ok(false);
        }
        let now = state
            .session
            .as_mut()
            .ok_or_else(|| crate::contract!())?
            .policy_now()
            .map_err(AgentWorkFailure::Browser)?;
        let requested_at = SemanticCaptureInstant::from_millis(now.millis());
        let deadline = SemanticCaptureInstant::from_millis(
            now.millis()
                .checked_add(MAX_SEMANTIC_SCREENSHOT_CAPTURE_MILLIS)
                .ok_or_else(|| crate::contract!())?,
        );
        let prepared = prepare_semantic_screenshot(
            SemanticScreenshotRequestId::new(u64::MAX).ok_or_else(|| crate::contract!())?,
            observation,
            turn.turn.continuation().baseline(),
            requested_at,
            deadline,
            SemanticScreenshotBudget::STANDARD,
        );
        #[cfg(feature = "probe-harness")]
        if let Err(error) = &prepared {
            let _ = writeln!(
                std::io::stderr(),
                "browser-screenshot: stage=preflight reason={error:?} content=redacted"
            );
        }
        Ok(prepared.is_ok())
    }

    /// Answers a Screenshot turn with a picture of the page. A capture the
    /// page did not give (it changed under it, pictures are not allowed
    /// there) hands the turn back with why, for a fresh text look.
    #[allow(clippy::type_complexity)]
    async fn screenshot_current(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        turn: AgentBrowserProviderTurn,
        observation: &SemanticObservation,
    ) -> Result<
        Result<Option<AgentBrowserProviderTurn>, Box<(AgentBrowserProviderTurn, AgentWorkFailure)>>,
        AgentWorkFailure,
    > {
        state.check_task_contract()?;
        if !state.viewport_screenshot
            || state.native.screenshot_pending.is_some()
            || state
                .native
                .retained
                .as_ref()
                .is_some_and(|browser| !browser.supports_screenshots())
        {
            return Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::UnsupportedTool(AgentBrowserToolKind::Screenshot),
            ));
        }
        let screenshot = match Self::capture_screenshot(
            state,
            worker,
            browser,
            turn.turn.continuation().baseline(),
            observation,
            SemanticScreenshotBudget::STANDARD,
        )
        .await
        {
            Ok(Some(screenshot)) => screenshot,
            Ok(None) => return Ok(Ok(None)),
            Err(
                failure @ (AgentWorkFailure::Screenshot(_)
                | AgentWorkFailure::ScreenshotRequest(_)
                | AgentWorkFailure::Context
                | AgentWorkFailure::ContextLost
                | AgentWorkFailure::Browser(AgentBrowserProviderError::Authority)),
            ) => return Ok(Err(Box::new((turn, failure)))),
            Err(failure) => return Err(failure),
        };
        state.native.check_control(worker, browser)?;
        state.refresh_account(worker, browser)?;
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
        session
            .policy
            .admit_model_image(&screenshot, observation, session.account, now)
            .await
            .map_err(|_| AgentWorkFailure::Browser(AgentBrowserProviderError::Authority))?;
        Self::provider(
            &mut state.native,
            worker,
            browser,
            session.cancellation.clone(),
            session.continue_after_screenshot(turn.into_tool_turn(), screenshot, observation),
        )
        .await
        .map(|next| Ok(Some(next)))
    }

    /// One bounded viewport screenshot of `observation`'s page, bound to the
    /// look the model holds.
    async fn capture_screenshot(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        baseline: &SemanticObservationAcknowledgement,
        observation: &SemanticObservation,
        budget: SemanticScreenshotBudget,
    ) -> Result<Option<SemanticScreenshot>, AgentWorkFailure> {
        let now = state
            .session
            .as_mut()
            .ok_or_else(|| crate::contract!())?
            .policy_now()
            .map_err(AgentWorkFailure::Browser)?;
        let remaining = state
            .native
            .deadline
            .saturating_duration_since(Instant::now());
        let window_millis = u64::try_from(remaining.as_millis())
            .unwrap_or(u64::MAX)
            .min(MAX_SEMANTIC_SCREENSHOT_CAPTURE_MILLIS);
        if window_millis == 0 {
            return Err(AgentWorkFailure::Deadline);
        }
        let requested_at = SemanticCaptureInstant::from_millis(now.millis());
        let deadline = SemanticCaptureInstant::from_millis(
            now.millis()
                .checked_add(window_millis)
                .ok_or_else(|| crate::contract!())?,
        );
        let id = SemanticScreenshotRequestId::new(state.native.id()?)
            .ok_or_else(|| crate::contract!())?;
        let origin = observation
            .frames()
            .first()
            .ok_or_else(|| crate::contract!())?
            .frame()
            .origin()
            .as_url()
            .origin()
            .ascii_serialization();
        let plan = state
            .session
            .as_ref()
            .ok_or_else(|| crate::contract!())?
            .policy
            .model_image_plan(id, observation.request().context(), &origin, now);
        use zephium_agentic::SemanticModelImagePlan;
        match plan {
            SemanticModelImagePlan::Waiting => {
                Self::raise_image_question(state, observation)?;
                return Ok(None);
            }
            SemanticModelImagePlan::Unavailable => {
                return Err(AgentWorkFailure::Browser(
                    AgentBrowserProviderError::Authority,
                ))
            }
            _ => {}
        }
        let request =
            prepare_semantic_screenshot(id, observation, baseline, requested_at, deadline, budget)
                .map_err(AgentWorkFailure::ScreenshotRequest)?;
        let request = if plan == SemanticModelImagePlan::Public {
            request.require_public_page()
        } else {
            request
        };
        let (pending, native) = state
            .native
            .screenshots
            .begin(request)
            .map_err(|_| crate::contract!())?;
        state.native.screenshot_pending = Some(pending);
        let dispatch = if let Some(retained) = &mut state.native.retained {
            retained.dispatch_screenshot(native, worker.semantic_screenshot_completion(), now)
        } else {
            browser.capture_semantic_screenshot(native, worker.semantic_screenshot_completion())
        };
        if dispatch != ContextDispatch::Scheduled {
            let pending = state
                .native
                .screenshot_pending
                .take()
                .ok_or_else(|| crate::contract!())?;
            state
                .native
                .screenshots
                .cancel(pending)
                .map_err(|_| crate::contract!())?;
            return Err(AgentWorkFailure::Context);
        }
        let capture = match state.native.next_event(worker, browser).await? {
            AgentRuntimeEvent::SemanticScreenshotTerminal(capture) => {
                if let Some(retained) = &mut state.native.retained {
                    retained.account_screenshot_terminal(now)?;
                }
                match capture {
                    Ok(capture) => capture,
                    Err(error) => {
                        let pending = state
                            .native
                            .screenshot_pending
                            .take()
                            .ok_or_else(|| crate::contract!())?;
                        state
                            .native
                            .screenshots
                            .cancel(pending)
                            .map_err(|_| crate::contract!())?;
                        if error
                            == zephium_agentic::SemanticScreenshotNativeFailure::ConsentRequired
                            && plan == SemanticModelImagePlan::Public
                        {
                            let next = state
                                .session
                                .as_ref()
                                .ok_or_else(|| crate::contract!())?
                                .policy
                                .model_image_consent_required(
                                    observation.request().context(),
                                    &origin,
                                );
                            if next == SemanticModelImagePlan::Waiting {
                                Self::raise_image_question(state, observation)?;
                                return Ok(None);
                            }
                        }
                        return Err(AgentWorkFailure::Screenshot(error));
                    }
                }
            }
            event => {
                state.native.retain(event)?;
                return Err(AgentWorkFailure::Mailbox);
            }
        };
        let pending = state
            .native
            .screenshot_pending
            .take()
            .ok_or_else(|| crate::contract!())?;
        let context = observation.request().context();
        state
            .native
            .screenshots
            .admit(pending, context, capture)
            .map(Some)
            .map_err(|_| AgentWorkFailure::ContextLost)
    }

    fn raise_image_question(
        state: &mut WorkState,
        observation: &SemanticObservation,
    ) -> Result<(), AgentWorkFailure> {
        let reason = AgentBrowserHumanReason::UserDecision;
        state
            .journal_mut()?
            .emit(AgentWorkEventKind::ModelRequestedHuman(reason))?;
        state.model_human_request = Some(AgentWorkHumanRequest {
            context: observation.request().context(),
            observation: observation.request().id(),
            generation: observation.request().generation(),
            reason,
            retained_resource: state
                .native
                .retained
                .as_ref()
                .map(|browser| browser.binding().lease().resource().identity()),
        });
        Ok(())
    }

    async fn wait_current(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        turn: AgentBrowserProviderTurn,
        observation: SemanticObservation,
    ) -> Result<
        Result<
            (
                SemanticObservation,
                SemanticCaptureInstant,
                AgentBrowserProviderTurn,
            ),
            Box<(AgentBrowserProviderTurn, SemanticObservation)>,
        >,
        AgentWorkFailure,
    > {
        state.check_task_contract()?;
        if !state.standalone_wait {
            return Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::UnsupportedTool(AgentBrowserToolKind::Wait),
            ));
        }
        let AgentBrowserToolProposal::Wait { condition, timeout } = turn.turn.proposal() else {
            return Err(crate::contract!());
        };
        let wait_deadline = Instant::now()
            .checked_add(Duration::from_millis(u64::from(timeout.millis())))
            .map_or(state.native.deadline, |deadline| {
                deadline.min(state.native.deadline)
            });
        let original = observation.clone();
        let Ok(mut wait) = SemanticStandaloneWait::prepare(
            *condition,
            observation,
            turn.turn.continuation().baseline(),
        ) else {
            return Ok(Err(Box::new((turn, original))));
        };
        let mut backoff = SemanticStandaloneWaitBackoff::new();
        let result = loop {
            let now = Instant::now();
            if now >= wait_deadline {
                break wait.time_out();
            }
            let wake = now
                .checked_add(Duration::from_millis(u64::from(
                    backoff.next_delay_millis(),
                )))
                .map_or(wait_deadline, |wake| wake.min(wait_deadline));
            tokio::select! {
                biased;
                event = state.native.next_event(worker, browser) => {
                    state.native.retain(event?)?;
                    return Err(AgentWorkFailure::Mailbox);
                }
                () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
            }
            if Instant::now() >= wait_deadline {
                break wait.time_out();
            }
            state.journal_mut()?.emit(AgentWorkEventKind::Observing)?;
            let current = match Self::observe_once(state, worker, browser).await {
                Err(AgentWorkFailure::Observation(SemanticRuntimePortFailure::NotReady)) => {
                    continue;
                }
                result => result?,
            };
            let Ok(step) = wait.advance(current) else {
                return Ok(Err(Box::new((turn, original))));
            };
            if Instant::now() >= wait_deadline {
                break match step {
                    SemanticStandaloneWaitStep::Pending(pending) => pending.time_out(),
                    SemanticStandaloneWaitStep::Satisfied(result) => result.into_timed_out(),
                };
            }
            match step {
                SemanticStandaloneWaitStep::Pending(pending) => wait = pending,
                SemanticStandaloneWaitStep::Satisfied(result) => break result,
            }
        };
        state.native.check_control(worker, browser)?;
        state.refresh_account(worker, browser)?;
        let action_authority = state.action_authority(result.observation())?;
        let captured_at = {
            let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
            let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
            SemanticCaptureInstant::from_millis(now.millis())
        };
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let cancellation = session.cancellation.clone();
        let (next_turn, observation) = Self::provider(
            &mut state.native,
            worker,
            browser,
            cancellation,
            session.continue_after_standalone_wait(
                turn.into_tool_turn(),
                result,
                Some(&action_authority),
            ),
        )
        .await?;
        Ok(Ok((observation, captured_at, next_turn)))
    }

    async fn finish_owned_extraction(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        observation: &SemanticObservation,
        captured_at: SemanticCaptureInstant,
        after_preparation: bool,
    ) -> Result<(), AgentWorkFailure> {
        state.refresh_account(worker, browser)?;
        let schema = state
            .extraction_schema
            .as_ref()
            .ok_or_else(|| crate::contract!())?;
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let crop_account = session.account;
        let crop_config = if state.viewport_screenshot && session.supports_record_crops() {
            session.config.clone().with_viewport_screenshot()
        } else {
            session.config.clone()
        };
        let crop_baseline = SemanticObservationAcknowledgement::whole_page_scope(observation)
            .ok_or_else(|| crate::contract!())?
            .0;
        let result = Self::provider(
            &mut state.native,
            worker,
            browser,
            session.cancellation.clone(),
            async {
                if after_preparation {
                    session
                        .extract_owned_observation(observation, captured_at, schema, None)
                        .await
                } else {
                    session
                        .extract_owned_initial(observation, captured_at, schema)
                        .await
                }
            },
        )
        .await?;
        if result.is_empty() {
            return Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::NoExtractionEvidence,
            ));
        }
        if !state.task.judged(observation) {
            state.task.evaluate(observation)?;
        }
        if state.task.accept_extraction(&result)? != AgentWorkTaskProgress::Complete {
            return Err(crate::contract!());
        }
        state.check_task_contract()?;
        state.native.check_control(worker, browser)?;
        let dropped = result.stats().dropped();
        let mut owned = result.into_owned().map_err(|_| crate::contract!())?;
        Self::attach_record_crop_permission(
            state,
            worker,
            browser,
            &mut owned,
            observation,
            RecordCropGrant {
                baseline: &crop_baseline,
                config: &crop_config,
                account: crop_account,
            },
        )?;
        state.extraction = Some(owned);
        if dropped > 0 {
            state
                .journal_mut()?
                .emit(AgentWorkEventKind::ExtractionDropped { values: dropped })?;
        }
        Ok(())
    }

    async fn extract_current(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        turn: AgentBrowserProviderTurn,
        observation: &SemanticObservation,
        captured_at: SemanticCaptureInstant,
    ) -> Result<Option<SemanticObservation>, AgentWorkFailure> {
        state.check_task_contract()?;
        let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
        session.check_live().map_err(AgentWorkFailure::Browser)?;
        // Keep the original run's capture capability and account authority.
        // Native listing reads bind their newly admitted observation below.
        let record_crop_baseline = turn.turn.continuation().baseline().clone();
        // Model-picture preference is independent of the existing local crop
        // authority. Preserve only the original Sensitive+model capability gate.
        let record_crop_config = if state.viewport_screenshot && session.supports_record_crops() {
            session.config.clone().with_viewport_screenshot()
        } else {
            session.config.clone()
        };
        if session.turns >= session.max_model_calls {
            // Do not capture data when no mapping call can be admitted.
            return Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::TurnLimit,
            ));
        }
        let mut completed_listing = if matches!(turn.turn.proposal(),
            AgentBrowserToolProposal::Extract { scope: AgentBrowserScopeProposal::Initial, schema }
                if state.extraction_schema.as_ref().is_some_and(|expected| expected.id() == *schema && expected.is_row_collection()))
            && turn
                .turn
                .continuation()
                .baseline()
                .authenticates(observation)
        {
            if observation
                .frames()
                .first()
                .and_then(|frame| frame.nodes().first())
                .is_some_and(|node| node.role() != SemanticRole::Document)
            {
                let schema = state
                    .extraction_schema
                    .as_ref()
                    .ok_or_else(|| crate::contract!())?;
                let read = read_semantic_observation_for_schema(
                    observation,
                    SemanticReadAuthority::Acknowledged(turn.turn.continuation().baseline()),
                    captured_at,
                    SemanticReadSensitivityLimit::PublicOnly,
                    schema.extraction_read_budget(),
                    schema,
                )
                .map_err(|error| {
                    AgentWorkFailure::Browser(AgentBrowserProviderError::Read(error))
                })?;
                state
                    .retained_read_evidence
                    .retain(&read, turn.turn.continuation().baseline())
                    .map_err(|error| {
                        AgentWorkFailure::Browser(AgentBrowserProviderError::Read(error))
                    })?;
            }
            let loaded = Box::pin(Self::load_listing(
                state,
                worker,
                browser,
                observation.clone(),
            ))
            .await?;
            // A look rooted inside a modal sheet (a filter drawer) is no whole
            // page to map records from on its own: the model's look is mapped.
            (loaded != *observation
                && SemanticObservationAcknowledgement::whole_page_scope(&loaded).is_some())
            .then_some(loaded)
        } else {
            None
        };
        if let Some(loaded) = completed_listing.take() {
            if state.human_request && Self::raise_human_wall(state, &loaded)? {
                return Ok(Some(loaded));
            }
            if looks_like_human_challenge(&loaded) {
                let (loaded, held) =
                    Self::settle_human_challenge(state, worker, browser, loaded).await?;
                if held {
                    return Ok(Some(loaded));
                }
                completed_listing = Some(loaded);
            } else {
                completed_listing = Some(loaded);
            }
        }
        let observation = completed_listing.as_ref().unwrap_or(observation);
        let captured_at = if completed_listing.is_some() {
            SemanticCaptureInstant::from_millis(
                state
                    .journal_mut()?
                    .clock
                    .now()
                    .map_err(|_| crate::contract!())?
                    .millis(),
            )
        } else {
            captured_at
        };
        let expanded = if matches!(
            turn.turn.proposal(),
            AgentBrowserToolProposal::Extract {
                scope: AgentBrowserScopeProposal::Subtree(_),
                ..
            }
        ) {
            if !state.subtree_extraction {
                return Err(AgentWorkFailure::Browser(
                    AgentBrowserProviderError::UnsupportedTool(AgentBrowserToolKind::Extract),
                ));
            }
            let frames = observation
                .frames()
                .iter()
                .map(|frame| frame.frame().clone())
                .collect::<Vec<_>>();
            let schema = state
                .extraction_schema
                .as_ref()
                .ok_or_else(|| crate::contract!())?;
            let next = state.native.id()?;
            let request = turn
                .turn
                .continuation()
                .begin_extraction_subtree(
                    observation,
                    &frames,
                    SemanticObservationId::new(next).ok_or_else(|| crate::contract!())?,
                    schema,
                    SemanticObservationBudget::INITIAL_FILTERED,
                )
                .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()))?;
            let frame = request
                .scope()
                .anchor()
                .ok_or_else(|| crate::contract!())?
                .frame()
                .clone();
            state.journal_mut()?.emit(AgentWorkEventKind::Observing)?;
            // Exactly one requested capture, not an observation/mutation retry.
            Some(Self::capture_once(state, worker, browser, request, frame).await?)
        } else {
            None
        };
        state.check_task_contract()?;
        let (source, captured_at) = if let Some(expanded) = &expanded {
            (
                expanded,
                SemanticCaptureInstant::from_millis(
                    state
                        .journal_mut()?
                        .clock
                        .now()
                        .map_err(|_| crate::contract!())?
                        .millis(),
                ),
            )
        } else {
            (observation, captured_at)
        };
        let frames = source
            .frames()
            .iter()
            .map(|snapshot| snapshot.frame().clone())
            .collect::<Vec<_>>();
        // A page that shows too little text to read its records from (a
        // canvas, a gallery, prices drawn as images) is mapped with one
        // viewport screenshot beside its text, when the model can see. The
        // image is best effort: without it the mapping runs as before.
        let mut viewport = if completed_listing.is_none()
            && expanded.is_none()
            && state.viewport_screenshot
            && state.native.screenshot_pending.is_none()
            && state
                .session
                .as_ref()
                .is_some_and(|session| session.supports_viewport_mapping())
            && state
                .native
                .retained
                .as_ref()
                .is_none_or(|browser| browser.supports_screenshots())
            && zephium_agentic::semantic_observation_needs_visual_mapping(observation)
        {
            let baseline = turn.turn.continuation().baseline().clone();
            match Self::capture_screenshot(
                state,
                worker,
                browser,
                &baseline,
                observation,
                SemanticScreenshotBudget::VIEWPORT_MAPPING,
            )
            .await
            {
                Ok(Some(screenshot)) => Some(screenshot),
                Ok(None) => return Ok(None),
                Err(
                    AgentWorkFailure::Screenshot(_)
                    | AgentWorkFailure::ScreenshotRequest(_)
                    | AgentWorkFailure::Context
                    | AgentWorkFailure::ContextLost
                    | AgentWorkFailure::Browser(AgentBrowserProviderError::Authority),
                ) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        state.refresh_account(worker, browser)?;
        // Freeze the independently verified sample after the existing refresh.
        // That refresh refuses context/account changes; taking its fresh token
        // here preserves exact equality without reauthorizing after closure.
        let record_crop_account = state
            .session
            .as_ref()
            .ok_or_else(|| crate::contract!())?
            .account;
        let schema = state
            .extraction_schema
            .as_ref()
            .ok_or_else(|| crate::contract!())?;
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        if let Some(image) = viewport.as_ref() {
            let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
            if session
                .policy
                .admit_model_image(image, observation, session.account, now)
                .await
                .is_err()
            {
                viewport = None;
            }
        }
        let result = if completed_listing.is_some() {
            Self::provider(
                &mut state.native,
                worker,
                browser,
                session.cancellation.clone(),
                session.extract_owned_observation(
                    source,
                    captured_at,
                    schema,
                    Some(&state.retained_read_evidence),
                ),
            )
            .await?
        } else {
            let result = Self::provider(
                &mut state.native,
                worker,
                browser,
                session.cancellation.clone(),
                session.extract_from_with_evidence(
                    turn,
                    source,
                    expanded.as_ref().map(|_| observation),
                    &frames,
                    captured_at,
                    schema,
                    if state.progressive_observation || state.navigation_discovery.is_some() {
                        Some(&state.retained_read_evidence)
                    } else {
                        None
                    },
                    viewport.as_ref(),
                ),
            )
            .await?;
            result
        };
        let dropped = result.stats().dropped();
        // The look a read maps (a whole-page expansion, a fresh look) is the
        // one the task judges before it accepts what was read there.
        if !state.task.judged(source) {
            state.task.evaluate(source)?;
        }
        if state.task.accept_extraction(&result)? != AgentWorkTaskProgress::Complete {
            return Err(crate::contract!());
        }
        state.check_task_contract()?;
        let mut owned = result.into_owned().map_err(|_| crate::contract!())?;
        let record_crop_baseline = if completed_listing.is_some() {
            // Owned extraction already admitted and mapped this exact public
            // observation. This updates source identity, never capture authority.
            SemanticObservationAcknowledgement::whole_page_scope(source)
                .ok_or_else(|| crate::contract!())?
                .0
        } else {
            record_crop_baseline
        };
        Self::attach_record_crop_permission(
            state,
            worker,
            browser,
            &mut owned,
            observation,
            RecordCropGrant {
                baseline: &record_crop_baseline,
                config: &record_crop_config,
                account: record_crop_account,
            },
        )?;
        state.extraction = Some(owned);
        if dropped > 0 {
            state
                .journal_mut()?
                .emit(AgentWorkEventKind::ExtractionDropped { values: dropped })?;
        }
        Ok(completed_listing)
    }

    fn attach_record_crop_permission(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        owned: &mut SemanticOwnedExtractionResult,
        observation: &SemanticObservation,
        grant: RecordCropGrant<'_>,
    ) -> Result<(), AgentWorkFailure> {
        let RecordCropGrant {
            baseline: record_crop_baseline,
            config: record_crop_config,
            account: record_crop_account,
        } = grant;
        if state.viewport_screenshot && record_crop_config.permits_viewport_screenshot() {
            state.native.check_control(worker, browser)?;
            if let Some(current) = state
                .session
                .as_mut()
                .filter(|session| session.check_live().is_ok())
            {
                if let Ok(policy_now) = current.policy_now() {
                    let now = Instant::now();
                    let policy_end = current
                        .policy
                        .manifest()
                        .plan_node(current.lease.node())
                        .map(|node| node.expires_at())
                        .unwrap_or(policy_now)
                        .min(current.policy.manifest().expires_at());
                    let account_left = zephium_agentic::MAX_AGENT_ACCOUNT_ATTESTATION_AGE_MILLIS
                        .saturating_sub(
                            policy_now
                                .millis()
                                .saturating_sub(record_crop_account.observed_at().millis()),
                        );
                    let expires = state
                        .native
                        .deadline
                        .min(now + Duration::from_secs(8))
                        .min(now + Duration::from_millis(account_left))
                        .min(
                            now + Duration::from_millis(
                                policy_end.millis().saturating_sub(policy_now.millis()),
                            ),
                        );
                    owned.carry_record_crop_permission(
                        record_crop_config,
                        &current.policy,
                        current.lease,
                        current
                            .max_actions
                            .saturating_sub(current.next_action.saturating_sub(1)),
                        record_crop_account,
                        current.account,
                        observation,
                        record_crop_baseline,
                        now,
                        expires,
                    );
                }
            }
        }
        Ok(())
    }

    async fn close_context(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(), AgentWorkFailure> {
        state.retained_read_evidence.clear();
        let id = state.native.identity.id();
        // Revoke before teardown even on trusted successful completion.
        state.native.revoke(browser)?;
        if let Some(expected) = state.native.cancellation {
            let event = state.native.next_event(worker, browser).await?;
            match event {
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::CancellationSettled(
                    settlement,
                )) if settlement.current() == expected => {
                    state.native.cancellation = None;
                    if settlement.outcome().is_err() {
                        state.native.retain(AgentRuntimeEvent::NativeTerminal(
                            ContextNativeEvent::CancellationSettled(settlement),
                        ))?;
                        return Err(AgentWorkFailure::Context);
                    }
                }
                event => {
                    state.native.retain(event)?;
                    return Err(AgentWorkFailure::Mailbox);
                }
            }
        }
        if state.native.close_attempted {
            return Err(AgentWorkFailure::Context);
        }
        state.native.close_attempted = true;
        let op = ContextOperationId::new(state.native.id()?).ok_or_else(|| crate::contract!())?;
        let operation = state
            .native
            .contexts()?
            .begin_close(id, op)
            .map_err(|_| AgentWorkFailure::Context)?;
        state.native.operation = Some(operation);
        let request =
            ContextTransitionRequest::try_new(operation).map_err(|_| AgentWorkFailure::Context)?;
        if browser.dispatch(ContextNativeRequest::Transition(request)) != ContextDispatch::Scheduled
        {
            state.native.operation = None;
            state
                .native
                .contexts()?
                .settle_close(id, operation, ContextSettlement::Refused)
                .map_err(|_| AgentWorkFailure::Context)?;
            return Err(AgentWorkFailure::Context);
        }
        let event = state.native.next_event(worker, browser).await?;
        match event {
            AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::TransitionSettled(
                settlement,
            )) if settlement.operation() == operation => {
                state.native.operation = None;
                let applied = settlement.outcome().is_ok();
                state
                    .native
                    .contexts()?
                    .settle_close(
                        id,
                        operation,
                        if applied {
                            ContextSettlement::Applied
                        } else {
                            ContextSettlement::Refused
                        },
                    )
                    .map_err(|_| AgentWorkFailure::Context)?;
                if !applied {
                    return Err(AgentWorkFailure::Context);
                }
            }
            event => {
                state.native.retain(event)?;
                return Err(AgentWorkFailure::Mailbox);
            }
        }
        Self::release_closed_context(state)
    }

    fn release_closed_context(state: &mut WorkState) -> Result<(), AgentWorkFailure> {
        let id = state.native.identity.id();
        let journal = state
            .session
            .as_mut()
            .and_then(|session| session.journal.as_mut())
            .or(state.journal.as_mut())
            .ok_or_else(|| crate::contract!())?;
        let release = journal
            .supervisor
            .reap_terminal_context(state.native.contexts()?, id)
            .map_err(|_| AgentWorkFailure::Context)?;
        let lease = state.native.profile.ok_or(AgentWorkFailure::Context)?;
        state
            .native
            .profiles()?
            .release(lease, release)
            .map_err(|_| AgentWorkFailure::Context)?;
        state.native.profile = None;
        journal.record()
    }

    fn begin_recovery_close(state: &mut WorkState, browser: &WorkBrowser<'_>) {
        if !state.native.revoked || state.native.profile.is_none() || state.native.close_attempted {
            return;
        }
        // Teardown is not business-action replay. It must not wait for a lost
        // observation/cancellation callback to consume the cleanup deadline.
        // The independent slot preserves every previously dispatched owner.
        state.native.close_attempted = true;
        let operation = (|| {
            let id = state.native.identity.id();
            let op =
                ContextOperationId::new(state.native.id()?).ok_or_else(|| crate::contract!())?;
            state
                .native
                .contexts()?
                .begin_close(id, op)
                .map_err(|_| AgentWorkFailure::Context)
        })();
        let Ok(operation) = operation else {
            return;
        };
        let Ok(request) = ContextTransitionRequest::try_new(operation) else {
            return;
        };
        state.native.recovery_close = Some(operation);
        if browser.dispatch(ContextNativeRequest::Transition(request)) != ContextDispatch::Scheduled
        {
            state.native.recovery_close = None;
            let id = state.native.identity.id();
            if let Ok(contexts) = state.native.contexts() {
                let _ = contexts.settle_close(id, operation, ContextSettlement::Refused);
            }
        }
    }

    async fn close_success(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_mut().ok_or_else(|| crate::contract!())?;
        Self::close_context(state, worker, browser).await?;
        self.close_resources(worker, browser, None).await
    }

    async fn close_unsuccessful(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        deadline: Instant,
    ) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_ref().ok_or_else(|| crate::contract!())?;
        // This is a terminal resource join, not an alternative interpretation
        // of a previous failed close or an acknowledged successful task.
        if state.failure.is_none()
            || state.session.is_none()
            || state.drained.is_some()
            || state.native.profile.is_some()
            || !state.native.revoked
            || !state.native.close_attempted
            || !state.native.deferred.is_empty()
            || state.native.operation.is_some()
            || state.native.recovery_close.is_some()
            || state.native.observation.is_some()
            || state.native.action_pending
            || state.native.screenshot_pending.is_some()
            || state.native.cancellation.is_some()
            || state.native.shutdown_audit.is_some()
            || state.native_terminal.is_some()
            || Instant::now() >= deadline
        {
            return Err(AgentWorkFailure::Shutdown);
        }
        self.close_resources(worker, browser, Some(deadline)).await
    }

    async fn close_resources(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        cleanup: Option<Instant>,
    ) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_mut().ok_or_else(|| crate::contract!())?;
        let terminal_intent = match cleanup {
            Some(_) => WorkTerminalIntent::ClosedUnsuccessfully(
                state.failure.ok_or_else(|| crate::contract!())?,
            ),
            None => state.model_human_request.map_or(
                WorkTerminalIntent::Succeeded,
                WorkTerminalIntent::WaitingForHuman,
            ),
        };
        state.native.screenshots.seal_for_shutdown();
        let screenshots = std::mem::replace(
            &mut state.native.screenshots,
            SemanticScreenshotCoordinator::new(),
        );
        let resources = state
            .native
            .resources
            .as_mut()
            .ok_or(AgentWorkFailure::Context)?;
        resources
            .contexts
            .seal_for_shutdown()
            .map_err(|_| AgentWorkFailure::Shutdown)?;
        resources
            .profiles
            .seal_for_shutdown()
            .map_err(|_| AgentWorkFailure::Shutdown)?;
        resources
            .cookies
            .seal_for_shutdown()
            .map_err(|_| AgentWorkFailure::Shutdown)?;
        let session = state.session.take().ok_or_else(|| crate::contract!())?;
        let unsuccessful = !matches!(terminal_intent, WorkTerminalIntent::Succeeded);
        let finished = if unsuccessful {
            session.try_finish_unsuccessful()
        } else {
            session.try_finish()
        };
        let terminal = match finished {
            Ok(terminal) => terminal,
            Err(refusal) => {
                let failure = refusal.error();
                state.session = Some(refusal.into_session());
                return Err(AgentWorkFailure::Browser(failure));
            }
        };
        if state.terminal_intent.replace(terminal_intent).is_some() {
            state.session = Some(*terminal.session);
            return Err(crate::contract!());
        }
        // Once the provider session has closed around an accepted human
        // request, that clean handoff wins over a later stop notification.
        // Continue only the bounded terminal cleanup lane: it ignores control
        // messages but still requires exact native/audit receipts before the
        // terminal claim. A stop observed before this boundary still prevents
        // the handoff from being frozen.
        let cleanup = cleanup.or_else(|| {
            matches!(terminal_intent, WorkTerminalIntent::WaitingForHuman(_))
                .then_some(state.native.deadline)
        });
        state.model_human_request = None;
        let resources = match state.native.resources.take() {
            Some(resources) => resources,
            None => {
                state.session = Some(*terminal.session);
                return Err(AgentWorkFailure::Context);
            }
        };
        let AgentBrowserSession {
            policy,
            journal,
            action_proposal_failure,
            action_executions,
            action_settlements,
            ..
        } = *terminal.session;
        let provider = terminal.provider;
        // These exact original owners, never replacement empty coordinators,
        // enter the constructor-closed native shutdown cohort.
        state.drained = Some(WorkDrained {
            proposal_refusal: action_proposal_failure,
            policy: Some(policy),
            journal,
            provider: Some(provider),
            native: None,
            resources: Some(AgentNativeShutdownResources::new(
                resources.contexts,
                resources.profiles,
                resources.cookies,
                action_executions,
                action_settlements,
                screenshots,
            )),
            proof: None,
            delivery: None,
            scoped_refusal: None,
        });
        let drained = state.drained.as_mut().ok_or_else(|| crate::contract!())?;
        match AgentNativeShutdownCoordinator::try_new(
            drained.resources.take().ok_or_else(|| crate::contract!())?,
        ) {
            Ok(native) => drained.native = Some(native),
            Err(refusal) => {
                drained.resources = Some(refusal.into_resources());
                return Err(AgentWorkFailure::Shutdown);
            }
        }
        let audit =
            ContextResourceAuditId::new(state.native.id()?).ok_or_else(|| crate::contract!())?;
        let native = drained.native.as_mut().ok_or_else(|| crate::contract!())?;
        native
            .begin_port_seal(audit)
            .map_err(|_| AgentWorkFailure::Shutdown)?;
        state.native.shutdown_audit = Some(audit);
        native
            .account_port_seal(audit, browser.seal_for_shutdown(audit)?)
            .map_err(|_| AgentWorkFailure::Shutdown)?;
        // No automatic mutation retry. A nonzero native barrier is retained
        // recovery; subsequent read-only resource qualification is explicit.
        if native.status().stage() != AgentNativeShutdownStage::ShutdownAuditPending {
            state.native.shutdown_audit = None;
            return Err(AgentWorkFailure::Shutdown);
        }
        match state
            .native
            .terminal_event(worker, browser, cleanup)
            .await?
        {
            AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::ShutdownAuditSettled(
                settlement,
            )) if settlement.audit() == audit => {
                native
                    .settle_shutdown_audit(settlement)
                    .map_err(|_| AgentWorkFailure::Shutdown)?;
                state.native.shutdown_audit = None;
            }
            event => {
                state.native.retain(event)?;
                return Err(AgentWorkFailure::Mailbox);
            }
        }
        let native = drained.native.take().ok_or_else(|| crate::contract!())?;
        match native.finish() {
            Ok(proof) => drained.proof = Some(proof),
            Err(refusal) => {
                drained.native = Some(refusal.into_coordinator());
                return Err(AgentWorkFailure::Shutdown);
            }
        }
        self.finish_accounting(worker, browser, cleanup).await
    }

    async fn finish_accounting(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        cleanup: Option<Instant>,
    ) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_mut().ok_or_else(|| crate::contract!())?;
        let drained = state.drained.as_mut().ok_or_else(|| crate::contract!())?;
        let journal = drained.journal.as_mut().ok_or_else(|| crate::contract!())?;
        let terminal_intent = state.terminal_intent.ok_or_else(|| crate::contract!())?;
        let completion = match terminal_intent {
            WorkTerminalIntent::WaitingForHuman(_) => {
                AgentSupervisorCompletion::Failed(AgentSupervisorFailure::PolicyDenied)
            }
            WorkTerminalIntent::ClosedUnsuccessfully(failure) => {
                let cancellation = match worker.stop_reason() {
                    Some(AgentRuntimeStopReason::HumanTakeover) => {
                        Some(AgentSupervisorCancellationReason::HumanTakeover)
                    }
                    Some(AgentRuntimeStopReason::PolicyRevoked) => {
                        Some(AgentSupervisorCancellationReason::PolicyRevoked)
                    }
                    Some(AgentRuntimeStopReason::Cancelled | AgentRuntimeStopReason::Suspend) => {
                        Some(AgentSupervisorCancellationReason::UserRequested)
                    }
                    None if failure == AgentWorkFailure::Deadline => {
                        Some(AgentSupervisorCancellationReason::DeadlineExceeded)
                    }
                    None => None,
                };
                let cancellation = if worker.shutdown_deadline().is_some() {
                    Some(AgentSupervisorCancellationReason::Shutdown)
                } else {
                    cancellation
                };
                if let Some(reason) = cancellation {
                    let _ = journal
                        .supervisor
                        .cancel_subtree(journal.root, journal.cancellation, reason)
                        .map_err(|_| AgentWorkFailure::Accounting)?;
                    journal.record()?;
                }
                AgentSupervisorCompletion::Failed(match failure {
                    AgentWorkFailure::Browser(AgentBrowserProviderError::Action(
                        crate::AgentBrowserActionError::NeedsHuman(_),
                    )) => AgentSupervisorFailure::PolicyDenied,
                    AgentWorkFailure::Browser(
                        AgentBrowserProviderError::Account(_)
                        | AgentBrowserProviderError::NoExtractionEvidence,
                    ) => AgentSupervisorFailure::PolicyDenied,
                    AgentWorkFailure::Browser(_) => AgentSupervisorFailure::ProviderFailed,
                    _ => AgentSupervisorFailure::PolicyDenied,
                })
            }
            WorkTerminalIntent::Succeeded => AgentSupervisorCompletion::Succeeded,
        };
        let execution = journal.execution.take().ok_or_else(|| crate::contract!())?;
        journal
            .supervisor
            .complete(execution, completion)
            .map_err(AgentWorkFailure::Supervisor)?;
        journal.record()?;
        journal
            .audit
            .seal_for_shutdown()
            .map_err(|_| AgentWorkFailure::Audit)?;
        self.deliver_audit(worker, browser, cleanup).await?;
        self.publish_terminal(worker).await
    }

    async fn deliver_audit(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        cleanup: Option<Instant>,
    ) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_mut().ok_or_else(|| crate::contract!())?;
        Self::drain_audit(state, worker, browser, cleanup).await?;
        state
            .journal_mut()?
            .audit
            .is_quiescent()
            .then_some(())
            .ok_or(AgentWorkFailure::Audit)
    }

    async fn drain_audit(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        cleanup: Option<Instant>,
    ) -> Result<(), AgentWorkFailure> {
        for _ in 0..MAX_PENDING_AGENT_AUDIT_EVENTS.div_ceil(MAX_AGENT_AUDIT_DELIVERY_EVENTS) {
            let journal = state.journal_mut()?;
            if journal.audit.status().pending() == 0 {
                return Ok(());
            }
            let batch = journal
                .audit
                .begin_next_delivery(MAX_AGENT_AUDIT_DELIVERY_EVENTS)
                .map_err(|_| AgentWorkFailure::Audit)?;
            let expected = batch.proof();
            let settlement = match state.audit.append(batch, worker.audit_completion()) {
                AgentAuditDispatch::Refused(settlement)
                    if settlement.proof() == expected
                        && settlement.outcome() != AgentAuditDeliveryOutcome::Committed =>
                {
                    settlement
                }
                AgentAuditDispatch::Accepted(proof) if proof == expected => {
                    match state
                        .native
                        .terminal_event(worker, browser, cleanup)
                        .await?
                    {
                        AgentRuntimeEvent::AuditTerminal(settlement)
                            if settlement.proof() == expected =>
                        {
                            settlement
                        }
                        event => {
                            state.native.retain(event)?;
                            return Err(AgentWorkFailure::Mailbox);
                        }
                    }
                }
                AgentAuditDispatch::Refused(settlement) => {
                    state
                        .native
                        .retain(AgentRuntimeEvent::AuditTerminal(settlement))?;
                    return Err(AgentWorkFailure::Audit);
                }
                AgentAuditDispatch::Accepted(_) => return Err(AgentWorkFailure::Audit),
            };
            if state
                .journal_mut()?
                .audit
                .settle_delivery(settlement)
                .map_err(|_| AgentWorkFailure::Audit)?
                != AgentAuditDeliveryOutcome::Committed
            {
                return Err(AgentWorkFailure::Audit);
            }
        }
        (state.journal_mut()?.audit.status().pending() == 0)
            .then_some(())
            .ok_or(AgentWorkFailure::Audit)
    }

    async fn publish_terminal(
        &mut self,
        worker: &mut AgentRuntimeWorker,
    ) -> Result<(), AgentWorkFailure> {
        let state = self.state.as_mut().ok_or_else(|| crate::contract!())?;
        if !state.native.deferred.is_empty()
            || state.native.operation.is_some()
            || state.native.recovery_close.is_some()
            || state.native.observation.is_some()
            || state.native.action_pending
            || state.native.screenshot_pending.is_some()
            || state.native.cancellation.is_some()
            || state.native.shutdown_audit.is_some()
            || state.native_terminal.is_some()
        {
            return Err(AgentWorkFailure::Mailbox);
        }
        let drained = state.drained.as_mut().ok_or_else(|| crate::contract!())?;
        if (drained.proof.is_none() && drained.delivery.is_none()) || drained.provider.is_none() {
            return Err(AgentWorkFailure::Shutdown);
        }
        let journal = drained.journal.as_ref().ok_or_else(|| crate::contract!())?;
        let terminal_intent = state.terminal_intent.ok_or_else(|| crate::contract!())?;
        if self.retained_terminal.is_some()
            && matches!(terminal_intent, WorkTerminalIntent::Succeeded)
            && state.extraction.is_none()
        {
            return Err(crate::contract!());
        }
        let closure = AgentRunMetricClosure::try_close(
            drained
                .policy
                .as_ref()
                .ok_or_else(|| crate::contract!())?
                .manifest(),
            &journal.supervisor,
            &journal.accounting,
            &journal.progress,
            &journal.actions,
            &journal.inputs,
        )
        .map_err(AgentWorkFailure::MetricClosure)?;
        // A clean success is claimed only as the ordinary terminal it proved.
        // If control arrived after its intent was frozen but before the claim,
        // the runtime must refuse that claim and retain recovery ownership;
        // accepting it as a cancelled terminal would publish `Accepted` from a
        // cancelled run. Waiting/unsuccessful intents already closed through
        // the unsuccessful accounting lane and may truthfully settle under the
        // matching control class without being relabelled.
        let controlled = !matches!(terminal_intent, WorkTerminalIntent::Succeeded);
        let class = if controlled && worker.shutdown_deadline().is_some() {
            AgentRuntimeControllerTerminalClass::Shutdown
        } else if controlled && worker.stop_reason().is_some() {
            AgentRuntimeControllerTerminalClass::Cancelled
        } else {
            AgentRuntimeControllerTerminalClass::Ordinary
        };
        let closure_matches_intent = match terminal_intent {
            WorkTerminalIntent::Succeeded => {
                closure.outcome() == AgentRunProgressOutcome::Succeeded
            }
            WorkTerminalIntent::WaitingForHuman(_) => {
                closure.outcome()
                    == AgentRunProgressOutcome::Failed(AgentSupervisorFailure::PolicyDenied)
            }
            WorkTerminalIntent::ClosedUnsuccessfully(_) => {
                closure.outcome() != AgentRunProgressOutcome::Succeeded
            }
        };
        if !closure_matches_intent {
            return Err(AgentWorkFailure::Accounting);
        }
        let claim = match if self.retained_terminal.is_some() {
            worker
                .try_claim_scoped_terminal(class)
                .await
                .map(retained::TerminalClaim::Scoped)
        } else {
            worker
                .try_claim_controller_terminal(class)
                .await
                .map(retained::TerminalClaim::Legacy)
        } {
            Ok(claim) => claim,
            Err(refusal) => {
                while let Some(event) = worker.try_drain_terminal_claim_refusal_event() {
                    state.native.retain(event)?;
                }
                return Err(AgentWorkFailure::RuntimeTerminal {
                    refusal,
                    stop: worker.stop_reason(),
                });
            }
        };
        let policy = drained.policy.take().ok_or_else(|| crate::contract!())?;
        let mut journal = drained.journal.take().ok_or_else(|| crate::contract!())?;
        // The only irreversible boundary is after runtime ingress closes and
        // native/provider proofs already exist. Refusal returns both owners.
        let settlement =
            match policy.settle_metric_closure(closure, &journal.accounting, journal.audit) {
                Ok(settlement) => settlement,
                Err(refusal) => {
                    let cause = refusal.error();
                    let (policy, audit) = refusal.into_parts();
                    drained.policy = Some(policy);
                    journal.audit = audit;
                    drained.journal = Some(journal);
                    return Err(AgentWorkFailure::PolicySettlement(cause));
                }
            };
        let provider = drained.provider.take().ok_or(AgentWorkFailure::Shutdown)?;
        match claim {
            retained::TerminalClaim::Legacy(claim) => {
                let proof = drained.proof.take().ok_or(AgentWorkFailure::Shutdown)?;
                claim.commit_with_shutdown(proof, settlement, provider);
            }
            retained::TerminalClaim::Scoped(claim) => {
                let delivery = drained.delivery.take().ok_or(AgentWorkFailure::Shutdown)?;
                if let Err(refusal) = claim.commit(delivery, settlement, provider) {
                    drained.scoped_refusal = Some(refusal);
                    return Err(AgentWorkFailure::Shutdown);
                }
            }
        }
        let human_review = drained
            .proposal_refusal
            .take()
            .and_then(crate::action::AgentBrowserActionProposalRefusal::discard_after_closure)
            .filter(|_| {
                closure.outcome()
                    == AgentRunProgressOutcome::Failed(AgentSupervisorFailure::PolicyDenied)
            });
        // Completion is separate from the bounded progress lane: saturation
        // cannot discard an already-consumed clean terminal owner.
        let _ = lock(&journal.events).publish(AgentWorkEventKind::Terminal);
        if let Some(terminal) = &self.retained_terminal {
            *lock(terminal) = Some(match terminal_intent {
                WorkTerminalIntent::WaitingForHuman(request) => {
                    AgentWorkRetainedOutcome::WaitingForHuman(AgentWorkWaitingForHuman {
                        settlement,
                        request,
                    })
                }
                WorkTerminalIntent::ClosedUnsuccessfully(failure) => {
                    AgentWorkRetainedOutcome::ClosedUnsuccessfully(AgentWorkClosedUnsuccessfully {
                        settlement,
                        failure,
                        human_review,
                    })
                }
                WorkTerminalIntent::Succeeded => AgentWorkRetainedOutcome::Accepted {
                    settlement,
                    extraction: Box::new(
                        state.extraction.take().ok_or_else(|| crate::contract!())?,
                    ),
                },
            });
            self.state.take();
            return Ok(());
        }
        *lock(&self.terminal) = Some(match terminal_intent {
            WorkTerminalIntent::WaitingForHuman(request) => {
                AgentWorkOutcome::WaitingForHuman(AgentWorkWaitingForHuman {
                    settlement,
                    request,
                })
            }
            WorkTerminalIntent::ClosedUnsuccessfully(failure) => {
                AgentWorkOutcome::ClosedUnsuccessfully(AgentWorkClosedUnsuccessfully {
                    settlement,
                    failure,
                    human_review,
                })
            }
            WorkTerminalIntent::Succeeded => AgentWorkOutcome::Succeeded(AgentWorkSuccess {
                settlement,
                extraction: state.extraction.take().map(Box::new),
            }),
        });
        self.state.take();
        Ok(())
    }

    async fn drain_recovery(
        &mut self,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Instant {
        let deadline = worker
            .shutdown_deadline()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(1));
        let Some(state) = self.state.as_mut() else {
            return deadline;
        };
        state.retained_read_evidence.clear();
        if Instant::now() < deadline {
            // Reconcile one already-known synchronous terminal, never reissue
            // native work. Time/audit refusal retains its exact original owner.
            if let Some(session) = state.session.as_mut() {
                if session.navigation_refusal.is_some() {
                    let _ = session.settle_navigation_refusal();
                }
            }
            Self::begin_recovery_close(state, browser);
            // A mailbox fault can have moved the exact audit terminal into the
            // bounded deferred lane before cleanup starts. Consume it there,
            // preserving foreign or otherwise unaccounted terminals in order.
            Self::reconcile_deferred_audit(state);
            Self::drain_retained_navigation(state, deadline).await;
            Self::drain_retained_action(state, deadline).await;
        }
        // Drain already-dispatched callbacks only; no action or provider retry.
        while state.native.operation.is_some()
            || state.native.recovery_close.is_some()
            || state.native.observation.is_some()
            || state.native.action_pending
            || state.native.screenshot_pending.is_some()
            || state.native.cancellation.is_some()
            || state.native.shutdown_audit.is_some()
            || state
                .journal_mut()
                .is_ok_and(|journal| journal.audit.status().in_flight() != 0)
        {
            let event = WorkNative::cleanup_event(worker, deadline).await;
            let Ok(event) = event else {
                break;
            };
            match &event {
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::ConstructionSettled(
                    value,
                )) if state.native.operation == Some(value.operation()) => {
                    state.native.operation = None
                }
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::NavigationSettled(value))
                    if state.native.operation == Some(value.operation()) =>
                {
                    state.native.operation = None
                }
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::TransitionSettled(value))
                    if state.native.operation == Some(value.operation()) =>
                {
                    state.native.operation = None
                }
                AgentRuntimeEvent::SemanticActionTerminal(value)
                    if state.session.as_ref().is_some_and(|session| {
                        session.action.as_ref().is_some_and(|action| {
                            action.accepts_settlement(&session.action_executions, value)
                        })
                    }) =>
                {
                    state.native.action_pending = false
                }
                AgentRuntimeEvent::SemanticScreenshotTerminal(_) => {}
                _ => {}
            }
            let accounted = match &event {
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::NavigationSettled(value))
                    if state.session.as_ref().is_some_and(|session| {
                        session
                            .navigation
                            .as_ref()
                            .is_some_and(|active| active.operation() == value.operation())
                    }) =>
                {
                    // A stop revokes continuation, not the original dispatched
                    // operation's accounting and durable terminal obligation.
                    state
                        .session
                        .as_mut()
                        .is_some_and(|session| session.settle_navigation_terminal(value).is_ok())
                }
                // These exact callbacks settle read-only/cancellation owners,
                // never a dispatched action or a replacement observation.
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::SemanticRuntimeSettled(
                    value,
                )) if state.native.observation.as_ref() == Some(value.correlation()) => {
                    state.native.observation = None;
                    true
                }
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::CancellationSettled(
                    value,
                )) if state.native.cancellation == Some(value.current()) => {
                    state.native.cancellation = None;
                    value.outcome().is_ok()
                }
                AgentRuntimeEvent::SemanticScreenshotTerminal(_) => {
                    let pending = state.native.screenshot_pending.take();
                    let now = state
                        .native
                        .clock
                        .as_ref()
                        .and_then(|clock| clock.now().ok());
                    let retained = state.native.retained.as_mut();
                    let accounted = match (retained, now) {
                        (Some(retained), Some(now)) => {
                            retained.account_screenshot_terminal(now).is_ok()
                        }
                        (None, _) => true,
                        _ => false,
                    };
                    pending.is_some_and(|pending| {
                        state.native.screenshots.cancel(pending).is_ok() && accounted
                    })
                }
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::TransitionSettled(value))
                    if state.native.recovery_close == Some(value.operation()) =>
                {
                    state.native.recovery_close = None;
                    let id = state.native.identity.id();
                    let applied = value.outcome().is_ok();
                    let settled = state.native.contexts().is_ok_and(|contexts| {
                        contexts
                            .settle_close(
                                id,
                                value.operation(),
                                if applied {
                                    ContextSettlement::Applied
                                } else {
                                    ContextSettlement::Refused
                                },
                            )
                            .is_ok()
                    });
                    settled && applied && Self::release_closed_context(state).is_ok()
                }
                AgentRuntimeEvent::AuditTerminal(settlement) => {
                    Self::settle_recovery_audit(state, *settlement)
                }
                AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::ShutdownAuditSettled(
                    settlement,
                )) if state.native.shutdown_audit == Some(settlement.audit()) => {
                    let settled = state
                        .drained
                        .as_mut()
                        .and_then(|drained| drained.native.as_mut())
                        .is_some_and(|native| native.settle_shutdown_audit(*settlement).is_ok());
                    if settled {
                        state.native.shutdown_audit = None;
                    }
                    settled
                }
                _ => false,
            };
            if !accounted && state.native.retain(event).is_err() {
                break;
            }
        }
        deadline
    }

    fn reconcile_deferred_audit(state: &mut WorkState) {
        let mut index = 0;
        while index < state.native.deferred.len() {
            let accounted = match &state.native.deferred[index] {
                AgentRuntimeEvent::AuditTerminal(settlement) => {
                    let settlement = *settlement;
                    Self::settle_recovery_audit(state, settlement)
                }
                _ => false,
            };
            if accounted {
                state.native.deferred.remove(index);
            } else {
                index += 1;
            }
        }
    }

    fn settle_recovery_audit(
        state: &mut WorkState,
        settlement: AgentAuditDeliverySettlement,
    ) -> bool {
        state.journal_mut().is_ok_and(|journal| {
            journal.audit.current_delivery().is_ok_and(|delivery| {
                delivery.is_some_and(|delivery| delivery.proof() == settlement.proof())
            }) && journal.audit.settle_delivery(settlement).is_ok()
        })
    }
}

impl Drop for AgentWorkController {
    fn drop(&mut self) {
        let Some(mut state) = self.state.take() else {
            return;
        };
        state.access.take();
        // Dropping undispatched objective data is not dropping runtime debt.
        if let Some(input) = state.input.as_mut() {
            input.objective.take();
        }
        state.transport.take();
        if let Some(session) = state.session.take() {
            match session.try_finish() {
                Ok(terminal) => state.session = Some(*terminal.session),
                Err(refusal) => state.session = Some(refusal.into_session()),
            }
        }
        let failure = state.failure.unwrap_or(AgentWorkFailure::Shutdown);
        // Only actor handles are abandoned here. Original operation receivers,
        // native port, health and destruction remain with the application.
        state.native.retained.take();
        if let Ok(journal) = state.journal_mut() {
            let _ = journal.emit(AgentWorkEventKind::Recovery);
        }
        if let Some(terminal) = &self.retained_terminal {
            let mut slot = lock(terminal);
            if slot.is_none() {
                *slot = Some(AgentWorkRetainedOutcome::Recovery(
                    AgentWorkRetainedRecovery(AgentWorkRecovery {
                        failure,
                        state: Box::new(state),
                    }),
                ));
            }
            return;
        }
        let mut slot = lock(&self.terminal);
        if slot.is_none() {
            *slot = Some(AgentWorkOutcome::Recovery(AgentWorkRecovery {
                failure,
                state: Box::new(state),
            }));
        }
    }
}

/// Maximum undrained product events. Saturation pauses execution; events are
/// never silently overwritten and the terminal owner has a separate slot.
pub const MAX_AGENT_WORK_EVENTS: usize = 64;

/// Closed phrases a bot check shows in place of the page. They are matched
/// against short node names and texts; page text is never kept or shown.
const HUMAN_CHALLENGE_PHRASES: [&str; 18] = [
    "just a moment",
    "performing security verification",
    "verify you are human",
    "verifying you are human",
    "please verify you are human",
    "checking your browser",
    "checking if the site connection is secure",
    "enable javascript and cookies to continue",
    "let's confirm you are human",
    "confirm you are human",
    "pardon our interruption",
    "attention required",
    "access denied",
    "please complete the security check",
    "press & hold",
    "human verification",
    "are you a human",
    "prove you are human",
];
/// Bot-check pages are small; a real page that mentions these words is not one.
const MAX_HUMAN_CHALLENGE_NODES: usize = 48;
/// How long a script-only check gets to pass on its own before a person is asked.
const HUMAN_CHALLENGE_SETTLE_MILLIS: [u64; 3] = [2_000, 3_000, 4_000];
/// Requests that the person sign in refused on one page before one is taken.
const MAX_SIGN_IN_REFUSALS: u8 = 2;
/// Extractions of a page still loading answered with a later look instead.
const MAX_LOADING_EXTRACT_WAITS: u8 = 2;
/// How long such a page is given before that look.
const LOADING_EXTRACT_PAUSE_MILLIS: u64 = 2_500;
/// A well-known script check is looked at again each second: for six on a
/// page task, for two on a read the lead can replace with other sources.
const AUTOMATIC_CHECK_SETTLE_MILLIS: [u64; 6] = [1_000; 6];
/// Refusals in a row, with no action verified and no page reached between
/// them, after which a page ends as a loop. Every refusal before that goes
/// back to the model with what to try instead.
const MAX_FRUITLESS_STEPS: u8 = 8;

type ProgressStamp = ([u8; 32], [u8; 32]);

#[derive(Default)]
struct NoProgress {
    pending: Option<ProgressStamp>,
    blocked: Vec<ProgressStamp>,
}
impl NoProgress {
    fn refuses(&mut self, current: Option<ProgressStamp>) -> bool {
        let Some(current) = current else {
            return false;
        };
        if let Some(previous) = self.pending.take() {
            if previous.0 == current.0 && !self.blocked.contains(&previous) {
                if self.blocked.len() == 64 {
                    self.blocked.remove(0);
                }
                self.blocked.push(previous);
            }
        }
        if self.blocked.contains(&current) {
            return true;
        }
        false
    }
    fn remember(&mut self, admitted: Option<ProgressStamp>) {
        if let Some(admitted) = admitted {
            self.pending = Some(admitted);
        }
    }
}

/// Turns in a row that only asked for a look, refused or not. From the
/// second, `snapshot` is not offered until an action or move happens.
#[derive(Default)]
struct LookRun(u8);

impl LookRun {
    fn looked(&mut self) -> bool {
        self.0 = self.0.saturating_add(1);
        self.0 >= 2
    }
    fn withheld(&self) -> bool {
        self.0 >= 2
    }
    fn reset(&mut self) {
        self.0 = 0;
    }
}

/// A native failure of one step that the page survived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Setback {
    Screenshot,
    Navigation,
    Action(SemanticActionBindingError),
}

/// Native failures in a row that went back to the model, and each failure
/// by its target. A third in a row with nothing done between them, or the
/// same failure on the same target a second time, ends the page as it ended
/// before such failures were handed back.
#[derive(Default)]
struct Setbacks {
    run: u8,
    seen: Vec<(Setback, u64)>,
}

const MAX_SETBACKS: u8 = 3;
const MAX_SEEN_SETBACKS: usize = 16;

impl Setbacks {
    fn count(
        &mut self,
        setback: Setback,
        target: u64,
        failure: AgentWorkFailure,
    ) -> Result<(), AgentWorkFailure> {
        self.run = self.run.saturating_add(1);
        if self.run >= MAX_SETBACKS || self.seen.contains(&(setback, target)) {
            return Err(failure);
        }
        if self.seen.len() == MAX_SEEN_SETBACKS {
            self.seen.remove(0);
        }
        self.seen.push((setback, target));
        Ok(())
    }
    fn progressed(&mut self) {
        self.run = 0;
    }
}

/// Counts refusals since the page last moved forward.
#[derive(Default)]
struct Fruitless(u8);

impl Fruitless {
    /// Two refusals before the end, the planner is offered extraction
    /// alone: what its looks already hold becomes the page's result instead
    /// of a loop that ends it with nothing.
    fn refused(&mut self, state: &mut WorkState) -> Result<(), AgentWorkFailure> {
        self.count()?;
        if self.0 == MAX_FRUITLESS_STEPS - 2 && state.extraction_schema.is_some() {
            if let Some(session) = state.session.as_mut() {
                session.decide_last();
            }
        }
        Ok(())
    }
    fn count(&mut self) -> Result<(), AgentWorkFailure> {
        self.0 = self.0.saturating_add(1);
        if self.0 >= MAX_FRUITLESS_STEPS {
            return Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::ActionProposalLoop,
            ));
        }
        Ok(())
    }
    fn progressed(&mut self) {
        self.0 = 0;
    }
}

#[cfg(test)]
mod fruitless_tests {
    use super::*;

    #[test]
    fn a_second_look_in_a_row_withholds_snapshot_until_something_is_done() {
        let mut run = LookRun::default();
        assert!(!run.withheld());
        assert!(!run.looked());
        assert!(run.looked());
        assert!(run.withheld());
        assert!(run.looked());
        run.reset();
        assert!(!run.withheld());
        assert!(!run.looked());
        assert!(run.looked());
    }

    #[test]
    fn unchanged_operations_are_barred_before_a_second_dispatch_but_real_changes_are_allowed() {
        let mut guard = NoProgress::default();
        let a = ([1; 32], [10; 32]);
        let different_action = ([1; 32], [11; 32]);
        let changed_page = ([2; 32], [10; 32]);
        let mut dispatched = 0;
        for _ in 0..3 {
            if !guard.refuses(Some(a)) {
                dispatched += 1;
                guard.remember(Some(a));
            }
        }
        assert_eq!(dispatched, 1);
        let mut through_read = NoProgress::default();
        through_read.remember(Some(a));
        assert!(!through_read.refuses(None));
        through_read.remember(None);
        assert!(
            through_read.refuses(Some(a)),
            "a cached read cannot reset the guard"
        );
        assert!(!guard.refuses(Some(different_action)));
        guard.remember(Some(different_action));
        assert!(!guard.refuses(Some(changed_page)));
        guard.remember(Some(changed_page));
        assert!(guard.refuses(Some(changed_page)));
        assert!(
            !guard.refuses(None),
            "waiting, extracting and human control stay available"
        );
        for i in 0..200 {
            let key = ([3; 32], [i as u8; 32]);
            if !guard.refuses(Some(key)) {
                guard.remember(Some(key));
            }
        }
        assert!(guard.blocked.len() <= 64);
    }

    #[test]
    fn a_third_failure_in_a_row_or_the_same_one_twice_ends_the_page() {
        let failure =
            AgentWorkFailure::Screenshot(zephium_agentic::SemanticScreenshotNativeFailure::Stale);
        let mut setbacks = Setbacks::default();
        assert!(setbacks.count(Setback::Screenshot, 0, failure).is_ok());
        assert!(setbacks.count(Setback::Navigation, 7, failure).is_ok());
        assert_eq!(
            setbacks.count(Setback::Navigation, 8, failure),
            Err(failure)
        );
        let mut setbacks = Setbacks::default();
        let changed = Setback::Action(SemanticActionBindingError::DispatchRejected);
        let unknown = Setback::Action(SemanticActionBindingError::OutcomeUnknown);
        assert!(setbacks.count(changed, 5, failure).is_ok());
        setbacks.progressed();
        assert!(setbacks.count(unknown, 5, failure).is_ok());
        setbacks.progressed();
        assert!(setbacks.count(changed, 6, failure).is_ok());
        setbacks.progressed();
        assert_eq!(
            setbacks.count(changed, 5, failure),
            Err(failure),
            "progress between them does not excuse the same failure twice"
        );
    }

    #[test]
    fn only_a_long_run_of_refusals_with_nothing_done_ends_a_page() {
        let mut fruitless = Fruitless::default();
        for _ in 1..MAX_FRUITLESS_STEPS {
            assert!(fruitless.count().is_ok());
        }
        fruitless.progressed();
        for _ in 1..MAX_FRUITLESS_STEPS {
            assert!(fruitless.count().is_ok());
        }
        assert_eq!(
            fruitless.count(),
            Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::ActionProposalLoop
            ))
        );
    }
}

/// A snapshot of the whole page: the initial look, or a region or subtree
/// of the document itself.
fn whole_look(proposal: &AgentBrowserToolProposal, observation: &SemanticObservation) -> bool {
    let target = match proposal {
        AgentBrowserToolProposal::Snapshot(AgentBrowserScopeProposal::Initial) => return true,
        AgentBrowserToolProposal::Snapshot(
            AgentBrowserScopeProposal::Region(target) | AgentBrowserScopeProposal::Subtree(target),
        ) => *target,
        _ => return false,
    };
    observation
        .frames()
        .iter()
        .flat_map(|frame| frame.nodes())
        .any(|node| {
            node.reference() == target
                && node.role() == SemanticRole::Document
                && node.parent().is_none()
        })
}

fn looks_like_human_challenge(observation: &SemanticObservation) -> bool {
    // A page that is only an embedded frame it cannot read (DataDome's and
    // similar captcha pages) is a check, whatever the frame says.
    let all = || {
        observation
            .frames()
            .iter()
            .flat_map(SemanticSnapshot::nodes)
    };
    if all().count() <= 4
        && all().any(|node| node.role() == SemanticRole::FrameBoundary)
        && all()
            .map(|node| {
                node.text().map_or(0, |text| text.len()) + node.name().map_or(0, |name| name.len())
            })
            .sum::<usize>()
            < 64
    {
        return true;
    }
    let mut nodes = 0;
    let mut matched = false;
    for snapshot in observation.frames() {
        for node in snapshot.nodes() {
            nodes += 1;
            if nodes > MAX_HUMAN_CHALLENGE_NODES {
                return false;
            }
            matched |= [node.name(), node.text()]
                .into_iter()
                .flatten()
                .map(|text| text.as_str().trim())
                .filter(|text| text.len() <= 96)
                .any(|text| {
                    let lower = text.to_ascii_lowercase();
                    HUMAN_CHALLENGE_PHRASES
                        .iter()
                        .any(|phrase| lower.starts_with(phrase))
                });
        }
    }
    matched
}

/// Closed reasons an interaction decision needs the page planner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentWorkDecisionRoute {
    /// No answer passed provider and projection admission.
    Unavailable,
    /// The selected operation or target was uncertain or absent.
    Uncertain,
    /// Typing needs the planner's separately admitted text/form proposal.
    TextRequired,
    /// A done answer is not independent task completion.
    CompletionUnverified,
    /// A blocked answer is not independent wall verification.
    BlockUnverified,
    /// The selected control needs an unsupported native recipe.
    Unsupported,
    /// Scrolling lacked accepted evidence of useful content below.
    ScrollUnverified,
    /// The operation repeated against an unchanged page.
    NoProgress,
    /// Native policy requires a different or separately approved proposal.
    Policy,
    /// A native read failed; its original ownership was closed before replanning.
    NativeRefused,
}

/// Content-free product phase. Model text never declares task completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentWorkEventKind {
    /// The exact runtime admission has started.
    Started,
    /// A Work-owned browser context is being prepared.
    ContextActive,
    /// Native semantic observation is active.
    Observing,
    /// One policy-reserved provider turn is active.
    ModelActive,
    /// An exact provider terminal was accounted.
    ModelSettled {
        /// Exact model call correlation; complete receipts remain Rust-owned.
        call: AgentModelCallId,
        /// Charged input tokens.
        input_tokens: u64,
        /// The cached-read subset of the input, when the provider reports it.
        cached_input_tokens: u64,
        /// Charged output tokens.
        output_tokens: u64,
        /// Charged micro-USD.
        cost_micro_usd: u64,
        /// Exact serialized provider request bytes.
        request_bytes: u32,
        /// Exact semantic disclosure bytes.
        semantic_bytes: u32,
        /// Exact or conservative usage accounting attribution.
        accounting: AgentModelUsageAccounting,
        /// Count plus streaming turn wall time, not server-only inference time.
        elapsed_millis: u64,
    },
    /// One typed decision call settled with closed provider/size/latency facts.
    DecisionSettled(DecisionCallDiagnostic),
    /// Why an action-only decision returned to the planner; no target or page data.
    DecisionPlanner(AgentWorkDecisionRoute),
    /// Per-kind fallback reasons; counts follow the decision routing contract.
    DecisionFallback {
        /// Noul, Choice and Score counts by unavailable, rate limited, invalid, uncertain.
        counts: [[u8; 4]; 3],
        /// Unresolved heads by declared purpose, in the closed purpose order:
        /// challenge, action, locate, picture, relevance, wall, completion, score.
        purposes: [u8; zephium_decision::DecisionPurpose::COUNT],
        /// Whether the shared call allowance permits attempting emulation.
        capacity: bool,
    },
    /// A catalog read found records on the typed path. `refused` is set when
    /// none could be copied and the page planner reads the page instead.
    RowRead {
        /// Records found, at most the requested number.
        found: u8,
        /// Whether a cell batch located their text columns.
        cells: bool,
        /// Why no record could be copied.
        refused: Option<SemanticExtractionError>,
    },
    /// Rust pressed a daily app's view control to read that view.
    AppViewOpened,
    /// Rust loaded more of a listing before its read.
    ListingLoaded {
        /// Loading steps taken.
        steps: u8,
        /// Distinct records seen across the loaded views.
        records: u16,
        /// Why the loading stopped.
        stop: listing::ListingStop,
    },
    /// Closed size facts of one observation a typed read decides on.
    ObservationFacts {
        /// Nodes across every captured frame.
        nodes: u16,
        /// Text bytes across every captured frame.
        text_bytes: u32,
        /// Nodes with the dialog role in the top frame.
        dialogs: u8,
        /// Whether the top frame was captured completely.
        complete: bool,
    },
    /// A chosen model's reply was cut to its first tool call. What was cut
    /// was never decoded and conferred nothing.
    ModelReplyTrimmed {
        /// Tool calls after the first.
        dropped_tool_calls: u16,
        /// Narration that came beside a tool call.
        dropped_text_bytes: u32,
    },
    /// A chosen model's reply had no usable browser call; it was told why
    /// and asked again this many times within the one call.
    ModelAskedAgain(u8),
    /// A fully received action call failed local decoding; nothing executed.
    ModelActionRejected(zephium_agentic::AgentBrowserToolContractError),
    /// A sparse page's mapping carried the host's viewport screenshot.
    ViewportImageAttached,
    /// The model proposed a bounded typed tool.
    ToolProposed(AgentBrowserToolKind),
    /// Snapshot scope was incompatible with the delivered baseline. No native
    /// capture ran; one budgeted provider turn can select a different operation.
    InspectionRefused,
    /// A model Navigate was refused before dispatch, with its closed reason:
    /// an unobserved or out-of-task address, the open document, a destination
    /// visited its limit, or no page visits left.
    NavigationRefused(AgentProviderNavigationRefusalReason),
    /// A model action failed binding before preparation, policy, or dispatch.
    /// Correcting the proposal consumes another ordinary budgeted model call.
    ActionProposalRefused(SemanticActionBindingError),
    /// An exact native scoped capture lost its anchor. A separate initial
    /// capture may restore current refs under the original run authority.
    InspectionAnchorLost,
    /// An independently authorized native effect is active.
    ActionActive,
    /// Native execution settled; a fresh observation is checking the outcome.
    Verifying,
    /// Native synchronously refused admission; the failed effect and batch
    /// were accounted. No native execution or retry is implied.
    ActionRejected(SemanticActionFailure),
    /// A completed read-only scroll did not prove movement; its failed receipt is closed.
    ActionUnverified(SemanticActionFailure),
    /// Records or optional record fields left out of an extraction because
    /// their own evidence did not hold; the rest stood.
    ExtractionDropped {
        /// Records and fields left out.
        values: u16,
    },
    /// Rust refused a mapping and the model mapped the same read once more,
    /// told why.
    ExtractionCorrected,
    /// A read's own target re-rendered while its page visibly changed; the
    /// change proved the action (a channel, thread or date opened).
    AppliedOnPageChange,
    /// A native effect was independently verified and accounted.
    Verified,
    /// An explicit policy/human boundary stopped execution.
    NeedsHuman(AgentNeedsHumanReason),
    /// The model deliberately stopped and requested a person with a closed reason.
    ModelRequestedHuman(AgentBrowserHumanReason),
    /// Execution owners closed; the separate outcome distinguishes task
    /// success from a fully drained failure/cancellation.
    Terminal,
    /// Exact retained ownership needs reconciliation; this is not success.
    Recovery,
}

/// How long a page waits for its look to become ready (another page's look
/// holding the presentation, a document still committing).
const NOT_READY_PATIENCE: Duration = Duration::from_secs(30);
/// Extra looks at a document still parsing before its first look gives up.
const DOCUMENT_LOADING_LOOKS: u32 = 3;

/// Stable content-free correlation for a product event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentWorkEvent {
    run: ContextRunId,
    sequence: u64,
    kind: AgentWorkEventKind,
    elapsed_millis: u64,
}

impl AgentWorkEvent {
    /// Exact run identity, never derived from page/model contents.
    pub const fn run(self) -> ContextRunId {
        self.run
    }
    /// Strictly increasing event identity within this run.
    pub const fn sequence(self) -> u64 {
        self.sequence
    }
    /// Closed product phase.
    pub const fn kind(self) -> AgentWorkEventKind {
        self.kind
    }
    /// Monotonic wall time since this actor's event journal was constructed.
    pub const fn elapsed_millis(self) -> u64 {
        self.elapsed_millis
    }
}

/// Closed failure vocabulary; no variant contains page, provider or user text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentWorkFailure {
    /// Challenge decision failed its exact observation/account binding.
    DecisionChallenge(DecisionProjectionError),
    /// Further-content decision failed its exact observation/account binding.
    DecisionMoreBelow(DecisionProjectionError),
    /// Action selection failed its exact observation/account or offered-option binding.
    DecisionOperation(DecisionProjectionError),
    /// Read selection failed its exact observation, account or schema binding.
    DecisionRead(DecisionProjectionError),
    /// Accounted scoped read reported AnchorMissing while its original retained
    /// document and lease remained live. Only progressive inspection may recover.
    InspectionAnchorLost,
    /// A settled model proposal crossed the trusted task's fresh phase gate.
    TaskPhase {
        /// Current product-side task state, never model completion text.
        expected: AgentWorkTaskProgress,
        /// Proposed bounded tool; no arguments or page content are retained.
        proposed: AgentBrowserToolKind,
    },
    /// Runtime cancellation or human revocation won admission.
    Cancelled,
    /// Human takeover requested revocation; no transfer is implied by recovery.
    HumanTakeover,
    /// Suspension requested revocation; native suspension is not fabricated.
    SuspendRequested,
    /// Trusted policy authority was revoked.
    PolicyRevoked,
    /// The caller's absolute deadline expired.
    Deadline,
    /// Bounded product event storage requires a consumer to drain it.
    Backpressure,
    /// Runtime callback/control correlation could not be reconciled.
    Mailbox,
    /// Exact atomic-close refusal and first trusted stop request, if present.
    RuntimeTerminal {
        /// Runtime-owned claim refusal, never flattened into generic failure.
        refusal: AgentRuntimeControllerTerminalRefusal,
        /// Product control reason; a request is not native acknowledgement.
        stop: Option<AgentRuntimeStopReason>,
    },
    /// Context/profile/native state refused the exact transition.
    Context,
    /// Exact content-free native port refusal.
    Native(ContextPortFailure),
    /// Exact content-free semantic runtime refusal.
    Observation(SemanticRuntimePortFailure),
    /// Exact bounded native viewport-capture refusal.
    Screenshot(SemanticScreenshotNativeFailure),
    /// Exact content-free refusal to bind a capture to its delivered observation.
    ScreenshotRequest(zephium_agentic::SemanticScreenshotRequestError),
    /// Renderer or navigation authority changed; stale refs are revoked.
    ContextLost,
    /// The public page's first load was redirected before it was ready.
    /// The gate refused it; the destination stays with the read's owner.
    Redirected,
    /// The page's main-frame load never reached its site: no connection, an
    /// unknown host, a refused or insecure connection.
    Unreachable,
    /// Provider or action authority returned a closed refusal.
    Browser(AgentBrowserProviderError),
    /// A progress/receipt/metric join was refused.
    Accounting,
    /// Exact content-free coverage refusal from the original metric reducers.
    MetricClosure(AgentRunMetricClosureError),
    /// Exact terminal policy/audit refusal; the original owners are retained.
    PolicySettlement(AgentRunPolicySettlementError),
    /// Exact model-receipt accounting refusal, with no provider content.
    ModelAccounting(AgentMetricError),
    /// Exact committed-input coverage refusal, with no disclosed input.
    InputAccounting(AgentProviderInputMetricError),
    /// Exact progress-reducer refusal, with no page or model content.
    ProgressAccounting(AgentProgressMetricError),
    /// Exact original supervisor refusal.
    Supervisor(AgentSupervisorRuntimeError),
    /// Durable audit delivery or acknowledgement was refused.
    Audit,
    /// The successful extraction could not be admitted for durable publication.
    ArtifactPublication(AgentWorkJournalError),
    /// Native or provider resources could not prove terminal drain.
    Shutdown,
    /// Trusted input, clock, state, or task contract was invalid.
    Contract,
    /// The task's own policy declines this one proposed action; the model
    /// may choose another, nothing having been issued.
    ActionDenied,
    /// The proposal must declare this effect, which the page shows it has.
    EffectRequired(SemanticEffectClass),
    /// The proposal repeats a commitment this page already made (a second
    /// add to cart of the same item); nothing is issued.
    AlreadyDone,
    /// The proposal repeats a step whose outcome this page task did not
    /// see: it may already have taken effect, so it is not issued again.
    OutcomeUnknown,
    /// The proposed send is not addressed to the recipient the task names;
    /// nothing is issued.
    WrongRecipient,
}

pub(super) struct WorkEvents {
    waker: Option<std::task::Waker>,
    started: Instant,
    run: ContextRunId,
    next: u64,
    queue: VecDeque<AgentWorkEvent>,
    fault: bool,
}

impl WorkEvents {
    pub(super) fn new(run: ContextRunId) -> Result<Self, AgentWorkFailure> {
        let mut queue = VecDeque::new();
        queue
            .try_reserve_exact(MAX_AGENT_WORK_EVENTS)
            .map_err(|_| AgentWorkFailure::Backpressure)?;
        Ok(Self {
            run,
            waker: None,
            started: Instant::now(),
            next: 1,
            queue,
            fault: false,
        })
    }

    pub(super) fn publish(&mut self, kind: AgentWorkEventKind) -> Result<(), AgentWorkFailure> {
        if self.fault || self.queue.len() == MAX_AGENT_WORK_EVENTS {
            self.fault = true;
            return Err(AgentWorkFailure::Backpressure);
        }
        let sequence = self.next;
        self.next = sequence
            .checked_add(1)
            .ok_or(AgentWorkFailure::Backpressure)?;
        self.queue.push_back(AgentWorkEvent {
            run: self.run,
            sequence,
            kind,
            elapsed_millis: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
        self.wake();
        Ok(())
    }

    fn wake(&self) {
        if let Some(waker) = &self.waker {
            let _notified =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| waker.wake_by_ref()));
        }
    }
}

pub(super) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    match value.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The exact supervisor, receipt reducers and durable ledger for one run.
/// It remains inside the session while provider/native authorities are live.
pub(crate) struct WorkJournal {
    pub(super) failure: Option<AgentWorkFailure>,
    model_started: Option<Instant>,
    pub(super) supervisor: AgentRunSupervisor,
    pub(super) execution: Option<AgentNodeExecution>,
    cancellation: AgentSupervisorCancellationId,
    pub(super) audit: AgentAuditLedger,
    pub(super) accounting: AgentRunAccountingMetrics,
    pub(super) progress: AgentRunProgressMetrics,
    pub(super) actions: AgentRunActionPerformanceMetrics,
    pub(super) inputs: AgentRunProviderInputMetrics,
    pub(super) root: AgentPlanNodeId,
    events: Arc<Mutex<WorkEvents>>,
    clock: Arc<dyn TerraControllerClock>,
    last_at: AgentPolicyInstant,
    next_audit: u64,
}

impl WorkJournal {
    pub(super) fn new(
        manifest: &AgentRunManifest,
        root: AgentPlanNodeId,
        id: AgentSupervisorId,
        cancellation: AgentSupervisorCancellationId,
        clock: Arc<dyn TerraControllerClock>,
        events: Arc<Mutex<WorkEvents>>,
    ) -> Result<Self, AgentWorkFailure> {
        let topology =
            AgentDelegationTopology::try_new(manifest, vec![AgentDelegationSpec::new(root, None)])
                .map_err(|_| crate::contract!())?;
        let supervisor = AgentRunSupervisor::new(id, topology);
        let audit = AgentAuditLedger::try_new(manifest, &supervisor)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        let accounting = AgentRunAccountingMetrics::try_new(manifest, &supervisor)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        let progress = AgentRunProgressMetrics::try_new(manifest, &supervisor)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        let actions = AgentRunActionPerformanceMetrics::try_new(manifest, &supervisor)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        let inputs = AgentRunProviderInputMetrics::try_new(manifest, &supervisor)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        Ok(Self {
            supervisor,
            failure: None,
            model_started: None,
            execution: None,
            cancellation,
            audit,
            accounting,
            progress,
            actions,
            inputs,
            root,
            events,
            clock,
            last_at: manifest.issued_at(),
            next_audit: 1,
        })
    }

    pub(super) fn emit(&self, kind: AgentWorkEventKind) -> Result<(), AgentWorkFailure> {
        lock(&self.events).publish(kind)
    }

    pub(super) fn record(&mut self) -> Result<(), AgentWorkFailure> {
        let now = self.clock.now().map_err(|_| crate::contract!())?;
        if now < self.last_at {
            return Err(crate::contract!());
        }
        let id = AgentAuditEventId::new(self.next_audit).ok_or(AgentWorkFailure::Accounting)?;
        self.next_audit = self
            .next_audit
            .checked_add(1)
            .ok_or(AgentWorkFailure::Accounting)?;
        let event = self
            .audit
            .record_current(&self.supervisor, self.root, id, now)
            .map_err(|_| AgentWorkFailure::Audit)?;
        self.last_at = now;
        self.progress
            .record_event(event)
            .map_err(AgentWorkFailure::ProgressAccounting)
    }

    pub(super) fn start(
        &mut self,
        attempt: AgentSupervisorAttemptId,
    ) -> Result<(), AgentWorkFailure> {
        self.record()?;
        self.execution = Some(
            self.supervisor
                .start(self.root, attempt)
                .map_err(|_| crate::contract!())?,
        );
        self.record()?;
        self.emit(AgentWorkEventKind::Started)
    }

    pub(super) fn model_active(
        &mut self,
        call: AgentProviderCallIdentity,
    ) -> Result<(), AgentWorkFailure> {
        if self.model_started.replace(Instant::now()).is_some() {
            return Err(AgentWorkFailure::Accounting);
        }
        self.supervisor
            .record_active_model_call(
                self.execution.as_ref().ok_or_else(|| crate::contract!())?,
                call,
            )
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.record()?;
        self.emit(AgentWorkEventKind::ModelActive)
    }

    pub(crate) fn needs_human(
        &mut self,
        transition: AgentNeedsHumanTransition,
    ) -> Result<(), AgentWorkFailure> {
        self.supervisor
            .record_human_refusal(
                self.execution.as_ref().ok_or_else(|| crate::contract!())?,
                transition,
            )
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.record()?;
        self.emit(AgentWorkEventKind::NeedsHuman(transition.reason()))
    }

    pub(super) fn model_settled(
        &mut self,
        receipt: AgentModelCallReceipt,
        input: AgentProviderInputMetricReceipt,
    ) -> Result<(), AgentWorkFailure> {
        self.accounting
            .record_model_receipt(receipt)
            .map_err(AgentWorkFailure::ModelAccounting)?;
        self.inputs
            .record(input)
            .map_err(AgentWorkFailure::InputAccounting)?;
        self.supervisor
            .record_model_call_result(
                self.execution.as_ref().ok_or_else(|| crate::contract!())?,
                receipt,
            )
            .map_err(AgentWorkFailure::Supervisor)?;
        self.record()?;
        let elapsed_millis = u64::try_from(
            self.model_started
                .take()
                .ok_or(AgentWorkFailure::Accounting)?
                .elapsed()
                .as_millis(),
        )
        .map_err(|_| crate::contract!())?;
        self.emit(AgentWorkEventKind::ModelSettled {
            call: receipt.id(),
            input_tokens: receipt.input_tokens(),
            cached_input_tokens: receipt
                .pricing_attribution()
                .map_or(0, |pricing| pricing.cached_input_tokens()),
            output_tokens: receipt.output_tokens(),
            cost_micro_usd: receipt.cost_micro_usd(),
            request_bytes: input.metrics().serialized_request_bytes(),
            semantic_bytes: input.metrics().semantic().disclosed_bytes(),
            accounting: receipt.usage_accounting(),
            elapsed_millis,
        })
    }

    pub(crate) fn action_active(
        &mut self,
        effect: &AgentActiveEffect,
    ) -> Result<(), AgentWorkFailure> {
        self.supervisor
            .record_active_effect(
                self.execution.as_ref().ok_or_else(|| crate::contract!())?,
                effect,
            )
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.record()?;
        self.emit(AgentWorkEventKind::ActionActive)
    }

    pub(super) fn action_settled(
        &mut self,
        receipt: AgentEffectReceipt,
        batch: &SemanticActionBatchResult,
        page_change: bool,
    ) -> Result<(), AgentWorkFailure> {
        self.accounting
            .record_effect_receipt(receipt)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.actions
            .record_batch_result(batch)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.supervisor
            .record_effect_result(
                self.execution.as_ref().ok_or_else(|| crate::contract!())?,
                receipt,
            )
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.record()?;
        if page_change {
            self.emit(AgentWorkEventKind::AppliedOnPageChange)?;
        }
        self.emit(AgentWorkEventKind::Verified)
    }

    pub(super) fn action_rejected(
        &mut self,
        batch: &SemanticActionBatchResult,
    ) -> Result<(), AgentWorkFailure> {
        self.action_failure(batch, false)
    }

    pub(super) fn action_unverified(
        &mut self,
        batch: &SemanticActionBatchResult,
    ) -> Result<(), AgentWorkFailure> {
        self.action_failure(batch, true)
    }

    fn action_failure(
        &mut self,
        batch: &SemanticActionBatchResult,
        issued: bool,
    ) -> Result<(), AgentWorkFailure> {
        let failure = batch.failure().ok_or(AgentWorkFailure::Accounting)?;
        let receipt = failure.receipt();
        let AgentEffectSettlement::Failed(reason) = receipt.settlement() else {
            return Err(AgentWorkFailure::Accounting);
        };
        self.accounting
            .record_effect_receipt(receipt)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.actions
            .record_batch_result(batch)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.supervisor
            .record_effect_result(
                self.execution.as_ref().ok_or_else(|| crate::contract!())?,
                receipt,
            )
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.record()?;
        self.emit(if issued {
            AgentWorkEventKind::ActionUnverified(reason)
        } else {
            AgentWorkEventKind::ActionRejected(reason)
        })
    }
}

impl fmt::Debug for WorkJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WorkJournal([owned, content-free])")
    }
}

/// A control's identity across looks of one document: a single-page app
/// re-renders it under a new reference, with the same role and name.
fn control_digest(node: &SemanticNode) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", node.role()).hash(&mut hasher);
    node.name().map(SemanticText::as_str).hash(&mut hasher);
    hasher.finish()
}

fn node_count(observation: &SemanticObservation) -> usize {
    observation
        .frames()
        .iter()
        .map(|frame| frame.nodes().len())
        .sum()
}
