//! Task-scoped document checkpoints through the original Work owners.

use super::*;

/// Where a document checkpoint comes from: the model's Navigate or Back, or
/// a same-site load the page started after an admitted action.
pub(super) enum NavigationStart {
    Model(Box<AgentBrowserProviderTurn>),
    Follow(Box<(AgentProviderContinuation, ContextNavigationTarget)>),
}

pub(super) struct NavigationTerminal {
    receipt: AgentNavigationReceipt,
    journal_failure: Option<AgentWorkFailure>,
}

impl AgentWorkController {
    pub(super) fn validate_navigation_target(
        target: &ContextNavigationTarget,
        context: &AgentWorkContextSpec,
    ) -> Result<(), AgentWorkFailure> {
        // Only the task-authored, admission-frozen URL is inspected here.
        // No page/model URL can widen this same-origin exact destination.
        if target == &context.target
            || target.as_url().fragment().is_some()
            || target.as_url().as_str().len() > MAX_AGENT_BROWSER_NAVIGATION_URL_BYTES
            || SemanticOrigin::parse(target.as_url().as_str()).as_ref() != Ok(&context.origin)
        {
            return Err(crate::contract!());
        }
        Ok(())
    }

    /// Why a model Navigate goes back to the model instead of to policy: a
    /// miscopied or unobserved link, an address a site task cannot open
    /// (another site, an in-page `#` route), the open document, a destination
    /// visited its limit, or no visits left. Each is correctable by the model;
    /// broken authority still fails closed in policy.
    pub(super) fn navigation_refusal(
        state: &WorkState,
        target: &ContextNavigationTarget,
        observation: &SemanticObservation,
    ) -> Option<AgentProviderNavigationRefusalReason> {
        use AgentProviderNavigationRefusalReason as Reason;
        let scope = state.navigation_discovery.as_ref()?;
        if !scope.admits(target) {
            return scope.is_site_session().then_some(Reason::OutsideScope);
        }
        // An in-page anchor ("Skip to content") or any address the session's
        // own document policy would refuse at dispatch is refused here, where
        // the model can choose again, instead of ending the page natively.
        if scope.is_site_session()
            && !zephium_agentic::WorkBrowserDocumentPolicy::SiteSession.admits_request(target)
        {
            return Some(Reason::OutsideScope);
        }
        let observed = observation
            .frames()
            .iter()
            .flat_map(|frame| frame.nodes())
            .any(|node| {
                node.role() == SemanticRole::Link
                    && node.sensitivity() == SemanticSensitivity::Public
                    && node.link_destination() == Some(target)
            });
        // A site task may open any address on its own site (its search
        // results for a query, a listing sorted by price); other scopes
        // follow only shown links.
        if !observed && !scope.is_site_session() {
            return Some(Reason::Unobserved);
        }
        let session = state.session.as_ref()?;
        session
            .policy
            .navigation_ledger_refusal(session.lease.lease(), target)
            .map(|refusal| match refusal {
                AgentNavigationLedgerRefusal::HopsSpent => Reason::HopsSpent,
                AgentNavigationLedgerRefusal::AlreadyHere => Reason::AlreadyHere,
                AgentNavigationLedgerRefusal::Revisited => Reason::Revisited,
                AgentNavigationLedgerRefusal::Redirected => Reason::Redirected,
            })
    }

    /// A refusal of the exact proposal refused before is marked repeated,
    /// so the model hears to change approach.
    pub(super) fn repeated_refusal(
        refused: &mut Vec<AgentProviderActionRefusalKey>,
        refusal: AgentProviderActionRefusal,
    ) -> Result<AgentProviderActionRefusal, AgentWorkFailure> {
        let Some(key) = refusal.key() else {
            return Ok(refusal);
        };
        if refused.contains(&key) {
            return Ok(refusal.repeated());
        }
        refused.try_reserve(1).map_err(|_| crate::contract!())?;
        refused.push(key);
        Ok(refusal)
    }

    /// Counts a refusal of a control that ran or reached the page (not
    /// observed, rejected at dispatch, covered) by the control's role and
    /// name digest: from the second on, the refusal is marked repeated. A
    /// covered control is kept for its key press only. Other refusals pass
    /// through.
    pub(super) fn count_refused_control(
        refused: &mut Vec<(u64, u8)>,
        covered: &mut Vec<u64>,
        refusal: AgentProviderActionRefusal,
        observation: &SemanticObservation,
        frames: &[SemanticFrameJoin],
    ) -> Result<AgentProviderActionRefusal, AgentWorkFailure> {
        if !matches!(
            refusal.reason(),
            SemanticActionBindingError::Unverified
                | SemanticActionBindingError::DispatchRejected
                | SemanticActionBindingError::TargetCovered
                | SemanticActionBindingError::OutcomeUnknown
        ) {
            return Ok(refusal);
        }
        let Some(control) = Self::refused_control(&refusal, observation, frames) else {
            return Ok(refusal);
        };
        if refusal.reason() == SemanticActionBindingError::TargetCovered
            && !covered.contains(&control)
        {
            covered.try_reserve(1).map_err(|_| crate::contract!())?;
            covered.push(control);
        }
        let count = match refused.iter_mut().find(|(known, _)| *known == control) {
            Some((_, count)) => {
                *count = count.saturating_add(1);
                *count
            }
            None => {
                refused.try_reserve(1).map_err(|_| crate::contract!())?;
                refused.push((control, 1));
                1
            }
        };
        Ok(if count == 1 {
            refusal
        } else {
            refusal.repeated()
        })
    }

    /// The refused proposal's control, by its role and name digest.
    pub(super) fn refused_control(
        refusal: &AgentProviderActionRefusal,
        observation: &SemanticObservation,
        frames: &[SemanticFrameJoin],
    ) -> Option<u64> {
        let target = refusal.target()?;
        let expected = observation.reference_frame(target).ok()?;
        let current = frames
            .iter()
            .find(|frame| frame.frame() == expected.frame())?;
        observation
            .resolve_node(target, current)
            .ok()
            .map(super::control_digest)
    }

    pub(super) async fn navigate_current(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        start: NavigationStart,
        observation: &SemanticObservation,
        captured_at: SemanticCaptureInstant,
        progress: AgentWorkTaskProgress,
    ) -> Result<
        (
            SemanticObservation,
            SemanticCaptureInstant,
            AgentWorkTaskProgress,
            Option<AgentBrowserProviderTurn>,
        ),
        AgentWorkFailure,
    > {
        state.check_task_contract()?;
        let (turn, follow) = match start {
            NavigationStart::Model(turn) => (Some(*turn), None),
            NavigationStart::Follow(follow) => (None, Some(*follow)),
        };
        // A model's load that fails goes back to the page it left.
        let departed = turn
            .as_ref()
            .and(state.native.retained.as_ref())
            .map(|browser| page_address(browser.binding().document()));
        let kind = turn
            .as_ref()
            .map_or(AgentBrowserToolKind::Navigate, |turn| {
                turn.turn.proposal().kind()
            });
        let is_back = kind == AgentBrowserToolKind::Back;
        if !matches!(
            kind,
            AgentBrowserToolKind::Navigate | AgentBrowserToolKind::Back
        ) || (is_back
            && (state.navigation_discovery.is_none()
                || !state.history_back
                || state.native.retained.is_none()))
        {
            return Err(crate::contract!());
        }
        let discovery = state.navigation_discovery.as_ref();
        let expected = if discovery.is_some() {
            AgentWorkTaskProgress::Continue
        } else {
            AgentWorkTaskProgress::ReadyForNavigation
        };
        if state.navigation_complete() || progress != expected {
            return Err(AgentWorkFailure::TaskPhase {
                expected: progress,
                proposed: kind,
            });
        }
        let proposed_target = if is_back {
            None
        } else if let Some((_, target)) = &follow {
            if !discovery.is_some_and(|scope| scope.is_site_session() && scope.admits(target)) {
                return Err(crate::contract!());
            }
            Some(target.clone())
        } else if let (Some(scope), Some(turn)) = (discovery, turn.as_ref()) {
            let AgentBrowserToolProposal::Navigate(target) = turn.turn.proposal() else {
                return Err(crate::contract!());
            };
            if !scope.admits(target) {
                return Err(crate::contract!());
            }
            Some(target.clone())
        } else {
            Some(
                state
                    .current_navigation_target()
                    .cloned()
                    .ok_or_else(|| crate::contract!())?,
            )
        };
        if !is_back
            && follow.is_none()
            && !matches!(
                (turn.as_ref().map(|turn| turn.turn.proposal()), proposed_target.as_ref()),
                (Some(AgentBrowserToolProposal::Navigate(proposed)), Some(target)) if proposed == target
            )
        {
            return Err(crate::contract!());
        }
        let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
        // Keep the original total ceiling and reserve the remaining exact route
        // proposals plus final extraction/mapping; no hop receives a new budget.
        let remaining_hops = if discovery.is_some() {
            1
        } else {
            state
                .navigation_length()
                .checked_sub(state.navigation_hops)
                .ok_or_else(|| crate::contract!())?
        };
        if usize::from(session.turns) + remaining_hops + 1 > usize::from(session.max_model_calls)
            || usize::try_from(
                session
                    .policy
                    .remaining_operations(session.lease.lease())
                    .map_err(|_| AgentWorkFailure::Browser(AgentBrowserProviderError::Authority))?,
            )
            .map_err(|_| crate::contract!())?
                < 2 * remaining_hops + 1
        {
            return Err(AgentWorkFailure::Browser(
                AgentBrowserProviderError::TurnLimit,
            ));
        }
        #[cfg(feature = "probe-harness")]
        let navigation_diagnostics = (
            proposed_target
                .as_ref()
                .is_some_and(|target| discovery.is_some_and(|scope| scope.admits(target))),
            proposed_target
                .as_ref()
                .is_some_and(|target| discovery.is_some_and(|scope| scope.departure() == target)),
        );
        state.refresh_account(worker, browser)?;
        if follow.is_none() {
            state
                .journal_mut()?
                .emit(AgentWorkEventKind::ToolProposed(kind))?;
        }
        state.native.check_control(worker, browser)?;
        state.check_task_contract()?;
        let id = state.native.identity.id();
        let automation = if let Some(retained) = &state.native.retained {
            let now = state
                .session
                .as_mut()
                .ok_or_else(|| crate::contract!())?
                .policy_now()
                .map_err(AgentWorkFailure::Browser)?;
            retained.automation_state(now)?
        } else {
            state
                .native
                .contexts()?
                .automation_state(id)
                .map_err(|_| AgentWorkFailure::Context)?
        };
        let legacy_operation = if state.native.retained.is_none() {
            Some(ContextOperationId::new(state.native.id()?).ok_or_else(|| crate::contract!())?)
        } else {
            None
        };
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let (continuation, following) = match (turn, follow) {
            (Some(turn), None) => (turn.into_tool_turn().into_parts().1, false),
            (None, Some((continuation, _))) => (continuation, true),
            _ => return Err(crate::contract!()),
        };
        if state.navigation_discovery.is_some() {
            let schema = state
                .extraction_schema
                .as_ref()
                .ok_or_else(|| crate::contract!())?;
            let read = read_semantic_observation_for_schema(
                observation,
                SemanticReadAuthority::Acknowledged(continuation.baseline()),
                captured_at,
                SemanticReadSensitivityLimit::PublicOnly,
                schema.extraction_read_budget(),
                schema,
            )
            .map_err(|error| AgentWorkFailure::Browser(AgentBrowserProviderError::Read(error)))?;
            state
                .retained_read_evidence
                .retain(&read, continuation.baseline())
                .map_err(|error| {
                    AgentWorkFailure::Browser(AgentBrowserProviderError::Read(error))
                })?;
        }
        let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
        let authorization = AgentNavigationAuthorizationRequest::new(
            session.lease.lease(),
            session.account,
            automation,
            now,
        );
        let permit = if is_back {
            session.policy.authorize_history_back(
                authorization,
                observation,
                continuation.baseline(),
            )
        } else if following {
            session.policy.authorize_follow(
                authorization,
                observation,
                continuation.baseline(),
                proposed_target.as_ref().ok_or_else(|| crate::contract!())?,
            )
        } else {
            session.policy.authorize_navigation(
                authorization,
                observation,
                continuation.baseline(),
                proposed_target.as_ref().ok_or_else(|| crate::contract!())?,
            )
        }
        .map_err(|error| {
            #[cfg(feature = "probe-harness")]
            let _ = writeln!(std::io::stderr(), "work-navigation: stage=authorize error={error:?} scope_admitted={} observed_link={} same_document={}",
                navigation_diagnostics.0,
                proposed_target.as_ref().is_some_and(|target| observation.frames().iter().flat_map(|frame| frame.nodes()).any(|node| node.role() == SemanticRole::Link && node.sensitivity() == SemanticSensitivity::Public && node.link_destination() == Some(target))),
                navigation_diagnostics.1);
            AgentWorkFailure::Browser(AgentBrowserProviderError::Navigation(error))
        })?;
        let target = permit.target().clone();
        let checkpoint = if is_back {
            continuation.retire_for_history_back(observation, &target, &session.config)
        } else if following {
            continuation.retire_for_follow(observation, &target, &session.config)
        } else {
            continuation.retire_for_navigation(observation, &target, &session.config)
        }
        .map_err(|_| AgentWorkFailure::Browser(crate::continuation!()));
        let checkpoint = match checkpoint {
            Ok(checkpoint) => checkpoint,
            Err(failure) => {
                session
                    .policy
                    .cancel_navigation(permit)
                    .map_err(|_| crate::contract!())?;
                return Err(failure);
            }
        };
        let prepared = if let Some(retained) = &mut state.native.retained {
            retained.prepare_navigation(automation.context(), now)
        } else {
            let op = legacy_operation.ok_or_else(|| crate::contract!())?;
            state
                .native
                .contexts()?
                .begin_navigation(id, op)
                .map_err(|_| AgentWorkFailure::Context)
        };
        let operation = match prepared {
            Ok(operation) => operation,
            Err(_) => {
                state
                    .session
                    .as_mut()
                    .ok_or_else(|| crate::contract!())?
                    .policy
                    .cancel_navigation(permit)
                    .map_err(|error| {
                        AgentWorkFailure::Browser(AgentBrowserProviderError::Navigation(error))
                    })?;
                return Err(AgentWorkFailure::Context);
            }
        };
        // The old registry join is now irrevocably stale, before native dispatch.
        state.native.snapshot_generation = None;
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let active = session
            .policy
            .dispatch_navigation(permit, operation, now)
            .map_err(|error| {
                #[cfg(feature = "probe-harness")]
                let _ = writeln!(
                    std::io::stderr(),
                    "work-navigation: stage=dispatch error={error:?}"
                );
                AgentWorkFailure::Browser(AgentBrowserProviderError::Navigation(error))
            });
        let active = match active {
            Ok(active) => active,
            Err(error) => {
                if let Some(retained) = &mut state.native.retained {
                    let _ = retained.cancel_navigation_preparation();
                }
                return Err(error);
            }
        };
        session.navigation = Some(active);
        let active = session
            .navigation
            .as_ref()
            .ok_or_else(|| crate::contract!())?;
        state.native.operation = Some(operation);
        let dispatch = if let Some(retained) = &mut state.native.retained {
            if is_back {
                retained.dispatch_history_back(active)
            } else {
                retained.dispatch_navigation(active)
            }
        } else {
            let request = active.native_request().map_err(|_| crate::contract!())?;
            browser.dispatch(ContextNativeRequest::Navigate(request))
        };
        let refusal = match dispatch {
            ContextDispatch::Rejected(failure) => Some(failure),
            ContextDispatch::Unsupported => Some(ContextPortFailure::Unsupported),
            ContextDispatch::Scheduled => None,
        };
        if let Some(failure) = refusal {
            // The port's exact decision is terminal evidence even when a later
            // clock/audit operation fails. It is not a callback or new permit.
            session.navigation_refusal =
                Some(super::super::AgentBrowserNavigationDispatchRefusal { operation, failure });
        }
        // Both original owners exist before dispatch. Audit/progress recording
        // is fallible, so do it before processing callbacks, but only after the
        // port has either accepted a real callback debt or explicitly refused.
        // A local journal error must never strand an undispatched successor.
        let journal_failure = session
            .journal
            .as_mut()
            .ok_or_else(|| crate::contract!())
            .and_then(|journal| journal.navigation_active(active))
            .err();
        if let Some(failure) = refusal {
            state.native.operation = None;
            if state.native.retained.is_none() {
                state
                    .native
                    .contexts()?
                    .settle_navigation(id, operation, ContextSettlement::Refused)
                    .map_err(|_| AgentWorkFailure::Context)?;
            }
            let terminal = state
                .session
                .as_mut()
                .ok_or_else(|| crate::contract!())?
                .settle_navigation_refusal();
            let terminal = match terminal {
                Ok(terminal) => terminal,
                Err(failure) => return Err(journal_failure.unwrap_or(failure)),
            };
            return Err(journal_failure
                .or(terminal.journal_failure)
                .unwrap_or(AgentWorkFailure::Native(failure)));
        }
        if let Some(failure) = journal_failure {
            return Err(failure);
        }
        let receipt = match state.native.next_navigation_event(worker, browser).await? {
            AgentRuntimeEvent::NativeTerminal(ContextNativeEvent::NavigationSettled(terminal))
                if terminal.operation() == operation =>
            {
                let result = state
                    .session
                    .as_mut()
                    .ok_or_else(|| crate::contract!())?
                    .settle_navigation_terminal(&terminal);
                let terminal_record = match result {
                    Ok(terminal_record) => terminal_record,
                    Err(failure) => {
                        state.native.retain(AgentRuntimeEvent::NativeTerminal(
                            ContextNativeEvent::NavigationSettled(terminal),
                        ))?;
                        return Err(failure);
                    }
                };
                state.native.operation = None;
                if state.native.retained.is_none() {
                    state
                        .native
                        .contexts()?
                        .settle_navigation(
                            id,
                            operation,
                            if terminal.outcome().is_ok() {
                                ContextSettlement::Applied
                            } else {
                                ContextSettlement::Refused
                            },
                        )
                        .map_err(|_| AgentWorkFailure::Context)?;
                }
                if let Some(failure) = terminal_record.journal_failure {
                    return Err(failure);
                }
                let receipt = terminal_record.receipt;
                if let AgentNavigationSettlement::Failed(failure) = receipt.settlement() {
                    return Err(AgentWorkFailure::Native(failure));
                }
                receipt
            }
            event => {
                state.native.retain(event)?;
                return Err(AgentWorkFailure::Mailbox);
            }
        };
        state.native.check_control(worker, browser)?;
        state.check_task_contract()?;
        if receipt.hop() != state.navigation_hops {
            return Err(crate::contract!());
        }
        if state.navigation_discovery.is_some() {
            state
                .retained_read_evidence
                .advance_after_navigation(receipt)
                .map_err(|error| {
                    AgentWorkFailure::Browser(AgentBrowserProviderError::Read(error))
                })?;
        }
        state.navigation_hops += 1;
        state.covered_controls.clear();
        state.scroll_streak = 0;
        state.look_streak = 0;
        state.moved_on();
        let arrived = state
            .native
            .retained
            .as_ref()
            .map(|browser| page_address(browser.binding().document()));
        match (departed, proposed_target.as_ref().map(page_address)) {
            (Some(departed), Some(target))
                if arrived.as_ref() == Some(&departed) && target != departed =>
            {
                state.setbacks.count(
                    super::Setback::Navigation,
                    address_digest(&target),
                    AgentWorkFailure::Native(ContextPortFailure::NativeRefused),
                )?;
            }
            _ => state.setbacks.progressed(),
        }
        let fresh = Self::fit_model_observation(
            Box::pin(Self::observe_after_navigation(state, worker, browser)).await?,
        )?;
        let now = state
            .session
            .as_mut()
            .ok_or_else(|| crate::contract!())?
            .policy_now()
            .map_err(AgentWorkFailure::Browser)?;
        let account = state.task.attest_account(operation.context(), now);
        state.native.check_control(worker, browser)?;
        state
            .session
            .as_mut()
            .ok_or_else(|| crate::contract!())?
            .accept_arrival(receipt, account?)
            .map_err(AgentWorkFailure::Browser)?;
        // A move can land on a sign-in, code or bot-check page: the person
        // gets it before any model call is spent on a page it cannot pass.
        let (fresh, held) = if state.human_request
            && (state.task.human_wall(&fresh).is_some() || looks_like_human_challenge(&fresh))
        {
            Box::pin(Self::settle_human_challenge(state, worker, browser, fresh)).await?
        } else {
            (fresh, false)
        };
        // An offer the site laid over the page it moved to (a location or
        // newsletter popover) is closed as on the first page.
        let fresh = match (held, state.task.overlay_dismissal(&fresh)) {
            (false, Some(target))
                if state.actions_before_extraction
                    && state
                        .navigation_discovery
                        .as_ref()
                        .is_some_and(AgentNavigationDiscovery::is_site_session) =>
            {
                Box::pin(Self::close_overlay(state, worker, browser, fresh, target)).await?
            }
            _ => fresh,
        };
        let captured_at = SemanticCaptureInstant::from_millis(
            state
                .journal_mut()?
                .clock
                .now()
                .map_err(|_| crate::contract!())?
                .millis(),
        );
        let progress = state.task_progress(&fresh)?;
        if held {
            return Ok((fresh, captured_at, progress, None));
        }
        let navigation_available = !state.navigation_complete();
        let action_authority = state
            .session
            .as_ref()
            .ok_or_else(|| crate::contract!())?
            .config
            .permits_tool(AgentBrowserToolKind::Act)
            .then(|| state.action_authority(&fresh))
            .transpose()?;
        let now = state
            .session
            .as_mut()
            .ok_or_else(|| crate::contract!())?
            .policy_now()
            .map_err(AgentWorkFailure::Browser)?;
        let account = state.task.attest_account(operation.context(), now);
        state.native.check_control(worker, browser)?;
        state.check_task_contract()?;
        let account = account?;
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let turn = Self::provider(
            &mut state.native,
            worker,
            browser,
            session.cancellation.clone(),
            session.continue_after_navigation(
                checkpoint,
                receipt,
                &fresh,
                account,
                action_authority.as_ref(),
                navigation_available,
            ),
        )
        .await?;
        Ok((fresh, captured_at, progress, Some(turn)))
    }
}

/// A document's address without the route an app keeps in its fragment.
pub(super) fn page_address(target: &ContextNavigationTarget) -> String {
    let mut url = target.as_url().clone();
    url.set_fragment(None);
    url.into()
}

pub(super) fn address_digest(address: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    address.hash(&mut hasher);
    hasher.finish()
}

impl AgentWorkController {
    /// A document a move just committed may still be an application shell of
    /// loading placeholders. A few bounded looks give it time to fill in, so
    /// the model is not paid to read, or snapshot again, an empty page. Like
    /// the first look's wait, this is scheduling only, never readiness proof.
    async fn observe_after_navigation(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<SemanticObservation, AgentWorkFailure> {
        const DELAYS_MS: [u64; 6] = [250, 500, 1000, 1500, 2000, 2000];
        let mut observation = Self::observe(state, worker, browser).await?;
        let started = Instant::now();
        // A site page that has rendered next to nothing yet (an app booting,
        // results still on their way) is waited out like a first page.
        let site = state
            .navigation_discovery
            .as_ref()
            .is_some_and(AgentNavigationDiscovery::is_site_session);
        for delay in DELAYS_MS {
            // A results page still saying it loads them gets longer.
            let announced = site && super::announces_loading(&observation);
            let deadline = started
                .checked_add(Duration::from_secs(if announced { 8 } else { 4 }))
                .ok_or(AgentWorkFailure::Deadline)?
                .min(state.native.deadline);
            if !(has_dominant_loading_placeholders(&observation)
                || announced
                || (site && super::unrendered(&observation)))
                || state
                    .native
                    .retained
                    .as_ref()
                    .is_some_and(|browser| !browser.allows_readiness_retry())
            {
                break;
            }
            let wake = Instant::now()
                .checked_add(Duration::from_millis(delay))
                .ok_or(AgentWorkFailure::Deadline)?;
            if wake >= deadline {
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
            state.check_task_contract()?;
            // The arrival's account is attested once, after its settled look.
            state.native.check_control(worker, browser)?;
            let next = Self::observe(state, worker, browser).await?;
            // A page that loaded its own site meanwhile is a new document:
            // its look continues the wait.
            if !super::replaced_document(&observation, &next) {
                validate_initial_readiness_successor(&observation, &next)?;
            }
            observation = next;
        }
        Ok(observation)
    }
}

impl AgentBrowserSession {
    pub(super) fn settle_navigation_terminal(
        &mut self,
        terminal: &ContextNavigationSettlement,
    ) -> Result<NavigationTerminal, AgentWorkFailure> {
        if self.navigation_refusal.is_some() {
            return Err(crate::contract!());
        }
        let now = self.policy_now().map_err(AgentWorkFailure::Browser)?;
        let active = self.navigation.as_ref().ok_or_else(|| crate::contract!())?;
        let receipt = self
            .policy
            .settle_navigation(active, terminal, now)
            .map_err(|error| {
                #[cfg(feature = "probe-harness")]
                let _ = writeln!(
                    std::io::stderr(),
                    "work-navigation: stage=settle error={error:?}"
                );
                AgentWorkFailure::Browser(AgentBrowserProviderError::Navigation(error))
            })?;
        Ok(self.record_navigation_terminal(receipt))
    }

    pub(super) fn settle_navigation_refusal(
        &mut self,
    ) -> Result<NavigationTerminal, AgentWorkFailure> {
        let refusal = self.navigation_refusal.ok_or_else(|| crate::contract!())?;
        if self
            .navigation
            .as_ref()
            .map(AgentActiveNavigation::operation)
            != Some(refusal.operation)
        {
            return Err(crate::contract!());
        }
        let now = self.policy_now().map_err(AgentWorkFailure::Browser)?;
        let active = self.navigation.as_ref().ok_or_else(|| crate::contract!())?;
        let receipt = self
            .policy
            .refuse_navigation_dispatch(active, refusal.failure, now)
            .map_err(|error| {
                AgentWorkFailure::Browser(AgentBrowserProviderError::Navigation(error))
            })?;
        self.navigation_refusal.take();
        Ok(self.record_navigation_terminal(receipt))
    }

    fn record_navigation_terminal(
        &mut self,
        receipt: AgentNavigationReceipt,
    ) -> NavigationTerminal {
        self.navigation_receipt = Some(receipt);
        self.navigation.take();
        let journal_failure = self
            .journal
            .as_mut()
            .ok_or_else(|| crate::contract!())
            .and_then(|journal| journal.navigation_settled(receipt))
            .err();
        if journal_failure.is_none() {
            self.navigation_receipt.take();
        }
        NavigationTerminal {
            receipt,
            journal_failure,
        }
    }

    async fn continue_after_navigation(
        &mut self,
        checkpoint: AgentProviderNavigationCheckpoint,
        receipt: AgentNavigationReceipt,
        observation: &SemanticObservation,
        account: AgentContextAccountBinding,
        action_authority: Option<&AgentProviderActionAuthority>,
        navigation_available: bool,
    ) -> Result<AgentBrowserProviderTurn, AgentBrowserProviderError> {
        self.check_live()?;
        if self.turns >= self.max_model_calls {
            return Err(AgentBrowserProviderError::TurnLimit);
        }
        let arrived = self.arrived.take() == Some(receipt.operation());
        if arrived {
            self.validate_account_update(account, receipt.operation().context())?;
        } else {
            self.validate_arrival(receipt, account)?;
        }
        let provisional = self.next_model_call_request()?;
        let request = AgentModelCallRequest::new(
            provisional.id(),
            provisional.lease(),
            account,
            provisional.budget(),
            provisional.now(),
        );
        checkpoint
            .validate_successor_with_action_authority(
                receipt,
                observation,
                request,
                &self.config,
                action_authority,
            )
            .map_err(|_| crate::continuation!())?;
        self.history_depth = match receipt.kind() {
            AgentNavigationKind::Load => self
                .history_depth
                .checked_add(1)
                .ok_or_else(|| crate::continuation!())?,
            AgentNavigationKind::HistoryBack => self
                .history_depth
                .checked_sub(1)
                .ok_or_else(|| crate::continuation!())?,
        };
        self.config = self
            .config
            .clone()
            .with_navigation_available(navigation_available)
            .with_history_back_available(self.history_depth > 0);
        let payload = encode_semantic_observation(
            observation,
            SemanticModelEncodingBudget::INITIAL_PROVIDER_EXACT_CONSERVATIVE,
        )
        .and_then(|encoded| encoded.admit_conservative_utf8(self.config.tokenizer()))
        .map_err(AgentBrowserProviderError::InitialEncoding)?;
        let objective = self
            .objective
            .as_ref()
            .ok_or_else(|| crate::continuation!())?;
        let prepared = match action_authority {
            Some(authority) => AgentPreparedObservationRequest::try_openai_for_provider_exact_count_with_action_authority(
                &mut self.policy,
                request,
                observation,
                payload,
                objective,
                self.config.clone(),
                authority,
            ),
            None => AgentPreparedObservationRequest::try_openai_for_provider_exact_count(
                &mut self.policy,
                request,
                observation,
                payload,
                objective,
                self.config.clone(),
            ),
        }
        .map_err(AgentBrowserProviderError::from_request)?;
        if account != self.account {
            self.account_attestations.push(account.attestation());
            self.account = account;
        }
        self.drive(prepared.into_transport_input(), Some(observation))
            .await
    }

    fn validate_arrival(
        &mut self,
        receipt: AgentNavigationReceipt,
        account: AgentContextAccountBinding,
    ) -> Result<(), AgentBrowserProviderError> {
        if receipt.source() != self.account.context()
            || account.observed_at() < receipt.settled_at()
            || account.attestation() == self.account.attestation()
        {
            return Err(AgentBrowserProviderError::Account(
                super::super::AgentBrowserAccountError::ContextChanged,
            ));
        }
        self.validate_account_update(account, receipt.operation().context())
    }

    /// Moves the account to the document a settled navigation arrived on
    /// before anything looks at it again: a bot check or sign-in wall found
    /// on arrival samples the account of the new document, not the old.
    pub(super) fn accept_arrival(
        &mut self,
        receipt: AgentNavigationReceipt,
        account: AgentContextAccountBinding,
    ) -> Result<(), AgentBrowserProviderError> {
        self.check_live()?;
        self.validate_arrival(receipt, account)?;
        self.account_attestations.push(account.attestation());
        self.account = account;
        self.arrived = Some(receipt.operation());
        Ok(())
    }
}

impl WorkJournal {
    fn navigation_active(
        &mut self,
        active: &AgentActiveNavigation,
    ) -> Result<(), AgentWorkFailure> {
        self.supervisor
            .record_active_navigation(
                self.execution.as_ref().ok_or_else(|| crate::contract!())?,
                active,
            )
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.record()
    }

    fn navigation_settled(
        &mut self,
        receipt: AgentNavigationReceipt,
    ) -> Result<(), AgentWorkFailure> {
        self.accounting
            .record_navigation_receipt(receipt)
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.supervisor
            .record_navigation_result(
                self.execution.as_ref().ok_or_else(|| crate::contract!())?,
                receipt,
            )
            .map_err(|_| AgentWorkFailure::Accounting)?;
        self.record()
    }
}
