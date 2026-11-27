//! Mechanical architecture checks for the bounded Terra controller crate.

use std::collections::BTreeSet;
use std::path::Path;

use toml::Value;

const MANIFEST: &str = "crates/zephium-agent-controller/Cargo.toml";
const ROOT: &str = "crates/zephium-agent-controller/src/lib.rs";
const PROBE: &str = "crates/zephium-agent-controller/src/probe.rs";
const TERRA: &str = "crates/zephium-agent-controller/src/terra.rs";
const ACTION: &str = "crates/zephium-agent-controller/src/action.rs";
const DECISION: &str = "crates/zephium-agent-controller/src/decision.rs";
const ACTION_REFUSAL: &str = "crates/zephium-agentic/src/agent_provider/action_refusal.rs";
const REINSPECTION: &str = "crates/zephium-agent-controller/src/work_reinspection.rs";
const WORK: &str = "crates/zephium-agent-controller/src/work.rs";
const WORK_DECISION: &str = "crates/zephium-agent-controller/src/work_decision.rs";
const INSPECTION: &str = "crates/zephium-agent-controller/src/work_inspection.rs";
const OBSERVATION_CHECKPOINT: &str =
    "crates/zephium-agentic/src/agent_provider/observation_checkpoint.rs";
const FORM: &str = "crates/zephium-agent-controller/src/work_form.rs";
// Work actor children that act only through its owned paths (code-owned reads,
// prepared actions): listing loads (a8d9b097) and compound forms (68ecb4c8).
const WORK_CHILDREN: [&str; 2] = [
    "crates/zephium-agent-controller/src/work_listing.rs",
    "crates/zephium-agent-controller/src/work_compound_form.rs",
];
const NAVIGATION: &str = "crates/zephium-agent-controller/src/work_navigation.rs";
const NAVIGATION_POLICY: &str = "crates/zephium-agentic/src/agent_policy/navigation.rs";
const RUN_MANIFEST: &str = "crates/zephium-agentic/src/agent_manifest.rs";
const READ: &str = "crates/zephium-agentic/src/semantic_read.rs";
const CONTINUATION: &str = "crates/zephium-agentic/src/agent_provider/continuation.rs";
const PROVIDER_REQUEST: &str = "crates/zephium-agentic/src/agent_provider/request.rs";
const POLICY: &str = "crates/zephium-agentic/src/agent_policy.rs";
const QUALIFIER: &str = "crates/zephium-terra-macos-probe/src/main.rs";
const WORK_QUALIFIER: &str = "crates/zephium-terra-macos-probe/src/work_actor.rs";
const ALLOWED_DEPENDENCIES: [&str; 7] = [
    "thiserror",
    "tokio",
    "zephium-agentic",
    "zephium-agent-model-catalog",
    "zephium-agent-provider-transport",
    "zephium-agent-runtime",
    "zephium-decision",
];
const FORBIDDEN_TERRA_TOKENS: [&str; 16] = [
    "reqwest",
    "keychain",
    "load_macos_development",
    "load_development",
    "load_probe",
    "zephium_engine",
    "zephium_store",
    "zephium_app",
    "tauri",
    "wry",
    "webkit",
    "rusqlite",
    "tokio::spawn",
    "std::thread",
    "unsafe",
    ".as_str()",
];

/// Checks the release graph and source boundary for the Terra controller.
pub(crate) fn check(repository: &Path) -> Result<(), String> {
    let manifest = read(repository.join(MANIFEST))?;
    let root = read(repository.join(ROOT))?;
    let probe = read(repository.join(PROBE))?;
    let terra = read(repository.join(TERRA))?;
    let action = read(repository.join(ACTION))?;
    validate_manifest(&manifest)?;
    validate_root(&root)?;
    validate_probe(&probe)?;
    validate_terra(&terra)?;
    validate_decision_routing(&read(repository.join(DECISION))?)?;
    validate_action(&action, &read(repository.join(ACTION_REFUSAL))?)?;
    validate_reinspection(&read(repository.join(REINSPECTION))?)?;
    validate_work(
        &read(repository.join(WORK))?,
        &read(repository.join(WORK_DECISION))?,
    )?;
    validate_progressive_observation(
        &read(repository.join(INSPECTION))?,
        &read(repository.join(OBSERVATION_CHECKPOINT))?,
    )?;
    validate_account_refresh(
        &terra,
        &read(repository.join(WORK))?,
        &read(repository.join(WORK_DECISION))?,
        &read(repository.join(POLICY))?,
    )?;
    validate_form(&read(repository.join(FORM))?)?;
    for child in WORK_CHILDREN {
        validate_work_child(&read(repository.join(child))?)?;
    }
    validate_navigation(
        &read(repository.join(NAVIGATION))?,
        &read(repository.join(NAVIGATION_POLICY))?,
        &read(repository.join(CONTINUATION))?,
    )?;
    validate_navigation_route_contract(&read(repository.join(RUN_MANIFEST))?)?;
    validate_navigation_progress(
        &read(repository.join(NAVIGATION_POLICY))?,
        &read(repository.join(CONTINUATION))?,
        &read(repository.join(PROVIDER_REQUEST))?,
    )?;
    validate_scoped_extraction(
        &read(repository.join(READ))?,
        &read(repository.join(CONTINUATION))?,
        &read(repository.join(POLICY))?,
    )?;
    validate_workflow_qualifier(&read(repository.join(QUALIFIER))?)?;
    validate_work_actor_qualifier(&read(repository.join(WORK_QUALIFIER))?)?;
    validate_inventory(&repository.join("crates/zephium-agent-controller"))?;
    Ok(())
}

fn validate_work_child(source: &str) -> Result<(), String> {
    for forbidden in FORBIDDEN_TERRA_TOKENS.into_iter().chain([
        "SemanticModelActionQualificationExecution",
        "for_execution_qualification",
        "TerraProbeActionBridge",
    ]) {
        if source.contains(forbidden) {
            return Err(format!(
                "Work actor child acquired forbidden authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn read(path: impl AsRef<Path>) -> Result<String, String> {
    let path = path.as_ref();
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn validate_reinspection(source: &str) -> Result<(), String> {
    for required in [
        "current.identity().profile() != source.identity().profile()",
        "current.identity().owner() != source.identity().owner()",
        "current.identity().id() == source.identity().id()",
        "binding.frame().origin() != action.frame().origin()",
        "binding.document() != &target.document",
        "current.current_requested_document() != &self.target.document",
        "account.account() != self.source_account.account()",
        "account.attestation() == self.source_account.attestation()",
        "account.observed_at() < self.issued_at",
        "snapshot.completeness() != SemanticCompleteness::Complete",
        "matches.next().is_some()",
        "node.sensitivity() == SemanticSensitivity::Secret",
        "value.truncated()",
        "completion.matches(self.binding.lease(), &self.correlation)",
        "unmatched: Some((Box::new(self), Box::new(completion)))",
        ".observe_initial(lease, now)",
        ".settle_observation(completion, now)",
    ] {
        if !source.contains(required) {
            return Err(format!("effect reinspection lost boundary: {required}"));
        }
    }
    for forbidden in [
        "dispatch_semantic_effect",
        "authorize_semantic_effect",
        "AgentVerifiedSemanticEffect",
        "continue_after_verified_action",
        "tokio::",
        "std::thread",
        "evaluateJavaScript",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "effect reinspection acquired execution authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_form(source: &str) -> Result<(), String> {
    for required in [
        "impl AgentWorkTask for AgentWorkFormTask",
        "pub fn try_new_local_preparation(",
        "const MAX_GOALS: usize = 8;",
        "MAX_SEMANTIC_VALUE_PREVIEW_BYTES",
        "SemanticScope::Initial",
        "snapshot.completeness() != SemanticCompleteness::Complete",
        "snapshot.generation() <= last.snapshot",
        "matches.next().is_some()",
        "descendant(snapshot, option_index, target_index)",
        "preview.source_bytes() == expected.len()",
        "action.source_observation() != baseline.observation",
        "action.source_observation_generation() != baseline.generation",
        "action.frame() != &baseline.frame",
        "action.bound_action().snapshot_generation() != baseline.snapshot",
        "action.bound_action().option_reference() != binding.option",
        "action.fill_text() != Some(&goal.value)",
        "self.bindings.clear();",
        "self.refused = true;",
        "self.account_sample.get()",
        "sample.context() == context",
        "impl AgentWorkTask for AgentWorkFormExtractionTask",
    ] {
        if !source.contains(required) {
            return Err(format!("trusted form contract lost boundary: {required}"));
        }
    }
    for forbidden in [
        "println!",
        "eprintln!",
        "tracing::",
        "log::",
        "serde",
        "derive(Debug",
        "derive(Clone, Debug",
        "AgentWorkController::",
        "AgentRunPolicy",
        "AgentRuntimeBrowser",
        "AgentProviderTransport",
        "tokio::",
        "std::thread",
        "fn allows_subtree_extraction",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "trusted form contract acquired extra authority or content sink: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_account_refresh(
    terra: &str,
    work: &str,
    decision: &str,
    policy: &str,
) -> Result<(), String> {
    let refresh = terra
        .split("pub fn refresh_account(")
        .nth(1)
        .and_then(|tail| tail.split("/// Starts one session").next())
        .ok_or("account refresh lost its owned session boundary")?;
    for required in [
        "self.check_live()?",
        "self.failure = Some(error)",
        "self.policy.accounting().reserved_operations() != 0",
        "self.validate_account_update(account, self.account.context())",
        "account.context() != expected_context",
        "account.account() != self.account.account()",
        "account.observed_at() > now",
        "account.observed_at() < self.account.observed_at()",
        "MAX_AGENT_ACCOUNT_ATTESTATION_AGE_MILLIS",
        "account != self.account",
        "account.attestation() == self.account.attestation()",
        "self.account_attestations.contains(&account.attestation())",
        "self.account_attestations.len() >= self.max_account_attestations",
    ] {
        if !refresh.contains(required) {
            return Err(format!("account refresh lost authority fence: {required}"));
        }
    }
    let sample = work
        .split("fn refresh_account(")
        .nth(1)
        .and_then(|tail| tail.split("fn task_progress(").next())
        .ok_or("Work lost trusted account sampling boundary")?;
    if sample
        .matches("self.native.check_control(worker, browser)?")
        .count()
        != 2
        || !sample.contains("self.task.attest_account(context, now);")
        || !sample.contains(".refresh_account(account)")
        || !work.contains("session.continue_inspection(")
        // Pinned across the actor's two files; new admission paths (discovery,
        // waits, screenshots, decisions, a site's whole first look, the whole
        // look a consent press is made on, an app view's look and each
        // landmark it opens, each view an app read switches to, a results
        // page's read, the look after Rust closes a site's own overlay
        // offer, the refusal of a whole look in a streak, the fresh look
        // after a banner search that found nothing to press, the refusal of
        // a repeated unchanged operation (e7438cd1), a planner-free owned
        // extraction (b44c64ab), the compound search-form offer (68ecb4c8)
        // and the decision after a settled human check (5d803f84)) each
        // sample once more.
        || format!("{work}\n{decision}")
            .matches("state.refresh_account(worker, browser)?")
            .count()
            != 36
        || !policy.contains("MAX_AGENT_ACCOUNT_ATTESTATION_AGE_MILLIS: u64 = 30_000;")
    {
        return Err("Work lost per-admission sampling, control or original expiry boundary".into());
    }
    for forbidden in [
        "AgentContextAccountBinding::new",
        "self.deadline =",
        "self.clock =",
        "self.policy =",
        "self.config =",
    ] {
        if refresh.contains(forbidden) || sample.contains(forbidden) {
            return Err(format!(
                "account refresh minted or replaced authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_scoped_extraction(read: &str, continuation: &str, policy: &str) -> Result<(), String> {
    for (source, required) in [
        (
            read,
            &[
                "struct SemanticReadSubtreeProof",
                "predecessor: SemanticObservationAcknowledgement",
                "if &exact != request",
                "frame.generation() <= anchor.snapshot_generation()",
                "root.key() != anchor.capability().node_key()",
                "ZEPHIUM-SEMANTIC-SUBTREE-READ-GUARD-1",
            ][..],
        ),
        (
            continuation,
            &[
                "pub fn begin_extraction_subtree(",
                "read.matches_subtree(&self.baseline, *target)",
                "subtree_target: Option<crate::SemanticReferenceId>",
                "self.correlation.extraction_schema != Some(schema.id())",
            ][..],
        ),
        (
            policy,
            &[
                "fn provider_subtree_read_taints(",
                "!read.matches_subtree(baseline, target)",
                "cohort.source_guard == baseline.guard()",
                "cohort.contains_reference(target)",
                "fragment.provenance().origin() != anchor.origin()",
                "read_taints(read, account)",
            ][..],
        ),
    ] {
        for boundary in required {
            if !source.contains(boundary) {
                return Err(format!("scoped extraction lost proof boundary: {boundary}"));
            }
        }
    }
    Ok(())
}

fn validate_navigation_route_contract(source: &str) -> Result<(), String> {
    for required in [
        "pub const MAX_AGENT_NAVIGATION_ROUTE_HOPS: usize = 2;",
        "destinations.is_empty() || destinations.len() > MAX_AGENT_NAVIGATION_ROUTE_HOPS",
        "target.as_url().fragment().is_some()",
        "SemanticOrigin::parse(target.as_url().as_str()).as_ref() != Ok(&origin)",
        "target == &departure || destinations[..index - 1].contains(target)",
        "self.navigation_route.is_some()",
        "|| self.navigation_discovery.is_some()",
        "|| self.origins.binary_search(route.origin()).is_err()",
        "if routed != 0",
        "ZEPHIUM-AGENT-NAVIGATION-ROUTES-1",
        "hasher.update(node.id().bytes())",
        "std::iter::once(route.departure()).chain(route.destinations())",
        "hasher.update((bytes.len() as u64).to_be_bytes())",
        "hasher.update(bytes)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "finite navigation route lost immutable boundary: {required}"
            ));
        }
    }
    Ok(())
}

fn validate_navigation_progress(
    policy: &str,
    continuation: &str,
    request: &str,
) -> Result<(), String> {
    for (source, required) in [
        (policy, &[
            "pub(crate) struct AgentNavigationCheckpointBinding",
            "checkpoint.binding == binding", "receipt.hop != hop",
            "receipt.target_guard != target_guard(&route.destinations()[hop])",
            "receipt.settlement != AgentNavigationSettlement::Committed",
            "receipt.operation.context() != context",
        ][..]),
        (continuation, &[
            "navigation_checkpoint: Option<super::request::AgentProviderNavigationContext>",
            "checkpoint.text.len() > super::request::MAX_AGENT_PROVIDER_NAVIGATION_CHECKPOINT_BYTES",
            "policy.validate_provider_navigation_checkpoint(request, checkpoint.binding)",
        ][..]),
        (request, &[
            "struct AgentProviderNavigationContext", "ZEPHIUM_HOST_NAVIGATION_CHECKPOINT_V1",
            // acc7cc16: every checkpoint URL passes the model-safe public URL gate.
            "crate::semantic_wire::model_safe_public_url(target)",
            ".map(provider_navigation_url)",
            "checkpoint.binding()", "openai_text_message(\"developer\", checkpoint)",
        ][..]),
    ] {
        for boundary in required {
            let compact = |text: &str| text.split_whitespace().collect::<String>();
            if !compact(source).contains(&compact(boundary)) {
                return Err(format!("trusted navigation progress lost boundary: {boundary}"));
            }
        }
    }
    // Since 884dc52c the two screenshot paths validate the checkpoint, which
    // still rejects unstructured input when none is carried, instead of
    // rejecting navigation input outright.
    if request
        .matches(".validate_navigation_checkpoint(policy, call_request)?;")
        .count()
        != 11
        || request
            .matches("policy.reject_unstructured_navigation_input(call_request)?;")
            .count()
            != 4
    {
        return Err(
            "navigation progress can be omitted, replayed or undercounted by a provider path"
                .into(),
        );
    }
    let initial = request
        .split("pub fn try_openai_for_provider_exact_count(")
        .nth(1)
        .and_then(|source| source.split("pub fn try_anthropic(").next())
        .ok_or("navigation progress lost whole-input observation builder")?;
    let projection = initial
        .find(".provider_navigation_checkpoint(call_request, observation)?")
        .ok_or("missing policy projection")?;
    let encoding = initial
        .find("encode_openai_observation_body_with_action_targets(")
        .ok_or("missing checkpoint encoding")?;
    let inspection = initial
        .find("let text = progress.encode(observation)?;")
        .ok_or("missing bounded inspection projection")?;
    let bound = initial
        .find("text.len() > MAX_AGENT_PROVIDER_INSPECTION_CHECKPOINT_BYTES")
        .ok_or("missing inspection checkpoint size bound")?;
    let measurement = initial
        .find("conservative_request_measurement(&config, &body)?")
        .ok_or("missing whole-input measurement")?;
    let admission = initial
        .find("policy.prepare_provider_observation_input(")
        .ok_or("missing whole-input admission")?;
    if !(projection < inspection
        && inspection < bound
        && bound < encoding
        && encoding < measurement
        && measurement < admission)
    {
        return Err("navigation progress serialized after policy reservation".into());
    }
    Ok(())
}

const NAVIGATION_VOCABULARY: &str = "self.config = self
            .config
            .clone()
            .with_navigation_available(navigation_available)
            .with_history_back_available(self.history_depth > 0);";

fn validate_navigation(actor: &str, policy: &str, continuation: &str) -> Result<(), String> {
    for (source, boundaries) in [
        (
            actor,
            &[
                "state.navigation_complete() || progress != expected",
                "AgentWorkTaskProgress::ReadyForNavigation",
                "if !scope.admits(target)",
                ".current_navigation_target()",
                "receipt.hop() != state.navigation_hops",
                "state.navigation_hops += 1;",
                "remaining_hops + 1",
                // 64d6c7b9: Back is exempt; any other proposal must equal the target.
                // A site-session follow carries no proposal; its target is
                // the engine's own record, admitted by scope and policy.
                "(Some(AgentBrowserToolProposal::Navigate(proposed)), Some(target)) if proposed == target",
                "scope.is_site_session() && scope.admits(target)",
                "state.refresh_account(worker, browser)?",
                "retire_for_navigation(observation, &target, &session.config)",
                ".authorize_navigation(",
                ".dispatch_navigation(permit, operation, now)",
                ".begin_navigation(id, op)",
                "state.native.snapshot_generation = None;",
                "session.navigation = Some(active);",
                "ContextNativeRequest::Navigate(request)",
                "terminal.operation() == operation",
                ".settle_navigation_terminal(&terminal)",
                "AgentBrowserNavigationDispatchRefusal",
                "self.navigation_refusal.ok_or_else(|| crate::contract!())?",
                "!= Some(refusal.operation)",
                ".refuse_navigation_dispatch(active, refusal.failure, now)",
                "Self::observe(state, worker, browser).await?",
                "state.task_progress(&fresh)?",
                "state.task.attest_account(operation.context(), now)",
                "account.observed_at() < receipt.settled_at()",
                "account.attestation() == self.account.attestation()",
                "self.validate_account_update(account, receipt.operation().context())?",
                "SemanticModelEncodingBudget::INITIAL_PROVIDER_EXACT_CONSERVATIVE",
                ".validate_successor_with_action_authority(",
                "self.drive(prepared.into_transport_input(), Some(observation))",
            ][..],
        ),
        (
            policy,
            &[
                "self.navigation.is_some()",
                "!self.calls.is_empty()",
                "!self.effects.is_empty()",
                "let route = node.navigation_route();",
                "route.map_or(1, |route| route.destinations().len())",
                "hop != self.navigation_receipts.iter().flatten().count()",
                "route.destinations().get(hop) != Some(target)",
                "node.navigation_discovery()",
                "!scope.admits(target)",
                "node.link_destination() == Some(target)",
                "node.sensitivity() == SemanticSensitivity::Public",
                // 506c0c96: discovery caps visits per destination instead of one.
                ".filter(|destination| *destination == target)",
                ">= scope.max_visits_per_destination()",
                "prior.settlement() == AgentNavigationSettlement::Committed",
                "prior.lease() == request.lease",
                "prior.node() == node_id",
                "prior.operation().context() == observation.request().context()",
                "request.account.observed_at() >= prior.settled_at()",
                "!baseline.matches(observation)",
                "!request.automation.can_automate()",
                // 506c0c96: discovery binds both origins to its scope, else same-origin.
                "!scope.admits_origin(source_origin) || !scope.admits_origin(&target_origin)",
                "discovery.is_none() && source_origin != &target_origin",
                "operation.kind() != ContextOperationKind::Navigate",
                "!is_document_successor(",
                "terminal.operation() != active.operation",
                ".admits_final_document(&active.row.target, target)",
                "self.navigation_attempts += 1;",
                ".get_mut(active.row.hop)",
                "*slot = Some(receipt)",
                "hash.update(node.bytes())",
                "hash.update(lease.bytes())",
                "hash.update((hop as u64).to_be_bytes())",
                "AgentNavigationProgressId(hash.finalize().into())",
            ][..],
        ),
        (
            continuation,
            &[
                "pub struct AgentProviderNavigationCheckpoint",
                "pub fn retire_for_navigation(",
                "self.correlation.navigation_target.as_ref() != Some(target)",
                "pub fn validate_successor(",
                "request.id() <= self.prior_call.call()",
                "receipt.matches_source(&self.baseline, &self.target)",
                "observation.request().context() != successor",
                "request.account().account() != receipt.account()",
            ][..],
        ),
    ] {
        for boundary in boundaries {
            if !source.contains(boundary) {
                return Err(format!("navigation lost boundary: {boundary}"));
            }
        }
    }
    // Admits one config rewrite (64d6c7b9): it only narrows the tool vocabulary
    // (with_history_back_available ANDs the configured grant) after a hop.
    if actor.matches(NAVIGATION_VOCABULARY).count() > 1 {
        return Err("navigation rewrote its config more than once".into());
    }
    let scanned_actor = actor.replacen(NAVIGATION_VOCABULARY, "", 1);
    for forbidden in [
        "self.policy =",
        "self.deadline =",
        "self.turns =",
        "self.config =",
        "self.clock =",
        "execute_semantic_action",
        "try_new_with_redirect_policy",
        "AgentProviderTranscript::",
    ] {
        if scanned_actor.contains(forbidden) || policy.contains(forbidden) {
            return Err(format!(
                "navigation widened or replaced authority: {forbidden}"
            ));
        }
    }
    let checkpoint = continuation
        .split("pub struct AgentProviderNavigationCheckpoint {")
        .nth(1)
        .and_then(|tail| tail.split('}').next())
        .ok_or("navigation checkpoint is missing")?;
    for forbidden in ["transcript", "String", "Arc<str>", "correlation:"] {
        if checkpoint.contains(forbidden) {
            return Err(format!(
                "retired checkpoint retained page replay: {forbidden}"
            ));
        }
    }
    let revoke = actor
        .find(".begin_navigation(id, op)")
        .ok_or("navigation lost ref revocation")?;
    let dispatch = actor
        .find("ContextNativeRequest::Navigate(request)")
        .ok_or("navigation lost dispatch")?;
    if revoke >= dispatch {
        return Err("navigation dispatched before old refs were revoked".into());
    }
    let proposal = actor
        .find(".emit(AgentWorkEventKind::ToolProposed(")
        .ok_or("navigation lost pre-authority proposal publication")?;
    let authorize = actor
        .find(".authorize_navigation(")
        .ok_or("navigation lost policy admission")?;
    let audit = actor
        .find("journal.navigation_active(active)")
        .ok_or("navigation lost active audit recording")?;
    if proposal >= authorize || dispatch >= audit {
        return Err("fallible navigation event/audit work can strand an undispatched owner".into());
    }
    let refusal = actor
        .find("session.navigation_refusal =")
        .ok_or("navigation lost exact synchronous refusal evidence")?;
    if refusal <= dispatch || refusal >= audit {
        return Err("synchronous refusal is not retained before fallible audit work".into());
    }
    Ok(())
}

fn validate_manifest(source: &str) -> Result<(), String> {
    let manifest: Value = toml::from_str(source)
        .map_err(|error| format!("controller manifest is invalid TOML: {error}"))?;
    let package = manifest
        .get("package")
        .and_then(Value::as_table)
        .ok_or_else(|| "controller package is missing".to_owned())?;
    if package.get("name").and_then(Value::as_str) != Some("zephium-agent-controller")
        || package.get("publish").and_then(Value::as_bool) != Some(false)
        || package.get("build").and_then(Value::as_bool) != Some(false)
    {
        return Err("controller package identity or build policy drifted".to_owned());
    }
    for forbidden in ["build-dependencies", "target"] {
        if manifest.get(forbidden).is_some() {
            return Err(format!("controller may not declare {forbidden}"));
        }
    }
    let development = manifest
        .get("dev-dependencies")
        .and_then(Value::as_table)
        .ok_or("controller test dependency inventory missing")?;
    let json = development
        .get("serde_json")
        .and_then(Value::as_table)
        .ok_or("controller tests require only workspace serde_json")?;
    if development.len() != 1
        || json.len() != 1
        || json.get("workspace").and_then(Value::as_bool) != Some(true)
    {
        return Err("controller test dependencies must remain serde_json workspace-only".into());
    }
    let dependencies = manifest
        .get("dependencies")
        .and_then(Value::as_table)
        .ok_or_else(|| "controller dependencies are missing".to_owned())?;
    let actual = dependencies
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = ALLOWED_DEPENDENCIES.into_iter().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err("controller dependency inventory drifted".to_owned());
    }
    let transport = dependencies
        .get("zephium-agent-provider-transport")
        .and_then(Value::as_table)
        .ok_or_else(|| "controller provider façade dependency is malformed".to_owned())?;
    if transport.get("workspace").and_then(Value::as_bool) != Some(true)
        || transport.get("optional").and_then(Value::as_bool) != Some(true)
        || transport.len() != 2
    {
        return Err("controller provider façade must remain optional workspace-only".to_owned());
    }
    // The typed-decision layer rides the provider façade: optional, workspace-only.
    let decision = dependencies
        .get("zephium-decision")
        .and_then(Value::as_table)
        .ok_or_else(|| "controller decision dependency is malformed".to_owned())?;
    if decision.get("workspace").and_then(Value::as_bool) != Some(true)
        || decision.get("optional").and_then(Value::as_bool) != Some(true)
        || decision.len() != 2
    {
        return Err("controller decision layer must remain optional workspace-only".to_owned());
    }
    let features = manifest
        .get("features")
        .and_then(Value::as_table)
        .ok_or_else(|| "controller features are missing".to_owned())?;
    require_array(features, "default", &[])?;
    require_array(
        features,
        "provider-transport",
        &[
            "dep:zephium-agent-provider-transport",
            "dep:zephium-decision",
            "zephium-agent-provider-transport/provider-transport",
        ],
    )?;
    require_array(
        features,
        "probe-harness",
        &[
            "provider-transport",
            "zephium-agent-provider-transport/probe-harness",
        ],
    )?;
    if features.len() != 3 {
        return Err("controller feature inventory drifted".to_owned());
    }
    Ok(())
}

fn require_array(
    features: &toml::map::Map<String, Value>,
    key: &str,
    expected: &[&str],
) -> Result<(), String> {
    let Some(values) = features.get(key).and_then(Value::as_array) else {
        return Err(format!("controller feature {key} is missing"));
    };
    let actual = values.iter().filter_map(Value::as_str).collect::<Vec<_>>();
    if actual != expected {
        return Err(format!("controller feature {key} drifted"));
    }
    Ok(())
}

fn validate_root(source: &str) -> Result<(), String> {
    for required in [
        "compile_error!(\"the agent controller probe harness is forbidden in optimized builds\")",
        "#[cfg(feature = \"provider-transport\")]\nmod terra;",
        "#[cfg(feature = \"provider-transport\")]\npub use terra",
        "#[cfg(feature = \"probe-harness\")]\nmod probe;",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "controller root lost required boundary: {required}"
            ));
        }
    }
    Ok(())
}

fn validate_probe(source: &str) -> Result<(), String> {
    for required in [
        "AgentProviderSettledToolTurn",
        "AgentBrowserToolProposal::Act",
        "SemanticModelActionQualificationExecution",
        "into_actions()",
        "drop(self.continuation)",
        "settle_for_continuation",
        "compute_semantic_diff",
        "TerraProbeVerifiedTransition",
    ] {
        if !source.contains(required) {
            return Err(format!("controller probe lost required bridge: {required}"));
        }
    }
    for forbidden in FORBIDDEN_TERRA_TOKENS {
        if source.contains(forbidden) {
            return Err(format!(
                "controller probe contains forbidden authority token: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_terra(source: &str) -> Result<(), String> {
    let read = source
        .split_once("pub async fn continue_after_read(")
        .and_then(|(_, tail)| tail.split_once("pub async fn extract<'a>("))
        .map(|(read, _)| read)
        .ok_or("controller lost bounded read driver")?;
    for required in [
        "self.check_live()?",
        "self.turns >= self.max_model_calls",
        "!self.config.permits_baseline_read()",
        "AgentBrowserScopeProposal::Initial",
        "read_semantic_observation",
        "SemanticReadAuthority::Acknowledged(continuation.baseline())",
        "SemanticReadSensitivityLimit::PublicOnly",
        "SemanticReadBudget::STANDARD",
        ".bind_read_request(request, &self.config, &read, payload)",
        "AgentProviderReadContinuationRequestDraft::try_new(bound)",
        "try_prepare_for_provider_exact_count",
        "self.drive(prepared.into_transport_input(), Some(observation))",
    ] {
        if !read.contains(required) {
            return Err(format!(
                "controller read lost authority boundary: {required}"
            ));
        }
    }
    for forbidden in [
        "invoke_semantic",
        "capture_once",
        "from_fingerprint",
        "begin_expansion",
        "acknowledge(",
    ] {
        if read.contains(forbidden) {
            return Err(format!(
                "controller read acquired new native/ref authority: {forbidden}"
            ));
        }
    }
    let compact: String = source.split_whitespace().collect();
    if compact.contains("pubfntry_finish_unsuccessful(")
        || compact.contains("pub(crate)fntry_finish_unsuccessful(")
        || compact.contains("pub(super)fntry_finish_unsuccessful(")
    {
        return Err("unsuccessful resource closure must stay private to the Work actor".into());
    }
    for required in [
        "impl AgentRuntimeController for TerraTextOnlyController",
        "try_terra_provider_exact_call_config",
        "settle_terra_provider_terminal",
        "AgentProviderAttempt",
        "count_openai_input_tokens",
        "AgentProviderBatchDisposition::Continue",
        "delta.len()",
        "drop(state.objective.take())",
        "drop(state.credential.take())",
        "attempt.abort(AgentProviderAbortReason::ControllerFault)",
        "state.transport.seal();",
        "const MAX_BROWSER_MODEL_TURNS: u8 = 8;",
        "restrict_to_locate_and_act()",
        "pub async fn start_initial(",
        "pub fn try_finish(",
        "fn try_finish_unsuccessful(",
        "self.seal_terminal(false)",
        "else if !unsuccessful",
        "self.retained_terminal.is_some()",
        "self.navigation_refusal.is_some()",
        "self.action_admission_failure.is_some()",
        "!unsuccessful || refusal.human_review().is_none()",
        "AgentBrowserSessionFinishRefusal",
        "self.policy.accounting().reserved_model_tokens() != 0",
        "action_executions: zephium_agentic::SemanticActionExecutionCoordinator::new()",
        "action_settlements: zephium_agentic::SemanticActionSettlementCoordinator::new()",
        "self.action_executions.seal()",
        "self.action_settlements.seal()",
        "continue_after_verified_action",
        "continue_after_locate",
        "locate_semantic_observation",
        "SemanticModelEncodingBudget::LOCATE_RESULT_PROVIDER_EXACT_CONSERVATIVE",
        ".bind_locate_request(model_request, &self.config, &result, payload)",
        "pub async fn extract<'a>(",
        "schema.id().get() != 1",
        "self.drive_terminal(input, Some(output)).await?",
        "SemanticReadSensitivityLimit::PublicOnly",
        "self.extraction_output.take()",
        ".bind_diff_request_with_action_authority(",
        ".try_prepare_for_provider_exact_count(&mut self.policy, request, diff)",
        ".try_prove_shutdown()",
        ".try_admit_model(input, &mut self.policy, model, cancellation)",
        ".execute_model_client(|batch| {",
        "settle_chosen_terminal(*settlement, chosen, policy)",
        "chosen_call_budget(",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "controller Terra path lost required boundary: {required}"
            ));
        }
    }
    for forbidden in FORBIDDEN_TERRA_TOKENS {
        if source.contains(forbidden) {
            return Err(format!(
                "controller Terra path contains forbidden authority token: {forbidden}"
            ));
        }
    }
    Ok(())
}

// Terra's typed-decision routing is a child of the Terra path and inherits its fence.
fn validate_decision_routing(source: &str) -> Result<(), String> {
    for forbidden in FORBIDDEN_TERRA_TOKENS {
        if source.contains(forbidden) {
            return Err(format!(
                "controller decision routing contains forbidden authority token: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_inventory(crate_root: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(crate_root)
        .map_err(|error| format!("cannot inspect controller crate root: {error}"))?;
    let root = entries
        .map(|entry| entry.map_err(|error| format!("cannot inspect controller entry: {error}")))
        .collect::<Result<Vec<_>, _>>()?;
    let names = root
        .iter()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    if names
        != BTreeSet::from([
            "Cargo.toml".to_owned(),
            "src".to_owned(),
            "tests".to_owned(),
        ])
    {
        return Err("controller crate root inventory drifted".to_owned());
    }
    let entries = std::fs::read_dir(crate_root.join("tests"))
        .map_err(|error| format!("cannot inspect controller integration tests: {error}"))?;
    let names = entries
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if names != BTreeSet::from(["scoped_runtime.rs".to_owned()]) {
        return Err("controller integration-test inventory drifted".to_owned());
    }
    let source = crate_root.join("src");
    let entries = std::fs::read_dir(&source)
        .map_err(|error| format!("cannot inspect controller source: {error}"))?;
    let names = entries
        .map(|entry| {
            entry.map_err(|error| format!("cannot inspect controller source entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    if names
        != BTreeSet::from([
            "action.rs".to_owned(),
            "lib.rs".to_owned(),
            "probe.rs".to_owned(),
            "terra.rs".to_owned(),
            "work.rs".to_owned(),
            "work_retained.rs".to_owned(),
            "work_reinspection.rs".to_owned(),
            "work_reinspection_tests.rs".to_owned(),
            "work_tests.rs".to_owned(),
            "work_combined_tests.rs".to_owned(),
            "work_scoped_tests.rs".to_owned(),
            "work_read_tests.rs".to_owned(),
            "work_account_tests.rs".to_owned(),
            "work_navigation.rs".to_owned(),
            "work_inspection.rs".to_owned(),
            "work_navigation_tests.rs".to_owned(),
            "work_route_tests.rs".to_owned(),
            "work_form.rs".to_owned(),
            "work_form_tests.rs".to_owned(),
            "work_discovery.rs".to_owned(),
            // Rust's own reading of well-known bot-check pages: pure
            // classification of a look, no provider, effect or native call.
            "work_challenge.rs".to_owned(),
            // Reviewed decision layer (2af19810, d7c2496e): Terra's typed-decision
            // routing, the Work actor's decision child and their test modules.
            "decision.rs".to_owned(),
            "terra_decision_tests.rs".to_owned(),
            "work_decision.rs".to_owned(),
            "work_decision_tests.rs".to_owned(),
            // Work actor children scanned by validate_work_child: bounded
            // listing loads (a8d9b097) and compound search forms (68ecb4c8).
            "work_listing.rs".to_owned(),
            "work_listing_tests.rs".to_owned(),
            "work_compound_form.rs".to_owned(),
        ])
    {
        return Err("controller source inventory drifted".to_owned());
    }
    Ok(())
}

fn validate_workflow_qualifier(source: &str) -> Result<(), String> {
    let workflow = source
        .split_once("fn run_variable_workflow()")
        .and_then(|(_, source)| source.split_once("fn run_two_action("))
        .map(|(source, _)| source)
        .ok_or_else(|| "generalized native workflow qualifier is missing".to_owned())?;
    for required in [
        "AgentBrowserSession::try_new",
        "AgentBrowserModel::Luna",
        "AgentBrowserRetention::InspectablePublicData",
        ".next_action(",
        ".authorize_action(",
        ".settle_action(",
        ".continue_after_verified_action(",
        "workflow.progress.observe(initial, final_query, language)",
        "workflow.session = Some(refusal.into_session())",
        "terminal.model_receipts().len() != workflow.metrics.len()",
        "workflow.verified < 3",
        "workflow.metrics.len() > 8",
    ] {
        if !workflow.contains(required) {
            return Err(format!(
                "native workflow lost production boundary: {required}"
            ));
        }
    }
    for forbidden in [
        "TerraProbeActionBridge",
        "SemanticModelActionQualificationExecution",
        "for_execution_qualification",
    ] {
        if workflow.contains(forbidden) {
            return Err(format!(
                "native workflow acquired synthetic authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_action(source: &str, refusal: &str) -> Result<(), String> {
    let refusal_branch = source
        .split("AgentEffectAuthorization::NeedsHuman(transition) =>")
        .nth(1)
        .and_then(|tail| tail.split("dispatch_semantic_effect").next())
        .ok_or("controller lost exact pre-dispatch refusal branch")?;
    if source.matches("human_review = Some(transition)").count() != 1
        || !refusal_branch.contains("human_review = Some(transition)")
        || !refusal_branch.contains(".needs_human(transition)")
    {
        return Err("controller review classification escaped exact unissued policy branch".into());
    }
    for required in [
        "turn.resolve_action",
        "authorize_semantic_effect",
        "dispatch_semantic_effect",
        "execution: &mut SemanticActionExecutionCoordinator",
        "settlement: &mut SemanticActionSettlementCoordinator",
        "terminal: Option<SemanticActionBatchResult>",
        "record_success",
        "verify_semantic_action_terminal",
        "settle_verified_semantic_terminal",
        "finalize_accounted_semantic_action_result",
        "self.pending = Some",
        "self.terminal = Some",
        "*proposal_failure = Some(AgentBrowserActionProposalRefusal",
        "proposal: self",
        "let mut human_review = None",
        "human_review = Some(transition)",
        "drop(self.proposal)",
        "AgentBrowserActionFinalizationRefusal",
        "action.settle_budget().millis() < MIN_AGENT_BROWSER_SNAPSHOT_SETTLE_MILLIS",
        "self.retain_failure(failed);",
        "AgentBrowserActionError::Verification(reason)",
    ] {
        if !source.contains(required) {
            return Err(format!("controller action lost boundary: {required}"));
        }
    }
    for required in [
        "config != &continuation.config",
        "!continuation.baseline.matches(observation)",
        "actions.iter().find_map(|action| {",
        "targets.permitted_operations(context.target)",
        "SemanticReferenceError::OperationDenied",
        "SemanticActionBatch::bind",
    ] {
        if !refusal.contains(required) {
            return Err(format!(
                "provider action resolution lost grounding boundary: {required}"
            ));
        }
    }
    for forbidden in FORBIDDEN_TERRA_TOKENS.into_iter().chain([
        "SemanticModelActionQualificationExecution",
        "for_execution_qualification",
    ]) {
        if source.contains(forbidden) {
            return Err(format!(
                "controller action contains forbidden authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_progressive_observation(inspection: &str, checkpoint: &str) -> Result<(), String> {
    for required in [
        "state.check_task_contract()?",
        "session.continue_after_observation(",
        "checkpoint.baseline()",
        // 7cf84437 threads the observation capability into the retained scope.
        "let capability = state.observation_capability();",
        ".observe_retained_scope(worker, expansion, capability)",
        "state.refresh_account(worker, browser)?",
        "Self::provider(",
        ".prepare_successor_with_action_authority(",
        "self.drive(prepared.into_transport_input(), Some(observation))",
    ] {
        if !inspection.contains(required) {
            return Err(format!(
                "progressive observation lost original owner join: {required}"
            ));
        }
    }
    for required in [
        "config != &self.config",
        "!self.baseline.matches(observation)",
        ".matches_manifest_revision(policy.manifest().id(), policy.manifest().guard())",
        "request.lease() != self.prior_call.lease()",
        "self.validate_successor(previous, current, request, &config)",
        "AgentPreparedObservationRequest::try_for_config_with_inspections_and_action_authority(",
        "successor_generation(previous, current, self.anchor_lost)",
        "!anchor_lost && matches!(current.request().scope(), crate::SemanticScope::Initial)",
        "== Some(current_generation)",
        "node.parent().is_some()",
        "self.context == observation.request().context()",
        "capture.snapshot == observation.frames()[0].generation().get()",
        "progress.captures.len() >= MAX_AGENT_INSPECTION_CAPTURES",
        ".find(|node| node.key() == key)",
        "node.reference().model_token()",
    ] {
        if !checkpoint.contains(required) {
            return Err(format!(
                "progressive observation lost delivery authority: {required}"
            ));
        }
    }
    for forbidden in FORBIDDEN_TERRA_TOKENS {
        if inspection.contains(forbidden) {
            return Err(format!(
                "progressive observation owns forbidden integration: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_work(work: &str, decision: &str) -> Result<(), String> {
    // The Work actor spans work.rs and its work_decision.rs child since d7c2496e.
    let actor = format!("{work}\n{decision}");
    let source = actor.as_str();
    let human = source
        .split("pub(crate) fn needs_human(")
        .nth(1)
        .and_then(|tail| tail.split("pub(super) fn model_settled(").next())
        .ok_or("Work actor lost human-refusal ownership boundary")?;
    if human.contains("execution.take()") || human.contains("wait_for_human(") {
        return Err("Work refusal yielded or discarded its original execution token".into());
    }
    let committed = source
        .find("claim.commit_with_shutdown(proof, settlement, provider)")
        .ok_or("Work actor lost original closure commit")?;
    let discarded = source
        .find("AgentBrowserActionProposalRefusal::discard_after_closure")
        .ok_or("Work actor lost original unissued proposal owner")?;
    if discarded < committed {
        return Err("Work actor discarded refusal owner before original closure commit".into());
    }
    for required in [
        "impl AgentRuntimeController for AgentWorkController",
        "self.account_sample.get()",
        "AgentRunSupervisor",
        "AgentAuditLedger",
        "AgentRunMetricClosure::try_close",
        "AgentNativeShutdownResources::new",
        "claim.commit_with_shutdown(proof, settlement, provider)",
        "state.task_progress(&observation)?",
        "let progress = self.task.evaluate(observation)?;",
        "self.task.extraction_schema() != self.extraction_schema.as_ref()",
        "navigation_route.as_ref() != approved_route",
        "navigation_target.is_some() && navigation_route.is_some()",
        "route.departure() != &input.context.target",
        "self.task.navigation_route() != self.navigation_route.as_ref()",
        "self.task.navigation_discovery() != self.navigation_discovery.as_ref()",
        "navigation_discovery.as_ref() != approved_discovery",
        "route.destinations().get(self.navigation_hops)",
        "self.navigation_hops == self.navigation_length()",
        "self.task.allows_actions_before_extraction() != self.actions_before_extraction",
        "(input.durable_result || actions_before_extraction || subtree_extraction)",
        "&& extraction_schema.is_none()",
        "self.task.allows_subtree_extraction() != self.subtree_extraction",
        "self.task.allows_baseline_read() != self.baseline_read",
        "session.config = session.config.with_baseline_read()",
        ".begin_extraction_subtree(",
        "Self::capture_once(state, worker, browser, request, frame).await?",
        "expanded.as_ref().map(|_| observation)",
        "session.config.restrict_to_scoped_extraction()",
        "session.config.restrict_to_actions_and_scoped_extraction()",
        "progress == AgentWorkTaskProgress::Complete && self.actions_before_extraction",
        "progress != AgentWorkTaskProgress::ReadyForExtraction",
        "progress == AgentWorkTaskProgress::ReadyForExtraction",
        "session.config.restrict_to_actions_and_extraction()",
        "captured_at = current_at;",
        "session.extract_from_with_evidence(",
        "if session.turns >= session.max_model_calls",
        "state.task.accept_extraction(&result)? != AgentWorkTaskProgress::Complete",
        "state.native.revoke(browser)",
        ".begin_action_settlement(",
        "MAX_AGENT_WORK_EVENTS: usize = 64",
        "AgentWorkOutcome::Recovery",
        "worker.try_drain_terminal_claim_refusal_event()",
        "recovery_close: Option<ContextOperationJoin>",
        "Self::begin_recovery_close(state, browser)",
        "let _ = session.settle_navigation_refusal();",
        "AgentWorkOutcome::ClosedUnsuccessfully",
        "session.try_finish_unsuccessful()",
        "state.native.deferred.is_empty()",
        "state.drained.is_some()",
        // Since 8755115b the terminal intent is frozen once and the closure must
        // match it, replacing the unsuccessful flag threaded into publication.
        "if state.terminal_intent.replace(terminal_intent).is_some() {",
        "let terminal_intent = state.terminal_intent.ok_or_else(|| crate::contract!())?;",
        "let closure_matches_intent = match terminal_intent {",
        "closure.outcome() != AgentRunProgressOutcome::Succeeded",
        "if !closure_matches_intent {",
        "self.publish_terminal(worker).await",
        ".record_human_refusal(",
        "proposal_refusal: action_proposal_failure",
        "AgentBrowserActionProposalRefusal::discard_after_closure",
        "AgentRunProgressOutcome::Failed(AgentSupervisorFailure::PolicyDenied)",
    ] {
        if !source.contains(required) {
            return Err(format!("Work actor lost boundary: {required}"));
        }
    }
    let scanned = without_announced_loading(&without_loading_classifier(
        &without_challenge_detector(source)?,
    )?)?;
    let scanned = without_look_digest(&without_typed_fill(&scanned)?)?;
    for forbidden in FORBIDDEN_TERRA_TOKENS.into_iter().chain([
        "SemanticModelActionQualificationExecution",
        "for_execution_qualification",
        "TerraProbeActionBridge",
    ]) {
        if scanned.contains(forbidden) {
            return Err(format!(
                "Work actor acquired forbidden authority: {forbidden}"
            ));
        }
    }
    validate_work_decision(&without_look_digest(decision)?)
}

// This exact 16-node boolean classifier is the only additional content read.
// Keep its whole body pinned so a changed bound, phrase or data flow is rejected.
const LOADING_CLASSIFIER: &str = r#"fn says_loading(observation: &SemanticObservation) -> bool {
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
}"#;

fn without_loading_classifier(source: &str) -> Result<String, String> {
    if source.matches(LOADING_CLASSIFIER).count() != 1 {
        return Err("Work loading classifier changed its exact bounded content read".to_owned());
    }
    Ok(source.replacen(
        LOADING_CLASSIFIER,
        &LOADING_CLASSIFIER.replace(".as_str()", ""),
        1,
    ))
}

// Admits one more boolean content read: after a navigation a site page whose
// status or progress line opens with a fixed loading phrase (rome2rio's
// "Loading trip planner") is looked at again for a few more seconds instead of
// being handed to the planner half-built. Only short lines of two scheduling
// roles are read, against a closed phrase list, and only a bool leaves it, so
// it grants no authority. Pinned whole like the classifier above.
const ANNOUNCED_LOADING_CLASSIFIER: &str = r#"fn announces_loading(observation: &SemanticObservation) -> bool {
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
}"#;

fn without_announced_loading(source: &str) -> Result<String, String> {
    if source.matches(ANNOUNCED_LOADING_CLASSIFIER).count() != 1 {
        return Err(
            "Work announced-loading classifier changed its exact bounded content read".to_owned(),
        );
    }
    Ok(source.replacen(
        ANNOUNCED_LOADING_CLASSIFIER,
        &ANNOUNCED_LOADING_CLASSIFIER.replace(".as_str()", ""),
        1,
    ))
}

// Admits the fill's own text (b8b81508): the planner's typed value, never page
// content, is matched by the agentic crate against the suggestions the fill
// opened, and only a reference leaves it, for a code-owned click that passes
// the same binding, task assessment and evidence as any owned read. Producer
// and its one consumer are pinned whole.
const TYPED_FILL_READ: &str = r#"let typed = (proposal.action().kind() == SemanticActionKind::Fill)
                .then(|| {
                    proposal
                        .action()
                        .fill_text()
                        .map(|text| text.as_str().to_owned())
                })
                .flatten();"#;

const TYPED_FILL_USE: &str = r#"if let Some(choice) = typed
                .as_deref()
                .and_then(|typed| zephium_agentic::semantic_sole_choice(&current, typed))
                .filter(|_| state.progressive_observation)
            {
                let next = Box::pin(Self::commit_suggestion("#;

fn without_typed_fill(source: &str) -> Result<String, String> {
    if source.matches(TYPED_FILL_READ).count() != 1
        || source.matches(TYPED_FILL_USE).count() != 1
        || source.matches("semantic_sole_choice(").count() != 1
    {
        return Err("Work fill-suggestion read changed its exact bounded use".to_owned());
    }
    Ok(source.replacen(
        TYPED_FILL_READ,
        &TYPED_FILL_READ.replace(".as_str()", ""),
        1,
    ))
}

// Admits the settle digest (2cba4aed): a click's page is looked at again until
// two looks hash the same, so only a u64 compared for equality leaves it.
const LOOK_DIGEST: &str = r#"fn look_digest(observation: &SemanticObservation) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for node in observation.frames().iter().flat_map(|frame| frame.nodes()) {
        format!("{:?}{:?}", node.role(), node.states()).hash(&mut hasher);
        node.name().map(|name| name.as_str()).hash(&mut hasher);
        node.text().map(|text| text.as_str()).hash(&mut hasher);
    }
    hasher.finish()
}"#;

const LOOK_DIGEST_USE: &str = "let settled = look_digest(&next) == look_digest(&current);";

fn without_look_digest(source: &str) -> Result<String, String> {
    if source.matches(LOOK_DIGEST).count() != 1
        || source.matches(LOOK_DIGEST_USE).count() != 1
        || source.matches("look_digest(").count() != 3
    {
        return Err("Work look digest changed its exact bounded equality read".to_owned());
    }
    Ok(source.replacen(LOOK_DIGEST, &LOOK_DIGEST.replace(".as_str()", ""), 1))
}

const CHALLENGE_DETECTOR: &str =
    "fn looks_like_human_challenge(observation: &SemanticObservation) -> bool {";

// Admits one content read (352048d5): the bot-check detector matches short node
// text against fixed phrases over a capped node count and yields only a bool.
fn without_challenge_detector(source: &str) -> Result<String, String> {
    let Some((head, tail)) = source.split_once(CHALLENGE_DETECTOR) else {
        return Ok(source.to_owned());
    };
    let (detector, rest) = tail
        .split_once("\n}\n")
        .ok_or("Work challenge detector is malformed")?;
    for required in [
        "if nodes > MAX_HUMAN_CHALLENGE_NODES {",
        ".filter(|text| text.len() <= 96)",
        ".any(|phrase| lower.starts_with(phrase))",
    ] {
        if !detector.contains(required) {
            return Err(format!("Work challenge detector lost bound: {required}"));
        }
    }
    if detector.matches(".as_str()").count() != 1
        || source.matches(CHALLENGE_DETECTOR).count() != 1
        || !source.contains("const MAX_HUMAN_CHALLENGE_NODES: usize = 48;")
    {
        return Err("Work challenge detector widened its content read".to_owned());
    }
    let detector = detector.replacen(".map(|text| text.as_str().trim())", "", 1);
    Ok(format!("{head}{detector}\n}}\n{rest}"))
}

// Decision-selected actions settle in work_decision.rs; the post-action capture
// time is still the settlement instant, and that actor has no wider authority.
const LOOK_BEGINS: &str = r#"fn look_begins(state: &mut WorkState) -> Result<AgentPolicyInstant, AgentWorkFailure> {
        state
            .session
            .as_mut()
            .ok_or_else(|| crate::contract!())?
            .policy_now()
            .map_err(AgentWorkFailure::Browser)
    }"#;

// The consent press's settled look (59094d95b) sits outside action
// settlement, so it carries its own rule: one look, dated as it begins.
const CONSENT_LOOK: &str =
    "let looked = Self::look_begins(state)?;\n            match Self::observe(state, worker, browser)";

fn validate_work_decision(source: &str) -> Result<(), String> {
    let consent = source
        .split("async fn observe_settled(")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .ok_or("Work decision actor lost its settled consent look")?;
    if source.matches("async fn observe_settled(").count() != 1
        || consent
            .matches("Self::observe(state, worker, browser)")
            .count()
            != 1
        || consent.matches(CONSENT_LOOK).count() != 1
        || consent.matches("looked =").count() != 1
        || !consent.contains("result => return Ok((result?, looked)),")
    {
        return Err(
            "Work decision actor dated its consent look by something other than its look".into(),
        );
    }
    let settled = source
        .split("pub(super) async fn execute_prepared_action(")
        .nth(1)
        .ok_or("Work decision actor lost verified action settlement")?;
    // Since 75c82aad settlement and capture are dated by when the last look
    // began: a policy-clock read taken immediately before each look.
    for required in [
        ".verify_action_settlement(",
        "SemanticSettleInstant::from_millis(looked.millis()),",
        "SemanticCaptureInstant::from_millis(looked.millis()),",
        LOOK_BEGINS,
    ] {
        if !settled.contains(required) {
            return Err(format!("Work decision actor lost boundary: {required}"));
        }
    }
    let looks = settled
        .matches("Self::observe(state, worker, browser)")
        .count();
    let dated = settled
        .matches(
            "looked = Self::look_begins(state)?;\n            match Self::observe(state, worker, browser)",
        )
        .count()
        + settled
            .matches(
                "looked = Self::look_begins(state)?;\n            current = Self::observe(state, worker, browser)",
            )
            .count()
        + settled
            .matches(
                "looked = Self::look_begins(state)?;\n            let next = Self::observe(state, worker, browser)",
            )
            .count();
    if looks != 3 || dated != looks || settled.matches("looked =").count() != looks {
        return Err("Work decision actor dated settlement by something other than its look".into());
    }
    for forbidden in FORBIDDEN_TERRA_TOKENS {
        if source.contains(forbidden) {
            return Err(format!(
                "Work decision actor acquired forbidden authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_work_actor_qualifier(source: &str) -> Result<(), String> {
    for required in [
        "type Task = AgentWorkFormTask;",
        "AgentWorkFormTask::try_new_local_preparation(",
        "AgentWorkFormPhase::try_new(vec![",
        "AgentWorkFormGoal::fill(",
        "AgentWorkFormGoal::select(",
        "AgentWorkController::try_new_for_probe(",
        "PendingAgentRuntime::spawn_suspended_with_controller(",
        "pending.bind_browser_port(port)",
        "zephium_engine::run_macos_agentic_work_actor_probe(",
        "zephium_store::SqliteStore::open(",
        "AgentBrowserRetention::InspectablePublicData",
        "AgentWorkOutcome::Succeeded(settlement)",
        "AgentBrowserShutdownOutcome::Clean(_)",
        "StoreShutdownOutcome::Clean",
        "settlement.closure().effects()",
        "impl AgentWorkTask for PublicPreparedResultTask",
        "Some(result.observation()) != self.ready",
        "preview.source_bytes() != value.as_str().len()",
        "preview.text() != value.as_str()",
        "self.extraction.accept_extraction(result)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "Work qualifier lost actual product boundary: {required}"
            ));
        }
    }
    for forbidden in [
        "TerraProbeActionBridge",
        "for_execution_qualification",
        "SemanticModelActionQualificationExecution",
        "impl AgentBrowserPort",
        "set_var(",
        "std::fs::write",
        "request.body()",
        "println!(\"{observation",
        "println!(\"{response",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "Work qualifier acquired synthetic/content authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        validate_account_refresh, validate_navigation, validate_navigation_progress,
        validate_navigation_route_contract, validate_reinspection, validate_scoped_extraction,
    };
    use super::{
        validate_action, validate_decision_routing, validate_form, validate_manifest,
        validate_probe, validate_root, validate_terra, validate_work,
        validate_work_actor_qualifier, validate_workflow_qualifier, CONSENT_LOOK,
        LOADING_CLASSIFIER, LOOK_DIGEST_USE,
    };

    const MANIFEST: &str = include_str!("../../crates/zephium-agent-controller/Cargo.toml");
    const ROOT: &str = include_str!("../../crates/zephium-agent-controller/src/lib.rs");
    const PROBE: &str = include_str!("../../crates/zephium-agent-controller/src/probe.rs");
    const TERRA: &str = include_str!("../../crates/zephium-agent-controller/src/terra.rs");
    const ACTION: &str = include_str!("../../crates/zephium-agent-controller/src/action.rs");
    const ACTION_REFUSAL: &str =
        include_str!("../../crates/zephium-agentic/src/agent_provider/action_refusal.rs");
    const WORK: &str = include_str!("../../crates/zephium-agent-controller/src/work.rs");
    const WORK_DECISION: &str =
        include_str!("../../crates/zephium-agent-controller/src/work_decision.rs");
    const READ: &str = include_str!("../../crates/zephium-agentic/src/semantic_read.rs");
    const CONTINUATION: &str =
        include_str!("../../crates/zephium-agentic/src/agent_provider/continuation.rs");
    const PROVIDER_REQUEST: &str =
        include_str!("../../crates/zephium-agentic/src/agent_provider/request.rs");
    const POLICY: &str = include_str!("../../crates/zephium-agentic/src/agent_policy.rs");
    const RUN_MANIFEST: &str = include_str!("../../crates/zephium-agentic/src/agent_manifest.rs");
    const QUALIFIER: &str = include_str!("../../crates/zephium-terra-macos-probe/src/main.rs");
    const WORK_QUALIFIER: &str =
        include_str!("../../crates/zephium-terra-macos-probe/src/work_actor.rs");
    const FORM: &str = include_str!("../../crates/zephium-agent-controller/src/work_form.rs");
    const NAVIGATION: &str =
        include_str!("../../crates/zephium-agent-controller/src/work_navigation.rs");
    const NAVIGATION_POLICY: &str =
        include_str!("../../crates/zephium-agentic/src/agent_policy/navigation.rs");

    #[test]
    fn progressive_observation_cannot_bypass_the_original_delivery_or_native_owners() {
        let inspection =
            include_str!("../../crates/zephium-agent-controller/src/work_inspection.rs");
        let checkpoint = include_str!(
            "../../crates/zephium-agentic/src/agent_provider/observation_checkpoint.rs"
        );
        super::validate_progressive_observation(inspection, checkpoint).unwrap();
        for boundary in [
            "session.continue_after_observation(",
            ".prepare_successor_with_action_authority(",
            "state.refresh_account(worker, browser)?",
        ] {
            assert!(super::validate_progressive_observation(
                &inspection.replace(boundary, "removed"),
                checkpoint
            )
            .is_err());
        }
        for boundary in [
            "request.lease() != self.prior_call.lease()",
            "successor_generation(previous, current, self.anchor_lost)",
            "!anchor_lost && matches!(current.request().scope(), crate::SemanticScope::Initial)",
            "== Some(current_generation)",
            "node.parent().is_some()",
            "self.context == observation.request().context()",
            "capture.snapshot == observation.frames()[0].generation().get()",
            "progress.captures.len() >= MAX_AGENT_INSPECTION_CAPTURES",
            ".find(|node| node.key() == key)",
            "node.reference().model_token()",
        ] {
            assert!(super::validate_progressive_observation(
                inspection,
                &checkpoint.replace(boundary, "removed")
            )
            .is_err());
        }
    }

    #[test]
    fn navigation_progress_cannot_lose_trust_replay_or_whole_input_admission() {
        validate_navigation_progress(NAVIGATION_POLICY, CONTINUATION, PROVIDER_REQUEST).unwrap();
        for boundary in ["receipt.hop != hop", "checkpoint.binding == binding"] {
            assert!(validate_navigation_progress(
                &NAVIGATION_POLICY.replace(boundary, "removed"),
                CONTINUATION,
                PROVIDER_REQUEST
            )
            .is_err());
        }
        for boundary in [
            ".validate_navigation_checkpoint(policy, call_request)?;",
            "policy.reject_unstructured_navigation_input(call_request)?;",
            "openai_text_message(\"developer\", checkpoint)",
            "crate::semantic_wire::model_safe_public_url(target)",
            "let text = progress.encode(observation)?;",
            "text.len() > MAX_AGENT_PROVIDER_INSPECTION_CHECKPOINT_BYTES",
            "conservative_request_measurement(&config, &body)?",
        ] {
            assert!(validate_navigation_progress(
                NAVIGATION_POLICY,
                CONTINUATION,
                &PROVIDER_REQUEST.replace(boundary, "removed")
            )
            .is_err());
        }
    }

    #[test]
    fn current_controller_boundary_is_valid() {
        validate_manifest(MANIFEST).expect("controller manifest");
        validate_root(ROOT).expect("controller root");
        validate_probe(PROBE).expect("controller probe");
        validate_terra(TERRA).expect("controller Terra path");
        validate_action(ACTION, ACTION_REFUSAL).expect("controller native action path");
        validate_work(WORK, WORK_DECISION).expect("production Work actor");
        validate_account_refresh(TERRA, WORK, WORK_DECISION, POLICY)
            .expect("trusted account sampling");
        validate_form(FORM).expect("production trusted form contract");
        validate_navigation(NAVIGATION, NAVIGATION_POLICY, CONTINUATION)
            .expect("exact document continuation");
        validate_scoped_extraction(READ, CONTINUATION, POLICY).expect("scoped extraction proof");
        validate_workflow_qualifier(QUALIFIER).expect("same-driver native qualification path");
        validate_work_actor_qualifier(WORK_QUALIFIER)
            .expect("actual actor/native/store qualification path");
    }

    #[test]
    fn navigation_cannot_restore_replay_widen_target_or_replace_run_owners() {
        for removed in [
            "navigation_route.as_ref() != approved_route",
            "navigation_target.is_some() && navigation_route.is_some()",
            "route.departure() != &input.context.target",
            "self.task.navigation_route() != self.navigation_route.as_ref()",
            "route.destinations().get(self.navigation_hops)",
        ] {
            assert!(validate_work(&WORK.replace(removed, "removed"), WORK_DECISION).is_err());
        }
        validate_navigation_route_contract(RUN_MANIFEST).unwrap();
        for removed in [
            "MAX_AGENT_NAVIGATION_ROUTE_HOPS: usize = 2",
            "destinations.is_empty() || destinations.len() > MAX_AGENT_NAVIGATION_ROUTE_HOPS",
            "target == &departure || destinations[..index - 1].contains(target)",
            "std::iter::once(route.departure()).chain(route.destinations())",
            "ZEPHIUM-AGENT-NAVIGATION-ROUTES-1",
        ] {
            assert!(
                validate_navigation_route_contract(&RUN_MANIFEST.replace(removed, "removed"))
                    .is_err()
            );
        }
        for removed in [
            "state.refresh_account(worker, browser)?",
            ".begin_navigation(id, op)",
            "state.task_progress(&fresh)?",
            "account.observed_at() < receipt.settled_at()",
            "self.navigation_refusal.ok_or_else(|| crate::contract!())?",
            "!= Some(refusal.operation)",
        ] {
            assert!(validate_navigation(
                &NAVIGATION.replace(removed, "removed"),
                NAVIGATION_POLICY,
                CONTINUATION
            )
            .is_err());
        }
        for removed in [
            ".admits_final_document(&active.row.target, target)",
            "!is_document_successor(",
            "*slot = Some(receipt)",
            "hop != self.navigation_receipts.iter().flatten().count()",
            "route.destinations().get(hop) != Some(target)",
            "prior.operation().context() == observation.request().context()",
            "request.account.observed_at() >= prior.settled_at()",
        ] {
            assert!(validate_navigation(
                NAVIGATION,
                &NAVIGATION_POLICY.replace(removed, "removed"),
                CONTINUATION
            )
            .is_err());
        }
        for forbidden in [
            "self.policy =",
            "self.deadline =",
            "self.turns =",
            "try_new_with_redirect_policy",
        ] {
            assert!(validate_navigation(
                &format!("{NAVIGATION}\n{forbidden}"),
                NAVIGATION_POLICY,
                CONTINUATION
            )
            .is_err());
        }
        assert!(validate_navigation(
            &NAVIGATION.replace(
                "let dispatch = if let Some(retained)",
                "journal.navigation_active(active); let dispatch = if let Some(retained)"
            ),
            NAVIGATION_POLICY,
            CONTINUATION
        )
        .is_err());
        assert!(validate_navigation(
            NAVIGATION,
            NAVIGATION_POLICY,
            &CONTINUATION.replace(
                "pub struct AgentProviderNavigationCheckpoint {",
                "pub struct AgentProviderNavigationCheckpoint { transcript: String,"
            )
        )
        .is_err());
    }

    #[test]
    fn account_sampling_cannot_renew_time_switch_identity_or_skip_a_boundary() {
        for removed in [
            "self.validate_account_update(account, self.account.context())",
            "account.context() != expected_context",
            "account.account() != self.account.account()",
            "self.account_attestations.contains(&account.attestation())",
            "self.failure = Some(error)",
        ] {
            assert!(validate_account_refresh(
                &TERRA.replace(removed, "removed"),
                WORK,
                WORK_DECISION,
                POLICY
            )
            .is_err());
        }
        for removed in [
            "state.refresh_account(worker, browser)?",
            "self.task.attest_account(context, now);",
            "self.native.check_control(worker, browser)?",
        ] {
            assert!(validate_account_refresh(
                TERRA,
                &WORK.replacen(removed, "removed", 1),
                WORK_DECISION,
                POLICY
            )
            .is_err());
        }
        assert!(validate_account_refresh(
            TERRA,
            WORK,
            WORK_DECISION,
            &POLICY.replace("u64 = 30_000;", "u64 = 600_000;")
        )
        .is_err());
        assert!(validate_account_refresh(
            &TERRA.replace(
                "pub fn refresh_account(",
                "pub fn refresh_account( /* self.deadline = */"
            ),
            WORK,
            WORK_DECISION,
            POLICY
        )
        .is_err());
    }

    #[test]
    fn app_view_admissions_each_retain_their_account_sample() {
        validate_account_refresh(TERRA, WORK, WORK_DECISION, POLICY)
            .expect("36 reviewed admission samples");
        for (start, end) in [
            ("async fn read_app_rows(", "async fn open_app_view("),
            ("async fn open_app_view(", "async fn read_app_look("),
        ] {
            let begin = WORK.find(start).expect("admission function");
            let finish = begin + WORK[begin..].find(end).expect("next function");
            let path = &WORK[begin..finish];
            // The last sample in each function is the one introduced for
            // successive view observations by 2f873db4 and kept by 8249cf5c.
            let sample = "state.refresh_account(worker, browser)?;";
            let at = begin + path.rfind(sample).expect("new view sample");
            let mut invalid = WORK.to_owned();
            invalid.replace_range(at..at + sample.len(), "");
            assert!(validate_account_refresh(TERRA, &invalid, WORK_DECISION, POLICY).is_err());
        }
    }

    #[test]
    fn form_task_cannot_lose_value_ref_or_freshness_binding_or_gain_content_sinks() {
        for boundary in [
            "action.fill_text() != Some(&goal.value)",
            "action.bound_action().option_reference() != binding.option",
            "snapshot.generation() <= last.snapshot",
            "self.refused = true;",
            "SemanticScope::Initial",
        ] {
            assert!(validate_form(&FORM.replace(boundary, "removed_boundary")).is_err());
        }
        for forbidden in [
            "tracing::info!",
            "serde::Serialize",
            "AgentRuntimeBrowser",
            "AgentRunPolicy",
            "tokio::spawn",
        ] {
            assert!(validate_form(&format!("{FORM}\n{forbidden}")).is_err());
        }
    }

    #[test]
    fn baseline_read_cannot_lose_policy_or_gain_capture_authority() {
        for required in [
            "!self.config.permits_baseline_read()",
            ".bind_read_request(request, &self.config, &read, payload)",
            "AgentProviderReadContinuationRequestDraft::try_new(bound)",
        ] {
            assert!(validate_terra(&TERRA.replace(required, "removed_boundary")).is_err());
        }
        for forbidden in ["invoke_semantic", "begin_expansion", "from_fingerprint"] {
            assert!(validate_terra(&TERRA.replace(
                "pub async fn continue_after_read(",
                &format!("pub async fn continue_after_read( /* {forbidden} */")
            ))
            .is_err());
        }
        assert!(validate_work(
            &WORK.replace(
                "self.task.allows_baseline_read() != self.baseline_read",
                "false"
            ),
            WORK_DECISION
        )
        .is_err());
    }

    #[test]
    fn scoped_read_cannot_lose_anchor_delivery_or_fresh_read_taint_binding() {
        for boundary in [
            "if &exact != request",
            "frame.generation() <= anchor.snapshot_generation()",
            "root.key() != anchor.capability().node_key()",
            "ZEPHIUM-SEMANTIC-SUBTREE-READ-GUARD-1",
        ] {
            assert!(validate_scoped_extraction(
                &READ.replace(boundary, "removed_boundary"),
                CONTINUATION,
                POLICY
            )
            .is_err());
        }
        for boundary in [
            "read.matches_subtree(&self.baseline, *target)",
            "subtree_target: Option<crate::SemanticReferenceId>",
        ] {
            assert!(validate_scoped_extraction(
                READ,
                &CONTINUATION.replace(boundary, "removed_boundary"),
                POLICY
            )
            .is_err());
        }
        for boundary in [
            "!read.matches_subtree(baseline, target)",
            "cohort.contains_reference(target)",
            "fragment.provenance().origin() != anchor.origin()",
        ] {
            assert!(validate_scoped_extraction(
                READ,
                CONTINUATION,
                &POLICY.replace(boundary, "removed_boundary")
            )
            .is_err());
        }
    }

    #[test]
    fn actor_qualification_requires_actual_runtime_native_and_durable_closure() {
        for boundary in [
            "pending.bind_browser_port(port)",
            "zephium_store::SqliteStore::open(",
            "AgentBrowserShutdownOutcome::Clean(_)",
            "AgentWorkOutcome::Succeeded(settlement)",
            "impl AgentWorkTask for PublicPreparedResultTask",
            "Some(result.observation()) != self.ready",
            "preview.source_bytes() != value.as_str().len()",
            "preview.text() != value.as_str()",
            "self.extraction.accept_extraction(result)",
        ] {
            assert!(validate_work_actor_qualifier(
                &WORK_QUALIFIER.replace(boundary, "removed_boundary")
            )
            .is_err());
        }
        assert!(
            validate_work_actor_qualifier(&format!("{WORK_QUALIFIER}\nimpl AgentBrowserPort"))
                .is_err()
        );
    }

    #[test]
    fn loading_classifier_admits_only_the_exact_bounded_boolean_read() {
        validate_work(WORK, WORK_DECISION).expect("reviewed loading classifier");
        for changed in [
            LOADING_CLASSIFIER.replace("nodes().count() <= 16", "nodes().count() <= 17"),
            LOADING_CLASSIFIER.replace("nodes().count() <= 16", "true"),
            LOADING_CLASSIFIER.replace(".as_str()", ".as_str().as_str()"),
            LOADING_CLASSIFIER.replace("words == \"loading\"", "words.contains(\"loading\")"),
            LOADING_CLASSIFIER.replace(
                "node.name(), node.text()",
                "node.name(), node.text(), node.text()",
            ),
        ] {
            assert_ne!(changed, LOADING_CLASSIFIER);
            assert!(validate_work(
                &WORK.replacen(LOADING_CLASSIFIER, &changed, 1),
                WORK_DECISION,
            )
            .is_err());
        }
        assert!(validate_work(&format!("{WORK}\n{LOADING_CLASSIFIER}"), WORK_DECISION).is_err());
        assert!(validate_work(
            &format!("{WORK}\nfn other_read() {{ page.as_str(); }}"),
            WORK_DECISION
        )
        .is_err());
    }

    #[test]
    fn work_children_cannot_acquire_forbidden_authority() {
        for child in [
            include_str!("../../crates/zephium-agent-controller/src/work_listing.rs"),
            include_str!("../../crates/zephium-agent-controller/src/work_compound_form.rs"),
        ] {
            super::validate_work_child(child).expect("reviewed Work actor child");
            for forbidden in [
                "fn f() { let _ = name.as_str(); }",
                "unsafe {}",
                "std::thread",
            ] {
                assert!(super::validate_work_child(&format!("{child}\n{forbidden}")).is_err());
            }
        }
    }

    #[test]
    fn fill_suggestion_and_look_digest_admit_only_their_exact_reads() {
        validate_work(WORK, WORK_DECISION).expect("reviewed fill and digest reads");
        for (from, to) in [
            (".fill_text()", ".target_name()"),
            (
                ".map(|text| text.as_str().to_owned())",
                ".map(|text| text.as_str().to_uppercase())",
            ),
            (".filter(|_| state.progressive_observation)", ""),
            (
                "semantic_sole_choice(&current, typed)",
                "semantic_sole_choice(&observation, typed)",
            ),
        ] {
            assert!(WORK.contains(from));
            assert!(validate_work(&WORK.replacen(from, to, 1), WORK_DECISION).is_err());
        }
        assert!(validate_work(
            &format!("{WORK}\nfn f(t: &str) {{ zephium_agentic::semantic_sole_choice(&l, t); }}"),
            WORK_DECISION
        )
        .is_err());
        for (from, to) in [
            (
                "node.text().map(|text| text.as_str()).hash(&mut hasher);",
                "node.text().map(|text| text.as_str()).hash(&mut hasher);\n        node.value().map(|value| value.as_str()).hash(&mut hasher);",
            ),
            ("-> u64 {\n    use std::hash", "-> String {\n    use std::hash"),
            (LOOK_DIGEST_USE, "let settled = look_digest(&next) <= look_digest(&current);"),
        ] {
            assert!(WORK_DECISION.contains(from));
            assert!(validate_work(WORK, &WORK_DECISION.replacen(from, to, 1)).is_err());
        }
        assert!(validate_work(
            WORK,
            &format!(
                "{WORK_DECISION}\nfn leak(o: &SemanticObservation) -> u64 {{ look_digest(o) }}"
            )
        )
        .is_err());
    }

    #[test]
    fn mutation_cannot_reenable_default_transport_or_content_reading() {
        assert!(validate_manifest(
            &MANIFEST.replace("default = []", "default = [\"provider-transport\"]")
        )
        .is_err());
        assert!(validate_terra(&format!("{TERRA}\nlet _ = delta.as_str();")).is_err());
        // The admitted detector read stays single, bounded and exact.
        let detector = ".map(|text| text.as_str().trim())";
        assert!(validate_work(
            &WORK.replacen(detector, ".map(|text| text.as_str().as_str().trim())", 1),
            WORK_DECISION
        )
        .is_err());
        assert!(validate_work(
            &WORK.replacen(".filter(|text| text.len() <= 96)", ".filter(|_| true)", 1),
            WORK_DECISION
        )
        .is_err());
        assert!(validate_work(&format!("{WORK}\nlet _ = page.as_str();"), WORK_DECISION).is_err());
        let decision = include_str!("../../crates/zephium-agent-controller/src/decision.rs");
        validate_decision_routing(decision).expect("decision routing");
        assert!(validate_decision_routing(&format!("{decision}\nstd::thread::spawn(f);")).is_err());
        let vocabulary = super::NAVIGATION_VOCABULARY;
        assert!(validate_navigation(
            &format!("{NAVIGATION}\n{vocabulary}"),
            NAVIGATION_POLICY,
            CONTINUATION
        )
        .is_err());
    }

    #[test]
    fn unsuccessful_drain_cannot_bypass_original_debt_or_claim_business_success() {
        for visibility in ["pub ", "pub(crate) ", "pub(super) "] {
            assert!(validate_terra(&TERRA.replace(
                "fn try_finish_unsuccessful(",
                &format!("{visibility}fn try_finish_unsuccessful("),
            ))
            .is_err());
        }
        for boundary in [
            "self.retained_terminal.is_some()",
            "self.navigation_refusal.is_some()",
            "self.action_admission_failure.is_some()",
            "!unsuccessful || refusal.human_review().is_none()",
            "else if !unsuccessful",
        ] {
            assert!(validate_terra(&TERRA.replace(boundary, "removed_boundary")).is_err());
        }
        for boundary in [
            "state.native.deferred.is_empty()",
            "let _ = session.settle_navigation_refusal();",
            "state.drained.is_some()",
            "if state.terminal_intent.replace(terminal_intent).is_some() {",
            "let terminal_intent = state.terminal_intent.ok_or_else(|| crate::contract!())?;",
            "let closure_matches_intent = match terminal_intent {",
            "closure.outcome() != AgentRunProgressOutcome::Succeeded",
            "if !closure_matches_intent {",
            "claim.commit_with_shutdown(proof, settlement, provider)",
            ".record_human_refusal(",
            "proposal_refusal: action_proposal_failure",
            "AgentBrowserActionProposalRefusal::discard_after_closure",
        ] {
            assert!(
                validate_work(&WORK.replace(boundary, "removed_boundary"), WORK_DECISION).is_err()
            );
        }
    }

    #[test]
    fn combined_task_cannot_bypass_frozen_phase_or_current_result_binding() {
        for boundary in [
            "self.task.extraction_schema() != self.extraction_schema.as_ref()",
            "self.task.allows_actions_before_extraction() != self.actions_before_extraction",
            "(input.durable_result || actions_before_extraction || subtree_extraction)",
            "&& extraction_schema.is_none()",
            "self.task.allows_subtree_extraction() != self.subtree_extraction",
            ".begin_extraction_subtree(",
            "Self::capture_once(state, worker, browser, request, frame).await?",
            "expanded.as_ref().map(|_| observation)",
            "session.config.restrict_to_scoped_extraction()",
            "session.config.restrict_to_actions_and_scoped_extraction()",
            "progress == AgentWorkTaskProgress::Complete && self.actions_before_extraction",
            "progress != AgentWorkTaskProgress::ReadyForExtraction",
            "progress == AgentWorkTaskProgress::ReadyForExtraction",
            "session.config.restrict_to_actions_and_extraction()",
            "captured_at = current_at;",
            "session.extract_from_with_evidence(",
            "if session.turns >= session.max_model_calls",
            "state.task.accept_extraction(&result)? != AgentWorkTaskProgress::Complete",
        ] {
            // The actor spans both files, so a boundary is removed from each.
            assert!(validate_work(
                &WORK.replace(boundary, "removed_boundary"),
                &WORK_DECISION.replace(boundary, "removed_boundary"),
            )
            .is_err());
        }
        for boundary in [
            ".verify_action_settlement(",
            "SemanticSettleInstant::from_millis(looked.millis()),",
            "SemanticCaptureInstant::from_millis(looked.millis()),",
            ".policy_now()\n            .map_err(AgentWorkFailure::Browser)\n    }",
        ] {
            assert!(WORK_DECISION.contains(boundary));
            assert!(
                validate_work(WORK, &WORK_DECISION.replace(boundary, "removed_boundary")).is_err()
            );
        }
        // Every look in settlement is dated immediately before it is taken.
        for look in [
            "match Self::observe(state, worker, browser)",
            "current = Self::observe(state, worker, browser)",
            "let next = Self::observe(state, worker, browser)",
        ] {
            let dated = format!("looked = Self::look_begins(state)?;\n            {look}");
            assert!(WORK_DECISION.contains(&dated));
            assert!(validate_work(WORK, &WORK_DECISION.replace(&dated, look)).is_err());
            assert!(validate_work(
                WORK,
                &WORK_DECISION.replace(
                    &dated,
                    &format!("{look}.await?;\n            looked = Self::look_begins(state)?;\n            {look}")
                )
            )
            .is_err());
        }
        assert!(validate_work(
            WORK,
            &format!("{WORK_DECISION}\nfn f() {{ std::thread::yield_now() }}")
        )
        .is_err());
        // The consent press's settled look is dated as it begins, too.
        let undated = "match Self::observe(state, worker, browser)";
        assert!(WORK_DECISION.contains(CONSENT_LOOK));
        for consent in [
            undated.to_owned(),
            format!("let looked = Self::look_begins(state)?;\n            Self::observe(state, worker, browser).await?;\n            {undated}"),
            format!("let looked = policy_later(state)?;\n            {undated}"),
        ] {
            assert!(
                validate_work(WORK, &WORK_DECISION.replacen(CONSENT_LOOK, &consent, 1)).is_err(),
                "{consent}"
            );
        }
        assert!(validate_work(
            WORK,
            &WORK_DECISION.replacen(
                "result => return Ok((result?, looked)),",
                "result => return Ok((result?, Self::look_begins(state)?)),",
                1
            )
        )
        .is_err());
        assert!(validate_work(
            WORK,
            &WORK_DECISION.replacen("async fn observe_settled(", "async fn observe_later(", 1)
        )
        .is_err());
    }

    #[test]
    fn mutation_cannot_bypass_action_authority_or_drop_pending_ownership() {
        for boundary in [
            "authorize_semantic_effect",
            "dispatch_semantic_effect",
            "verify_semantic_action_terminal",
            "settle_verified_semantic_terminal",
            "finalize_accounted_semantic_action_result",
            "self.pending = Some",
        ] {
            assert!(
                validate_action(
                    &ACTION.replace(boundary, "removed_boundary"),
                    ACTION_REFUSAL
                )
                .is_err(),
                "{boundary}"
            );
        }
        assert!(validate_action(
            &format!("{ACTION}\nSemanticModelActionQualificationExecution"),
            ACTION_REFUSAL
        )
        .is_err());
        for boundary in [
            "config != &continuation.config",
            "!continuation.baseline.matches(observation)",
            "actions.iter().find_map(|action| {",
            "targets.permitted_operations(context.target)",
            "SemanticReferenceError::OperationDenied",
            "SemanticActionBatch::bind",
        ] {
            assert!(
                validate_action(
                    ACTION,
                    &ACTION_REFUSAL.replace(boundary, "removed_boundary")
                )
                .is_err(),
                "{boundary}"
            );
        }
    }

    #[test]
    fn workflow_qualifier_cannot_replace_real_policy_or_claim_partial_success() {
        for boundary in [
            "AgentBrowserSession::try_new",
            ".authorize_action(",
            ".settle_action(",
            "workflow.progress.observe(initial, final_query, language)",
            "workflow.session = Some(refusal.into_session())",
            "terminal.model_receipts().len() != workflow.metrics.len()",
        ] {
            assert!(
                validate_workflow_qualifier(&QUALIFIER.replace(boundary, "removed_boundary"))
                    .is_err()
            );
        }
    }

    #[test]
    fn reinspection_cannot_restore_execution_or_relax_independent_evidence() {
        let source = include_str!("../../crates/zephium-agent-controller/src/work_reinspection.rs");
        validate_reinspection(source).unwrap();
        for guard in [
            "binding.document() != &target.document",
            "account.account() != self.source_account.account()",
            "account.attestation() == self.source_account.attestation()",
            "matches.next().is_some()",
            "value.truncated()",
            "unmatched: Some((Box::new(self), Box::new(completion)))",
        ] {
            assert!(validate_reinspection(&source.replace(guard, "removed")).is_err());
        }
        assert!(
            validate_reinspection(&format!("{source}\ncontinue_after_verified_action")).is_err()
        );
    }
}
