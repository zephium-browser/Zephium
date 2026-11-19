use super::*;

const MAX_UNCHANGED_DECISION_OBSERVATIONS: u8 = 3;
/// Cheap typed re-observations of one page before the read stops asking the
/// recommended backend. Three covers the recorded product pages: expanding one
/// collapsed specification disclosure plus two screens of scrolling, and it
/// bounds the added work at three native operations and three sub-second
/// batches, well inside one read's existing action and call allowances.
const MAX_READ_REOBSERVATIONS: u8 = 3;
/// Looks at a page still loading after a step, half a second apart.
const MAX_SETTLING_LOOKS: u8 = 16;

impl AgentWorkController {
    pub(super) async fn run_decision_actions(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        mut observation: SemanticObservation,
        mut captured_at: SemanticCaptureInstant,
        mut progress: AgentWorkTaskProgress,
    ) -> Result<
        (
            SemanticObservation,
            SemanticCaptureInstant,
            AgentWorkTaskProgress,
            bool,
        ),
        AgentWorkFailure,
    > {
        let mut unchanged = 0u8;
        let mut reobservations = 0u8;
        let mut consent_dismissed = false;
        let mut disclosure_toggled = false;
        let mut pending: Option<Box<zephium_agentic::DecisionReadSelection>> = None;
        // The unfinished look with the most located values before Rust moved
        // the page; a later look may finish the read joined with it.
        let mut earlier_look: Option<Box<zephium_agentic::DecisionReadSelection>> = None;
        // The address the one-document gate admitted, never one from page text.
        let document = state.native.retained.as_ref().map(|browser| {
            zephium_agentic::untracked_document_address(browser.binding().document())
        });
        loop {
            state.check_task_contract()?;
            state.native.check_control(worker, browser)?;
            let top = observation.frames().first();
            state
                .journal_mut()?
                .emit(AgentWorkEventKind::ObservationFacts {
                    nodes: observation.node_count(),
                    text_bytes: observation.total_text_bytes(),
                    dialogs: top.map_or(0, |frame| {
                        let dialogs = frame
                            .nodes()
                            .iter()
                            .filter(|node| node.role() == SemanticRole::Dialog)
                            .count();
                        u8::try_from(dialogs).unwrap_or(u8::MAX)
                    }),
                    complete: top.is_some_and(|frame| {
                        frame.completeness() == SemanticCompleteness::Complete
                    }),
                })?;
            let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
            if session.decisions.is_none() || progress == AgentWorkTaskProgress::Complete {
                break;
            }
            let actions_only = session.actions_only_decisions();
            if state
                .decision_answers
                .as_ref()
                .is_some_and(|answers| !answers.matches_observation(&observation, session.account))
            {
                state.decision_answers = None;
            }
            if state.decision_answers.is_none() {
                if actions_only {
                    if state.task.human_wall(&observation).is_some()
                        || looks_like_human_challenge(&observation)
                    {
                        let (fresh, held) =
                            Self::settle_human_challenge(state, worker, browser, observation)
                                .await?;
                        observation = fresh;
                        captured_at = SemanticCaptureInstant::from_millis(
                            state
                                .session
                                .as_mut()
                                .ok_or_else(|| crate::contract!())?
                                .policy_now()
                                .map_err(AgentWorkFailure::Browser)?
                                .millis(),
                        );
                        progress = state.task_progress(&observation)?;
                        if held {
                            return Ok((observation, captured_at, progress, true));
                        }
                    }
                    state.refresh_account(worker, browser)?;
                    let authority = state.action_authority(&observation)?;
                    let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                    if session.account.account() != AgentAccountScope::Anonymous {
                        break;
                    }
                    state.decision_answers = Self::provider(
                        &mut state.native,
                        worker,
                        browser,
                        session.cancellation.clone(),
                        session.decide_observation(&observation, &authority, None, false),
                    )
                    .await?;
                } else if !state.human_request {
                    if Self::classify_human_challenge(
                        state,
                        worker,
                        browser,
                        &observation,
                        reobservations > 0,
                    )
                    .await?
                    {
                        break;
                    }
                } else {
                    let (fresh, challenged) =
                        Self::settle_human_challenge(state, worker, browser, observation).await?;
                    observation = fresh;
                    captured_at = SemanticCaptureInstant::from_millis(
                        state
                            .session
                            .as_mut()
                            .ok_or_else(|| crate::contract!())?
                            .policy_now()
                            .map_err(AgentWorkFailure::Browser)?
                            .millis(),
                    );
                    progress = state.task_progress(&observation)?;
                    if challenged {
                        return Ok((observation, captured_at, progress, true));
                    }
                }
                if progress == AgentWorkTaskProgress::Complete {
                    break;
                }
            }
            let Some(mut answers) = state.decision_answers.take() else {
                if actions_only {
                    state
                        .journal_mut()?
                        .emit(AgentWorkEventKind::DecisionPlanner(
                            AgentWorkDecisionRoute::Unavailable,
                        ))?;
                }
                break;
            };
            // A catalog read finishes from the records it found, after at most
            // the bounded scrolls when fewer than requested are in view and
            // more are confidently below. Only finding none leaves the planner.
            let mut rows_scroll = None;
            if let Some(schema) = state
                .extraction_schema
                .clone()
                .filter(|schema| !actions_only && schema.is_row_collection())
            {
                let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                if let Some(discovery) = answers
                    .take_row_discovery(&observation, session.account, &schema)
                    .map_err(AgentWorkFailure::DecisionRead)?
                {
                    let more_below = answers
                        .take_more_below(&observation, session.account)
                        .map_err(AgentWorkFailure::DecisionMoreBelow)?;
                    if discovery.rows() < discovery.max_items()
                        && more_below == Some(true)
                        && reobservations < MAX_READ_REOBSERVATIONS
                        && state.actions_before_extraction
                        && session.next_action <= session.max_actions
                    {
                        rows_scroll = answers
                            .take_reobservation_scroll(&observation, session.account)
                            .map_err(AgentWorkFailure::DecisionOperation)?;
                    }
                    if rows_scroll.is_none() {
                        // Boxed: the read loop's future must stay well inside
                        // the runtime worker's stack.
                        if Box::pin(Self::finish_rows(
                            state,
                            worker,
                            browser,
                            &observation,
                            captured_at,
                            discovery,
                        ))
                        .await?
                        {
                            return Ok((
                                observation,
                                captured_at,
                                AgentWorkTaskProgress::Complete,
                                false,
                            ));
                        }
                        break;
                    }
                }
            }
            let mut gap: Option<Box<zephium_agentic::DecisionReadSelection>> = None;
            let mut candidate: Option<Box<zephium_agentic::DecisionReadSelection>> = None;
            if let Some(schema) = state.extraction_schema.clone().filter(|_| !actions_only) {
                let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                if let Some((selection, ready)) = answers
                    .take_read_progress_retaining_evidence(
                        &observation,
                        session.account,
                        &schema,
                        captured_at,
                        &mut state.retained_read_evidence,
                    )
                    .map_err(AgentWorkFailure::DecisionRead)?
                {
                    // A look that located everything except optional values it
                    // confidently found absent finishes once a later look
                    // confirms each absence; those columns publish unknown.
                    // A required column never settles this way.
                    // A look that lacks values an earlier look located, and
                    // together with it settles every column, finishes joined:
                    // a scroll must not lose a value the first look found.
                    let joined = (!ready)
                        .then(|| {
                            [earlier_look.as_deref(), pending.as_deref()]
                                .into_iter()
                                .flatten()
                                .find_map(|earlier| {
                                    selection.joined_with(
                                        earlier,
                                        reobservations >= MAX_READ_REOBSERVATIONS,
                                    )
                                })
                        })
                        .flatten();
                    let confirmed = pending.take_if(|earlier| {
                        !ready && joined.is_none() && selection.confirms_absence(earlier)
                    });
                    if ready || confirmed.is_some() || joined.is_some() {
                        state.refresh_account(worker, browser)?;
                        let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
                        // The earlier look's values come from the evidence it
                        // retained, cited under the current observation.
                        let evidence = std::mem::take(&mut state.retained_read_evidence);
                        // A confirming look that itself located everything the
                        // earlier look did is read directly, with focused
                        // generation if needed; otherwise the earlier look's
                        // copies are cited: a scroll must not lose a value
                        // the first look found.
                        let prepared = match joined.or(confirmed
                            .filter(|earlier| {
                                !selection.awaits_absence() || !selection.keeps_located(earlier)
                            })
                            .map(|earlier| *earlier))
                        {
                            Some(earlier) => earlier.prepare_confirmed(
                                &observation,
                                session.account,
                                captured_at,
                                document.as_ref(),
                                &evidence,
                            ),
                            None => selection.prepare(
                                &observation,
                                session.account,
                                captured_at,
                                document.as_ref(),
                            ),
                        };
                        // A clipped or unavailable exact source leaves the normal planner available.
                        let located = match prepared {
                            Ok(located) => located,
                            Err(_) => {
                                state.retained_read_evidence = evidence;
                                break;
                            }
                        };
                        // Boxed: the read loop's future must stay well inside the
                        // runtime worker's stack.
                        Box::pin(Self::finish_located_read(state, worker, browser, located))
                            .await?;
                        state.retained_read_evidence = evidence;
                        return Ok((
                            observation,
                            captured_at,
                            AgentWorkTaskProgress::Complete,
                            false,
                        ));
                    }
                    if selection.awaits_absence() {
                        candidate = Some(Box::new(selection));
                    } else {
                        gap = Some(Box::new(selection));
                    }
                }
            }
            let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
            let more_below = answers
                .take_more_below(&observation, session.account)
                .map_err(AgentWorkFailure::DecisionMoreBelow)?;
            let actionable = state.actions_before_extraction
                && progress == AgentWorkTaskProgress::Continue
                && session.next_action <= session.max_actions
                && session
                    .policy
                    .remaining_operations(session.lease.lease())
                    .map_err(|_| crate::contract!())?
                    >= 3;
            // A consent dialog the wall head recognised is dismissed first by
            // the task's own recipe, once per read, with no provider choosing.
            // Then a settled action head is already paid for, so it is
            // preferred. Otherwise one cheap typed re-observation runs before
            // any generated value: Rust scrolls an already offered region and
            // asks the recommended backend again over value heads only.
            let consent = if actionable && !consent_dismissed {
                answers
                    .take_consent_dismissal(&observation, session.account, || {
                        state.task.consent_dismissal(&observation)
                    })
                    .map_err(AgentWorkFailure::DecisionOperation)?
            } else {
                None
            };
            consent_dismissed |= consent.is_some();
            let pressing_consent = consent.is_some() && rows_scroll.is_none();
            let proposed = match consent {
                Some(consent) => Some(consent),
                None if actionable => answers
                    .take_action_selection(&observation, session.account)
                    .map_err(AgentWorkFailure::DecisionOperation)?,
                None => None,
            };
            if proposed.as_ref().is_some_and(|selection| {
                matches!(selection.operation(), DecisionOperation::Scroll(_))
            }) && more_below != Some(true)
            {
                if actions_only {
                    state
                        .journal_mut()?
                        .emit(AgentWorkEventKind::DecisionPlanner(
                            AgentWorkDecisionRoute::ScrollUnverified,
                        ))?;
                }
                break;
            }
            if actions_only && proposed.is_none() {
                state
                    .journal_mut()?
                    .emit(AgentWorkEventKind::DecisionPlanner(
                        AgentWorkDecisionRoute::Uncertain,
                    ))?;
                break;
            }
            // Rust's own re-observation is optimistic: if the page refuses it,
            // the read continues on the existing planner path instead of ending.
            let mut reobserving = false;
            let selection = match rows_scroll.or(proposed) {
                Some(selection)
                    if matches!(selection.operation(), DecisionOperation::Scroll(_))
                        && reobservations < MAX_READ_REOBSERVATIONS
                        && state
                            .extraction_schema
                            .as_ref()
                            .is_some_and(SemanticExtractionSchema::is_row_collection) =>
                {
                    reobservations = reobservations.saturating_add(1);
                    reobserving = true;
                    selection
                }
                Some(selection) => selection,
                None => {
                    let reobserve = actionable
                        && reobservations < MAX_READ_REOBSERVATIONS
                        && (gap.is_some() || candidate.is_some() || pending.is_some());
                    // A value still unlocated may sit in a collapsed
                    // disclosure the task's closed list names: once it is in
                    // view, Rust toggles it once before scrolling the page.
                    let disclosure = if reobserve && !disclosure_toggled {
                        match state.task.detail_disclosure(&observation) {
                            Some(operation) => {
                                disclosure_toggled = true;
                                answers
                                    .take_code_owned(&observation, session.account, operation)
                                    .map_err(AgentWorkFailure::DecisionOperation)?
                            }
                            None => None,
                        }
                    } else {
                        None
                    };
                    let scroll = match disclosure {
                        Some(disclosure) => Some(disclosure),
                        None if reobserve => answers
                            .take_reobservation_scroll(&observation, session.account)
                            .map_err(AgentWorkFailure::DecisionOperation)?,
                        None => None,
                    };
                    match scroll {
                        Some(scroll) => {
                            reobservations = reobservations.saturating_add(1);
                            reobserving = true;
                            if gap.as_ref().is_some_and(|look| {
                                earlier_look
                                    .as_ref()
                                    .is_none_or(|earlier| look.located() >= earlier.located())
                            }) {
                                earlier_look = gap.take();
                            }
                            gap = None;
                            scroll
                        }
                        None => {
                            // The recommended backend stopped short. One focused
                            // generation over the located neighbourhood finishes
                            // the read; the general planner is not asked here.
                            if let Some(gap) = gap.filter(|gap| {
                                gap.unresolved() > 0
                                    && gap.located() > 0
                                    && gap.unresolved_are_generated()
                            }) {
                                state.refresh_account(worker, browser)?;
                                let session =
                                    state.session.as_ref().ok_or_else(|| crate::contract!())?;
                                let located = match gap.prepare_completing(
                                    &observation,
                                    session.account,
                                    captured_at,
                                    document.as_ref(),
                                ) {
                                    Ok(located) => located,
                                    Err(_) => break,
                                };
                                // Boxed: the read loop's future must stay well
                                // inside the runtime worker's stack.
                                Box::pin(Self::finish_located_read(
                                    state, worker, browser, located,
                                ))
                                .await?;
                                return Ok((
                                    observation,
                                    captured_at,
                                    AgentWorkTaskProgress::Complete,
                                    false,
                                ));
                            }
                            break;
                        }
                    }
                }
            };
            let _ = gap;
            let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
            let Some(recipe) = state
                .task
                .decision_action_recipe(selection.operation(), &observation)?
            else {
                if actions_only {
                    let reason = match selection.operation() {
                        DecisionOperation::Type(_) => AgentWorkDecisionRoute::TextRequired,
                        DecisionOperation::Done => AgentWorkDecisionRoute::CompletionUnverified,
                        DecisionOperation::Blocked => AgentWorkDecisionRoute::BlockUnverified,
                        _ => AgentWorkDecisionRoute::Unsupported,
                    };
                    state
                        .journal_mut()?
                        .emit(AgentWorkEventKind::DecisionPlanner(reason))?;
                }
                break;
            };
            let progress_key = recipe.progress_fingerprint(&observation);
            if actions_only && state.no_progress.refuses(progress_key) {
                state
                    .journal_mut()?
                    .emit(AgentWorkEventKind::DecisionPlanner(
                        AgentWorkDecisionRoute::NoProgress,
                    ))?;
                break;
            }
            let frames = observation
                .frames()
                .iter()
                .map(|frame| frame.frame().clone())
                .collect::<Vec<_>>();
            let batch = SemanticActionBatchId::new(session.next_action)
                .ok_or_else(|| crate::contract!())?;
            // A step Rust takes on answers an earlier look gave (a scroll to
            // find more rows, a disclosure) may act on a look no model has
            // been sent: the look taints the run as a model's reading would,
            // as for Rust's own consent press.
            let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
            let account = session.account;
            session
                .policy
                .admit_owned_observation(&observation, account)
                .map_err(|_| crate::contract!())?;
            let proposal = match crate::AgentBrowserActionProposal::bind_decision(
                selection,
                recipe,
                &observation,
                &frames,
                batch,
            ) {
                Ok(proposal) => proposal,
                // Nothing was dispatched; the full planner may supply a supported recipe.
                Err(_) => break,
            };
            let assessment = match state.task.assess_observed(proposal.action(), &observation) {
                Ok(assessment) => assessment,
                Err(AgentWorkFailure::ActionDenied) => {
                    if actions_only {
                        state
                            .journal_mut()?
                            .emit(AgentWorkEventKind::DecisionPlanner(
                                AgentWorkDecisionRoute::Policy,
                            ))?;
                    }
                    break;
                }
                Err(AgentWorkFailure::EffectRequired(_)) if actions_only => {
                    state
                        .journal_mut()?
                        .emit(AgentWorkEventKind::DecisionPlanner(
                            AgentWorkDecisionRoute::Policy,
                        ))?;
                    break;
                }
                Err(error) => return Err(error),
            };
            state
                .journal_mut()?
                .emit(AgentWorkEventKind::ToolProposed(AgentBrowserToolKind::Act))?;
            Self::retain_action_read_evidence(state, &proposal, &observation, captured_at)?;
            if actions_only {
                state.no_progress.remember(progress_key);
            }
            let executed = Self::execute_prepared_action(
                state,
                worker,
                browser,
                proposal,
                assessment,
                &observation,
            )
            .await;
            let (current, at, transition) = match executed {
                Ok(settled) => settled,
                Err(AgentWorkFailure::Browser(error))
                    if actions_only
                        && matches!(
                            error,
                            AgentBrowserProviderError::Action(_)
                                | AgentBrowserProviderError::ActionRejected(_)
                                | AgentBrowserProviderError::ActionUnverified
                        ) =>
                {
                    let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                    let _ = session.take_rejected_refusal();
                    if !session
                        .close_failed_owned_action()
                        .map_err(AgentWorkFailure::Browser)?
                    {
                        return Err(AgentWorkFailure::Browser(error));
                    }
                    state
                        .journal_mut()?
                        .emit(AgentWorkEventKind::DecisionPlanner(
                            AgentWorkDecisionRoute::NativeRefused,
                        ))?;
                    let looked = Self::look_begins(state)?;
                    observation = Box::pin(Self::observe(state, worker, browser)).await?;
                    captured_at = SemanticCaptureInstant::from_millis(looked.millis());
                    progress = state.task_progress(&observation)?;
                    break;
                }
                // A consent press commits nothing: the page that saved the
                // choice and loaded itself again under it is looked at anew.
                Err(AgentWorkFailure::Browser(
                    error @ (AgentBrowserProviderError::Action(_)
                    | AgentBrowserProviderError::ActionRejected(_)
                    | AgentBrowserProviderError::ActionUnverified),
                )) if pressing_consent => {
                    let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
                    let _ = session.take_rejected_refusal();
                    if !session
                        .close_failed_owned_action()
                        .map_err(AgentWorkFailure::Browser)?
                    {
                        return Err(AgentWorkFailure::Browser(error));
                    }
                    let looked;
                    (observation, looked) =
                        Box::pin(Self::observe_settled(state, worker, browser)).await?;
                    captured_at = SemanticCaptureInstant::from_millis(looked.millis());
                    progress = state.task_progress(&observation)?;
                    break;
                }
                Err(AgentWorkFailure::Browser(AgentBrowserProviderError::Action(_)))
                    if reobserving =>
                {
                    break
                }
                Err(error) => return Err(error),
            };
            let batch = transition
                .batch_result()
                .ok_or_else(|| crate::contract!())?;
            unchanged = if batch
                .final_state()
                .and_then(SemanticActionResult::diff)
                .is_some_and(|diff| diff.entries().is_empty())
            {
                unchanged.saturating_add(1)
            } else {
                0
            };
            state.task.accept_verified_action(batch, &current)?;
            observation = current;
            if let Some(selection) = candidate.take() {
                pending = Some(selection);
            }
            captured_at = at;
            progress = state.task_progress(&observation)?;
            if unchanged >= MAX_UNCHANGED_DECISION_OBSERVATIONS {
                if actions_only {
                    break;
                }
                return Err(AgentWorkFailure::Browser(
                    AgentBrowserProviderError::NoProgress,
                ));
            }
            if state.journal_mut()?.audit.status().pending() >= 32 {
                Self::drain_audit(state, worker, browser, None).await?;
            }
        }
        Ok((observation, captured_at, progress, false))
    }

    /// Looks at a page that may still be replacing its document: a stale or
    /// replaced look is taken again, briefly, instead of ending the read.
    /// The look is dated by when it began.
    async fn observe_settled(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(SemanticObservation, AgentPolicyInstant), AgentWorkFailure> {
        let mut waits = 0u8;
        loop {
            let looked = Self::look_begins(state)?;
            match Self::observe(state, worker, browser).await {
                Err(AgentWorkFailure::Observation(
                    SemanticRuntimePortFailure::Stale
                    | SemanticRuntimePortFailure::DocumentReplaced,
                )) if waits < MAX_SETTLING_LOOKS => {
                    waits += 1;
                    Self::settle_pause(state, worker, browser).await?;
                }
                result => return Ok((result?, looked)),
            }
        }
    }

    /// A catalog read's records: one cell batch locates their text columns
    /// when any are requested, then Rust copies every value from inside its
    /// record. False when nothing could be copied, leaving the planner.
    async fn finish_rows(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        observation: &SemanticObservation,
        captured_at: SemanticCaptureInstant,
        discovery: zephium_agentic::DecisionRowDiscovery,
    ) -> Result<bool, AgentWorkFailure> {
        state.refresh_account(worker, browser)?;
        let mut cells = if discovery.needs_cells() {
            let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
            Self::provider(
                &mut state.native,
                worker,
                browser,
                session.cancellation.clone(),
                session.decide_row_cells(observation, &discovery),
            )
            .await?
        } else {
            None
        };
        let session = state.session.as_ref().ok_or_else(|| crate::contract!())?;
        let found = u8::try_from(discovery.rows()).unwrap_or(u8::MAX);
        let asked = cells.is_some();
        let prepared = discovery.prepare(observation, session.account, captured_at, cells.as_mut());
        state.journal_mut()?.emit(AgentWorkEventKind::RowRead {
            found,
            cells: asked,
            refused: prepared.as_ref().err().copied(),
        })?;
        let Ok(located) = prepared else {
            return Ok(false);
        };
        Box::pin(Self::finish_located_read(state, worker, browser, located)).await?;
        Ok(true)
    }

    /// One extraction from located sources. Focused generation is the only
    /// provider call it can make, and it never reopens the general planner.
    pub(super) async fn finish_located_read(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        located: zephium_agentic::DecisionLocatedRead<'_>,
    ) -> Result<(), AgentWorkFailure> {
        state.journal_mut()?.emit(AgentWorkEventKind::ToolProposed(
            AgentBrowserToolKind::Extract,
        ))?;
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let result = Self::provider(
            &mut state.native,
            worker,
            browser,
            session.cancellation.clone(),
            session.extract_located(located),
        )
        .await?;
        if state.task.accept_extraction(&result)? != AgentWorkTaskProgress::Complete {
            return Err(crate::contract!());
        }
        state.check_task_contract()?;
        state.extraction = Some(result.into_owned().map_err(|_| crate::contract!())?);
        Ok(())
    }

    pub(super) fn retain_action_read_evidence(
        state: &mut WorkState,
        proposal: &crate::AgentBrowserActionProposal,
        observation: &SemanticObservation,
        captured_at: SemanticCaptureInstant,
    ) -> Result<(), AgentWorkFailure> {
        if state.progressive_observation || state.navigation_discovery.is_some() {
            if let Some(schema) = &state.extraction_schema {
                state
                    .session
                    .as_ref()
                    .ok_or_else(|| crate::contract!())?
                    .check_live()
                    .map_err(AgentWorkFailure::Browser)?;
                let read = read_semantic_observation_for_schema(
                    observation,
                    SemanticReadAuthority::Acknowledged(proposal.baseline()),
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
                    .retain(&read, proposal.baseline())
                    .map_err(|error| {
                        AgentWorkFailure::Browser(AgentBrowserProviderError::Read(error))
                    })?;
            }
        }
        Ok(())
    }

    pub(super) async fn execute_prepared_action(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
        proposal: crate::AgentBrowserActionProposal,
        assessment: AgentEffectAssessment,
        baseline: &SemanticObservation,
    ) -> Result<
        (
            SemanticObservation,
            SemanticCaptureInstant,
            crate::AgentBrowserVerifiedTransition,
        ),
        AgentWorkFailure,
    > {
        state.refresh_account(worker, browser)?;
        let rewritable = state.task.rewritable_field(proposal.action(), baseline);
        let discard_listing_view = state
            .extraction_schema
            .as_ref()
            .is_some_and(SemanticExtractionSchema::is_row_collection)
            && !Self::preserves_listing_view(
                proposal.action().kind(),
                baseline.frames().iter().find_map(|frame| {
                    proposal
                        .action()
                        .target_node(frame)
                        .map(SemanticNode::reference)
                }),
                baseline,
            );
        let predecessor = proposal.baseline().clone();
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        session.rewritable_fill = rewritable;
        let id = state.native.identity.id();
        let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
        let automation = if let Some(retained) = &state.native.retained {
            retained.automation_state(now)?
        } else {
            state
                .native
                .contexts()?
                .automation_state(id)
                .map_err(|_| AgentWorkFailure::Context)?
        };
        if discard_listing_view {
            state
                .retained_read_evidence
                .discard_view_before_action(&predecessor)
                .map_err(|error| {
                    AgentWorkFailure::Browser(AgentBrowserProviderError::Read(error))
                })?;
        }
        let request = session
            .authorize_action(
                proposal,
                &assessment,
                automation,
                SemanticActionExecutionInstant::from_millis(now.millis()),
            )
            .map_err(AgentWorkFailure::Browser)?;
        let action_deadline = request.deadline();
        let _ = state.follow.take();
        let request = if state
            .navigation_discovery
            .as_ref()
            .is_some_and(AgentNavigationDiscovery::is_site_session)
        {
            let request = request
                .with_follow(state.follow.clone())
                .with_settle_quiet();
            if state.consent_press {
                request.with_consent()
            } else {
                request
            }
        } else {
            request
        };
        let dispatch = if let Some(retained) = &mut state.native.retained {
            retained.dispatch_action(request, now)
        } else {
            browser.execute_semantic_action(request, worker.semantic_action_completion())
        };
        // Native ownership starts at dispatch, before fallible policy
        // accounting. Recovery must drain even if that accounting fails.
        state.native.action_pending = matches!(dispatch, ContextDispatch::Scheduled);
        session
            .account_action_dispatch(dispatch)
            .map_err(AgentWorkFailure::Browser)?;
        let terminal = match state
            .native
            .next_action_event(worker, browser, action_deadline)
            .await?
        {
            AgentRuntimeEvent::SemanticActionTerminal(terminal)
                if session.action.as_ref().is_some_and(|action| {
                    action.accepts_settlement(&session.action_executions, &terminal)
                }) =>
            {
                terminal
            }
            event => {
                state.native.retain(event)?;
                return Err(AgentWorkFailure::Mailbox);
            }
        };
        state.native.action_pending = false;
        state.native_terminal = Some(terminal);
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let terminal = state
            .native_terminal
            .take()
            .ok_or_else(|| crate::contract!())?;
        let mut wake = session
            .begin_action_settlement(terminal)
            .map_err(AgentWorkFailure::Browser)?;
        // Native evidence cannot restore a revoked document or authorize
        // postcondition reads. Settle its original owner first, then fail
        // closed before any continuation, success, or retry.
        state.native.check_control(worker, browser)?;
        for _ in 0..8 {
            let Some(next_wake) = wake else {
                break;
            };
            let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
            let delay = Duration::from_millis(next_wake.millis().saturating_sub(now.millis()));
            tokio::select! {
                biased;
                event = state.native.next_event(worker, browser) => {
                    state.native.retain(event?)?;
                    return Err(AgentWorkFailure::Mailbox);
                }
                () = tokio::time::sleep(delay) => {}
            }
            let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
            wake = session
                .wake_action_settlement(SemanticSettleInstant::from_millis(now.millis()))
                .map_err(AgentWorkFailure::Browser)?;
        }
        if wake.is_some() {
            return Err(crate::contract!());
        }
        state.journal_mut()?.emit(AgentWorkEventKind::Verifying)?;
        // A step that loaded a new page on the site is verified once that
        // page has settled; a page still loading is looked at again, briefly.
        let mut waits = 0u8;
        // Evidence is dated by when its look began: a slow capture on a
        // throttled page still shows the page as it was within the window.
        let mut looked;
        let current = loop {
            looked = Self::look_begins(state)?;
            match Self::observe(state, worker, browser).await {
                Err(AgentWorkFailure::Observation(
                    SemanticRuntimePortFailure::NotReady
                    | SemanticRuntimePortFailure::Stale
                    | SemanticRuntimePortFailure::DocumentReplaced,
                )) if waits < MAX_SETTLING_LOOKS
                    && state
                        .navigation_discovery
                        .as_ref()
                        .is_some_and(AgentNavigationDiscovery::is_site_session) =>
                {
                    waits += 1;
                    Self::settle_pause(state, worker, browser).await?;
                }
                result => break result?,
            }
        };
        // A read that opened something in a site's app may draw it a moment
        // later: look again while its settle window lasts, so the change is
        // its proof.
        let mut current = current;
        let site_app = state
            .navigation_discovery
            .as_ref()
            .is_some_and(AgentNavigationDiscovery::is_site_session);
        for _ in 0..if site_app { 4 } else { 0 } {
            let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
            let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
            if !session.action_awaits_page_change(
                &current,
                SemanticSettleInstant::from_millis(now.millis()),
            ) {
                break;
            }
            let wake =
                Instant::now() + Duration::from_millis(crate::action::PAGE_CHANGE_LOOK_MILLIS);
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
            looked = Self::look_begins(state)?;
            current = Self::observe(state, worker, browser).await?;
        }
        // A tab's panel or a sheet a click opened can still be drawing when
        // the click is proven: Expedia's flight form was not there yet.
        for _ in 0..if site_app { 3 } else { 0 } {
            let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
            let now = session.policy_now().map_err(AgentWorkFailure::Browser)?;
            if !session
                .action_may_still_draw(&current, SemanticSettleInstant::from_millis(now.millis()))
            {
                break;
            }
            let wake =
                Instant::now() + Duration::from_millis(crate::action::PAGE_CHANGE_LOOK_MILLIS);
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
            looked = Self::look_begins(state)?;
            let next = Self::observe(state, worker, browser).await?;
            let settled = look_digest(&next) == look_digest(&current);
            current = next;
            if settled {
                break;
            }
        }
        // The model is shown the page an action opened up (a filter panel, a
        // long results list) by its leading part, as a first look is, and the
        // action is verified on that same look.
        let current = Self::fit_model_observation(current)?;
        let session = state.session.as_mut().ok_or_else(|| crate::contract!())?;
        let (_, transition) = session
            .verify_action_settlement(
                baseline,
                &current,
                SemanticSettleInstant::from_millis(looked.millis()),
            )
            .map_err(AgentWorkFailure::Browser)?;
        Ok((
            current,
            SemanticCaptureInstant::from_millis(looked.millis()),
            transition,
        ))
    }

    /// One half-second pause while a page settles, within the run's
    /// deadline; a native event ends it for the mailbox to handle.
    async fn settle_pause(
        state: &mut WorkState,
        worker: &mut AgentRuntimeWorker,
        browser: &WorkBrowser<'_>,
    ) -> Result<(), AgentWorkFailure> {
        let wake = Instant::now() + Duration::from_millis(500);
        if wake >= state.native.deadline {
            return Err(AgentWorkFailure::Deadline);
        }
        tokio::select! {
            biased;
            event = state.native.next_event(worker, browser) => {
                state.native.retain(event?)?;
                Err(AgentWorkFailure::Mailbox)
            }
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => Ok(()),
        }
    }

    fn look_begins(state: &mut WorkState) -> Result<AgentPolicyInstant, AgentWorkFailure> {
        state
            .session
            .as_mut()
            .ok_or_else(|| crate::contract!())?
            .policy_now()
            .map_err(AgentWorkFailure::Browser)
    }
}

/// What a look shows, by role, name, text and value: two equal digests are
/// the same page drawn twice.
fn look_digest(observation: &SemanticObservation) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for node in observation.frames().iter().flat_map(|frame| frame.nodes()) {
        format!("{:?}{:?}", node.role(), node.states()).hash(&mut hasher);
        node.name().map(|name| name.as_str()).hash(&mut hasher);
        node.text().map(|text| text.as_str()).hash(&mut hasher);
    }
    hasher.finish()
}
