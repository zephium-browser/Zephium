//! Shared fixtures and settings for native Work qualification.
#[cfg(feature = "durable-runtime")]
use std::io::Write as _;
use std::sync::Arc;
#[cfg(feature = "durable-runtime")]
use std::time::Instant;
#[cfg(feature = "durable-runtime")]
use zephium_agentic::AgentProviderTransportConfig;
#[cfg(feature = "durable-runtime")]
use zephium_core::work::runtime::WorkRuntimeProjection;
#[cfg(feature = "durable-runtime")]
use zephium_work_composition::durable_runtime::WorkBrowserAdapterSettings;
#[cfg(feature = "durable-runtime")]
fn probe_clock() -> &'static Instant {
    static CLOCK: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    CLOCK.get_or_init(Instant::now)
}
#[cfg(feature = "durable-runtime")]
fn say(line: std::fmt::Arguments<'_>) {
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

#[cfg(feature = "durable-runtime")]
pub(super) struct Scripted {
    pub(super) start: &'static str,
}

#[cfg(feature = "durable-runtime")]
impl zephium_core::work::model::WorkModelClient for Scripted {
    fn call<'a>(
        &'a self,
        request: zephium_core::work::model::WorkModelRequest,
        _: &'a (dyn Fn(zephium_core::work::model::WorkModelEvent) + Send + Sync),
    ) -> zephium_core::work::model::WorkModelFuture<'a> {
        use zephium_core::work::model::*;
        let answered = request
            .messages
            .iter()
            .any(|message| matches!(message, WorkModelMessage::ToolResults(_)));
        let lead = request.tools.iter().any(|tool| tool.name == "start_part");
        let goal = request
            .messages
            .iter()
            .find_map(|message| match message {
                WorkModelMessage::User(parts) => parts.iter().find_map(|part| match part {
                    WorkModelPart::Text(text) => text
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("Request: ")
                                .or_else(|| line.strip_prefix("Goal: "))
                        })
                        .map(str::to_owned),
                    _ => None,
                }),
                _ => None,
            })
            .unwrap_or_default();
        let starts: Vec<&str> = self.start.split(' ').filter(|s| !s.is_empty()).collect();
        // A helper's brief names its part: "Site 2" browses the second start.
        let part = request
            .messages
            .iter()
            .find_map(|message| match message {
                WorkModelMessage::User(parts) => parts.iter().find_map(|part| match part {
                    WorkModelPart::Text(text) => text
                        .lines()
                        .find_map(|line| line.strip_prefix("Part: Site "))
                        .and_then(|rest| rest.split(' ').next()?.parse::<usize>().ok()),
                    _ => None,
                }),
                _ => None,
            })
            .unwrap_or(1);
        let start = starts.get(part - 1).copied().unwrap_or(self.start);
        if lead && !answered {
            let calls = (1..=starts.len())
                .map(|index| {
                    WorkModelPart::ToolCall(WorkModelToolCall {
                        id: format!("part-{index}"),
                        name: "start_part".into(),
                        arguments: serde_json::json!({"title": format!("Site {index}"),
                            "helper": "browser",
                            "goal": format!("Page {index}: {}", goal.chars().take(180).collect::<String>()),
                            "brief": goal}),
                    })
                })
                .collect();
            return Box::pin(async move {
                Ok(WorkModelOutcome {
                    stop: WorkModelStop::ToolUse,
                    usage: WorkModelUsage {
                        cost_micros: Some(0),
                        ..WorkModelUsage::default()
                    },
                    assistant: calls,
                })
            });
        }
        let (name, arguments) = match (lead, answered) {
            (true, _) => ("finish", serde_json::json!({"say": "Checked the page."})),
            (false, false) => (
                "browse",
                serde_json::json!({"start": start, "goal": goal, "mine": true, "view": start.contains(".probe.test/"), "records": {
                "title": "Results", "max_items": 8, "columns": [
                    {"name": "price", "value": {"kind": "text"}, "required": false, "extraction": "verbatim"},
                    {"name": "rating", "value": {"kind": "text"}, "required": false, "extraction": "verbatim"},
                    {"name": "details", "value": {"kind": "text"}, "required": false, "extraction": "generate"},
                    {"name": "url", "value": {"kind": "url"}, "required": false, "extraction": "generate"},
                    {"name": "photo", "value": {"kind": "image_url"}, "required": false, "extraction": "generate"}
                ]}}),
            ),
            (false, true) => (
                "finish",
                serde_json::json!({"summary": "Checked", "digest": "Checked the page."}),
            ),
        };
        Box::pin(async move {
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    cost_micros: Some(0),
                    ..WorkModelUsage::default()
                },
                assistant: vec![WorkModelPart::ToolCall(WorkModelToolCall {
                    id: "scripted".into(),
                    name: name.into(),
                    arguments,
                })],
            })
        })
    }
}

#[cfg(feature = "durable-runtime")]
pub(super) fn run_row(
    name: &str,
    index: usize,
    execution: &zephium_core::work::runtime::WorkExecutionFact,
    wall_ms: u128,
) {
    use zephium_core::work::runtime::*;
    let usage = execution.attempts.first().and_then(|a| a.usage);
    let mut kinds: Vec<&str> = execution
        .steps
        .iter()
        .filter(|s| matches!(s.kind, WorkStepKindV1::Publish))
        .flat_map(|s| s.artifacts.iter())
        .filter_map(|id| execution.artifacts.iter().find(|a| a.id == *id))
        .map(|a| a.data.kind_name())
        .collect();
    kinds.sort_unstable();
    let revised = execution
        .artifacts
        .iter()
        .filter(|a| a.revises.is_some())
        .count();
    say(format_args!(
        "lead-run: scenario={} request={} status={:?} wall_ms={} steps={} turns={} searches={} pages={} parts={} parts_done={} objects={} revised={} inputs={} tokens={} cost_micro_usd={} accounting={:?} asks={}",
        name,
        index,
        execution.status,
        wall_ms,
        execution.steps.len(),
        execution.steps.iter().filter(|s| matches!(s.kind, WorkStepKindV1::Turn)).count(),
        execution.steps.iter().filter(|s| matches!(s.kind, WorkStepKindV1::Search { .. })).count(),
        execution.steps.iter().filter(|s| matches!(s.kind, WorkStepKindV1::Read { .. })).count(),
        execution.parts.len(),
        execution
            .parts
            .iter()
            .filter(|p| p.state == zephium_core::work::parts::WorkPartStateV1::Done)
            .count(),
        kinds.join(","),
        revised,
        execution.inputs.len(),
        usage.map_or(0, |u| u.model_tokens),
        usage.map_or(0, |u| u.cost_micro_usd),
        usage.map(|u| u.accounting),
        execution.steps.iter().filter(|s| matches!(s.kind, WorkStepKindV1::Ask { .. })).count(),
    ));
}

/// One closed row per part: how it ended, what it needs, the site it
/// worked on and how many objects it placed.
#[cfg(feature = "durable-runtime")]
pub(super) fn part_rows(execution: &zephium_core::work::runtime::WorkExecutionFact) {
    use zephium_core::work::runtime::WorkStepKindV1;
    for part in &execution.parts {
        let need = part
            .need
            .as_ref()
            .and_then(|need| serde_json::to_value(need).ok())
            .map(|value| {
                let field = |key: &str| {
                    value
                        .get(key)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned()
                };
                format!("{}/{}", field("kind"), field("reason"))
            });
        let objects = execution
            .steps
            .iter()
            .filter(|s| s.part == Some(part.id) && matches!(s.kind, WorkStepKindV1::Publish))
            .map(|s| s.artifacts.len())
            .sum::<usize>();
        say(format_args!(
            "lead-part: helper={:?} state={:?} need={} site={} objects={objects}",
            part.helper,
            part.state,
            need.as_deref().unwrap_or("none"),
            part.service
                .as_ref()
                .and_then(|service| service.host.as_deref())
                .unwrap_or("-"),
        ));
    }
    object_rows(execution);
}

/// One closed row per object the run placed: its kind, whether a part
/// placed it, and for picks how many items carry a price, a picture and a
/// link.
#[cfg(feature = "durable-runtime")]
fn object_rows(execution: &zephium_core::work::runtime::WorkExecutionFact) {
    use zephium_core::work::artifact::WorkArtifactDataV1 as Data;
    use zephium_core::work::runtime::WorkStepKindV1;
    for artifact in execution
        .steps
        .iter()
        .filter(|s| matches!(s.kind, WorkStepKindV1::Publish))
        .flat_map(|s| s.artifacts.iter())
        .filter_map(|id| execution.artifacts.iter().find(|a| a.id == *id))
    {
        let detail = match &artifact.data {
            Data::Picks { items, .. } => format!(
                " items={} priced={} pictured={} linked={} sources={}",
                items.len(),
                items.iter().filter(|i| i.price.is_some()).count(),
                items
                    .iter()
                    .filter(|i| !i.image_candidates.is_empty())
                    .count(),
                items.iter().filter(|i| i.url.is_some()).count(),
                artifact.evidence.len()
            ),
            Data::Sheet { rows, columns, .. } => {
                format!(
                    " rows={} columns={} sources={}",
                    rows.len(),
                    columns.len(),
                    artifact.evidence.len()
                )
            }
            _ => format!(" sources={}", artifact.evidence.len()),
        };
        say(format_args!(
            "lead-object: kind={} part={} revised={}{detail}",
            artifact.data.kind_name(),
            artifact.part.is_some(),
            artifact.revises.is_some(),
        ));
    }
}

#[cfg(feature = "durable-runtime")]
pub(super) struct WorkflowResult {
    pub(super) state: WorkRuntimeProjection,
    pub(super) failure: Option<&'static str>,
}

#[cfg(feature = "durable-runtime")]
pub(super) fn browser_settings(
    profile: zephium_app::AgentWorkProfileBinding,
    credential: zephium_agentic::AgentProviderCredential,
) -> WorkBrowserAdapterSettings {
    WorkBrowserAdapterSettings {
        decisions: zephium_work_composition::durable_runtime::WorkDecisionPreference::Recommended,
        decision_model: None,
        retain_public_responses: true,
        loopback_anonymous: false,
        refusal_diagnostic: None,
        stage_diagnostic: Some(|stage| {
            let _ = writeln!(
                std::io::stdout().lock(),
                "durable-work: stage={stage} at_ms={}",
                probe_clock().elapsed().as_millis()
            );
        }),
        model_diagnostic: Some(|event| {
            let _ = writeln!(std::io::stdout().lock(), "browser-model: {event:?}");
        }),
        resource_diagnostic: Some(|cause| {
            let _ = writeln!(
                std::io::stdout().lock(),
                "durable-work: resource_failure={cause:?}; content=redacted"
            );
        }),
        diagnostic: Some(|_, snapshot| {
            let _ = writeln!(
                std::io::stdout().lock(),
                "durable-work: native_phase={:?}; failure={:?}; persistence={:?}; content=redacted",
                snapshot.phase,
                snapshot.failure,
                snapshot.persistence_failure
            );
        }),
        profile,
        model: zephium_agent_controller::AgentBrowserModel::Gpt6Luna,
        config: zephium_app::AgentWorkApplicationConfig::new(
            zephium_agent_runtime::AgentRuntimeConfig::STANDARD,
            AgentProviderTransportConfig::STANDARD,
        ),
        access: credential.into(),
    }
}

pub(super) struct NoChrome;
impl zephium_core::ports::chrome::Chrome for NoChrome {
    fn position(&self, _: zephium_core::ports::chrome::ChromeFrame) -> bool {
        false
    }
}
impl zephium_app::PresentationChrome for NoChrome {
    fn apply_tab_for_presentation(
        &self,
        _: zephium_app::ChromePresentation,
        _: zephium_app::ChromePresentationCallback,
    ) -> zephium_app::ChromePresentationDispatch {
        zephium_app::ChromePresentationDispatch::Rejected
    }
}

pub(super) fn seeded_blocker(
    data: &std::path::Path,
) -> Result<Arc<zephium_blocker_service::ManagedBlocker>, super::ProbeFailure> {
    use zephium_blocker_service::{
        EmbeddedReleaseAsset, LicensePolicy, ReleaseCatalogSeed, UpdateLimits,
    };
    let seed = ReleaseCatalogSeed::from_embedded_gzip(
        include_bytes!("../../../assets/blocker-seed/v1/catalog.json"),
        include_bytes!("../../../assets/blocker-seed/v1/release-seed.json"),
        vec![
            EmbeddedReleaseAsset::new(
                "easylist.txt",
                include_bytes!("../../../assets/blocker-seed/v1/easylist.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "easyprivacy.txt",
                include_bytes!("../../../assets/blocker-seed/v1/easyprivacy.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-filters.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-filters.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-quick-fixes.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-quick-fixes.txt.gz"),
            ),
        ],
        UpdateLimits {
            max_manifest_bytes: 16 * 1024,
            max_sources: 4,
            max_source_bytes: 4 * 1024 * 1024,
            max_total_source_bytes: 8 * 1024 * 1024,
            ..UpdateLimits::default()
        },
        LicensePolicy::new(["CC-BY-SA-3.0", "GPL-3.0-only"])
            .map_err(|_| super::ProbeFailure::Authority)?,
    )
    .map_err(|_| super::ProbeFailure::Authority)?;
    zephium_blocker_service::ManagedBlocker::with_release_seed(
        seed,
        zephium_blocker::CompiledArtifactCacheConfig::new(data.join("compiled"))
            .map_err(|_| super::ProbeFailure::Authority)?,
    )
    .map_err(|_| super::ProbeFailure::Runtime)
}
