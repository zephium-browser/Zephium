//! Mechanical exclusion checks for agentic diagnostic facilities.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

const AGENTIC_MANIFEST: &str = "crates/zephium-agentic/Cargo.toml";
const AGENTIC_ROOT: &str = "crates/zephium-agentic/src/lib.rs";
const AGENTIC_FIXTURE_SERVER: &str = "crates/zephium-agentic/src/fixture_server.rs";
const AGENTIC_PROBE_EVIDENCE_PATH: &str = "crates/zephium-agentic/src/probe_evidence_path.rs";
const AGENTIC_PROVIDER_ROOT: &str = "crates/zephium-agentic/src/agent_provider.rs";
const AGENTIC_WINDOWS_REVIEW_BINARY: &str =
    "crates/zephium-agentic/src/bin/windows_agentic_input_evidence_review.rs";
const AGENTIC_WINDOWS_SEMANTIC_REVIEW_BINARY: &str =
    "crates/zephium-agentic/src/bin/windows_agentic_semantic_evidence_review.rs";
const AGENTIC_WINDOWS_SEMANTIC_EVIDENCE: &str =
    "crates/zephium-agentic/src/semantic_probe_evidence.rs";
const AGENTIC_PROBE_QUALIFICATION: &str = "crates/zephium-agentic/src/probe_qualification.rs";
const AGENTIC_PROVIDER_REQUEST: &str = "crates/zephium-agentic/src/agent_provider/request.rs";
const AGENTIC_PROVIDER_CONTINUATION: &str =
    "crates/zephium-agentic/src/agent_provider/continuation.rs";
const AGENTIC_PROVIDER_TOOL: &str = "crates/zephium-agentic/src/agent_provider/tool.rs";
const AGENTIC_PROVIDER_OPENAI: &str = "crates/zephium-agentic/src/agent_provider/openai.rs";
const AGENTIC_PROVIDER_ANTHROPIC: &str = "crates/zephium-agentic/src/agent_provider/anthropic.rs";
const AGENTIC_PROVIDER_PRICING: &str = "crates/zephium-agentic/src/agent_provider/pricing.rs";
const AGENTIC_PROVIDER_SETTLEMENT: &str = "crates/zephium-agentic/src/agent_provider/settlement.rs";
const AGENTIC_POLICY: &str = "crates/zephium-agentic/src/agent_policy.rs";
const AGENTIC_EFFECT_POLICY: &str = "crates/zephium-agentic/src/agent_policy/effect.rs";
const AGENTIC_AUDIT: &str = "crates/zephium-agentic/src/agent_audit.rs";
const AGENTIC_ACTION_METRICS: &str = "crates/zephium-agentic/src/agent_action_metrics.rs";
const AGENTIC_INPUT_METRICS: &str = "crates/zephium-agentic/src/agent_input_metrics.rs";
const AGENTIC_LIFECYCLE: &str = "crates/zephium-agentic/src/agent_lifecycle.rs";
const AGENTIC_METRIC_CLOSURE: &str = "crates/zephium-agentic/src/agent_metric_closure.rs";
const AGENTIC_METRICS: &str = "crates/zephium-agentic/src/agent_metrics.rs";
const AGENTIC_NATIVE_SHUTDOWN: &str = "crates/zephium-agentic/src/agent_native_shutdown.rs";
const AGENTIC_NATIVE_SHUTDOWN_DRIVER: &str =
    "crates/zephium-agentic/src/agent_native_shutdown_driver.rs";
const AGENTIC_PROGRESS_METRICS: &str = "crates/zephium-agentic/src/agent_progress_metrics.rs";
const AGENTIC_SEMANTIC: &str = "crates/zephium-agentic/src/semantic.rs";
const AGENTIC_SEMANTIC_WIRE: &str = "crates/zephium-agentic/src/semantic_wire.rs";
const AGENTIC_SEMANTIC_DIFF: &str = "crates/zephium-agentic/src/semantic_diff.rs";
const AGENTIC_SEMANTIC_DIFF_MODEL: &str = "crates/zephium-agentic/src/semantic_diff_model.rs";
const AGENTIC_SEMANTIC_LOCATE: &str = "crates/zephium-agentic/src/semantic_locate.rs";
const AGENTIC_SEMANTIC_LOCATE_MODEL: &str = "crates/zephium-agentic/src/semantic_locate_model.rs";
const AGENTIC_SEMANTIC_READ: &str = "crates/zephium-agentic/src/semantic_read.rs";
const AGENTIC_SEMANTIC_READ_MODEL: &str = "crates/zephium-agentic/src/semantic_read_model.rs";
const AGENTIC_SEMANTIC_EXTRACT: &str = "crates/zephium-agentic/src/semantic_extract.rs";
const AGENTIC_SEMANTIC_EXTRACT_MODEL: &str = "crates/zephium-agentic/src/semantic_extract_model.rs";
const AGENTIC_PROVIDER_EXTRACTION: &str = "crates/zephium-agentic/src/agent_provider/extraction.rs";
const AGENTIC_SEMANTIC_ACTION: &str = "crates/zephium-agentic/src/semantic_action.rs";
const AGENTIC_SEMANTIC_ACTION_BATCH_RESULT: &str =
    "crates/zephium-agentic/src/semantic_action_batch_result.rs";
const AGENTIC_SEMANTIC_ACTION_RESULT: &str = "crates/zephium-agentic/src/semantic_action_result.rs";
const AGENTIC_SEMANTIC_EXECUTE: &str = "crates/zephium-agentic/src/semantic_execute.rs";
const AGENTIC_SEMANTIC_EXECUTE_COORDINATOR: &str =
    "crates/zephium-agentic/src/semantic_execute_coordinator.rs";
const AGENTIC_SEMANTIC_SETTLE_COORDINATOR: &str =
    "crates/zephium-agentic/src/semantic_settle_coordinator.rs";
const AGENTIC_SEMANTIC_SETTLE: &str = "crates/zephium-agentic/src/semantic_settle.rs";
const AGENTIC_SEMANTIC_SCREENSHOT: &str = "crates/zephium-agentic/src/semantic_screenshot.rs";
const AGENTIC_SEMANTIC_VERIFY: &str = "crates/zephium-agentic/src/semantic_verify.rs";
const AGENTIC_CONTEXT_PORT: &str = "crates/zephium-agentic/src/context_port.rs";
const AGENTIC_CONTEXT_REGISTRY: &str = "crates/zephium-agentic/src/context_registry.rs";
const AGENTIC_COOKIE_TRANSFER: &str = "crates/zephium-agentic/src/cookie_transfer.rs";
const AGENTIC_PROFILE_LEASE: &str = "crates/zephium-agentic/src/profile_lease.rs";
const AGENTIC_SUPERVISOR: &str = "crates/zephium-agentic/src/agent_supervisor.rs";
const AGENTIC_SUPERVISOR_RUNTIME: &str = "crates/zephium-agentic/src/agent_supervisor/runtime.rs";
const AGENTIC_SUPERVISOR_PROGRESS: &str =
    "crates/zephium-agentic/src/agent_supervisor/runtime/progress.rs";
const AGENTIC_SUPERVISOR_CONTEXT_SCHEDULE: &str =
    "crates/zephium-agentic/src/agent_supervisor/runtime/context_schedule.rs";
const APP_MANIFEST: &str = "crates/zephium-app/Cargo.toml";
const APP_ROOT: &str = "crates/zephium-app/src/lib.rs";
const APP_API: &str = "crates/zephium-app/src/api.rs";
const APP_ACTOR: &str = "crates/zephium-app/src/actor/mod.rs";
const APP_SHELL: &str = "crates/zephium-app/src/shell/mod.rs";
const DESKTOP_MANIFEST: &str = "desktop/Cargo.toml";
const STORE_ACTOR: &str = "crates/zephium-store/src/actor.rs";
const STORE_AGENT_AUDIT_ACTOR: &str = "crates/zephium-store/src/actor/agent_audit.rs";
const STORE_AGENT_AUDIT_HUB: &str = "crates/zephium-store/src/hub/agent_audit.rs";
const PROVIDER_TRANSPORT_MANIFEST: &str = "crates/zephium-agentic/Cargo.toml";
const PROVIDER_TRANSPORT_ROOT: &str = "crates/zephium-agentic/src/provider_transport.rs";
const PROVIDER_TRANSPORT_FACADE_MANIFEST: &str =
    "crates/zephium-agent-provider-transport/Cargo.toml";
const PROVIDER_TRANSPORT_FACADE_ROOT: &str = "crates/zephium-agent-provider-transport/src/lib.rs";
const ENGINE_MANIFEST: &str = "crates/zephium-engine/Cargo.toml";
const ENGINE_ROOT: &str = "crates/zephium-engine/src/lib.rs";
const ENGINE_PLATFORM_MODULE: &str = "crates/zephium-engine/src/platform/mod.rs";
const ENGINE_HOST_ROOT: &str = "crates/zephium-engine/src/host/mod.rs";
const ENGINE_AGENT_CONTEXT_PORT: &str = "crates/zephium-engine/src/agent_context_port.rs";
const ENGINE_AGENT_CONTEXT_HOST: &str = "crates/zephium-engine/src/host/agent_context.rs";
const ENGINE_AGENT_COOKIE_SOURCE: &str = "crates/zephium-engine/src/host/agent_cookie_source.rs";
const ENGINE_MACOS_MODULE: &str = "crates/zephium-engine/src/platform/macos/mod.rs";
const ENGINE_MACOS_AGENT_CONTEXT: &str =
    "crates/zephium-engine/src/platform/macos/agent_context.rs";
const ENGINE_MACOS_SEMANTIC_RUNTIME: &str =
    "crates/zephium-engine/src/platform/macos/semantic_runtime.rs";
const ENGINE_MACOS_SEMANTIC_ACTION: &str =
    "crates/zephium-engine/src/platform/macos/semantic_action.rs";
const ENGINE_MACOS_SEMANTIC_SCREENSHOT: &str =
    "crates/zephium-engine/src/platform/macos/semantic_screenshot.rs";
const ENGINE_MACOS_SEMANTIC_PROBE: &str =
    "crates/zephium-engine/src/platform/macos/agentic_semantic_probe.rs";
const ENGINE_MACOS_RENDERING_PROBE: &str =
    "crates/zephium-engine/src/platform/macos/agentic_rendering_probe.rs";
const ENGINE_MACOS_RENDERING_OPPORTUNITY_PROBE: &str =
    "crates/zephium-engine/src/platform/macos/agentic_rendering_opportunity_probe.rs";
const ENGINE_MACOS_RENDERING_PRESENTED_PROBE: &str =
    "crates/zephium-engine/src/platform/macos/agentic_rendering_presented_probe.rs";
const ENGINE_WINDOWS_MODULE: &str = "crates/zephium-engine/src/platform/windows/mod.rs";
const ENGINE_WINDOWS_AGENT_CONTEXT: &str =
    "crates/zephium-engine/src/platform/windows/agent_context.rs";
const ENGINE_WINDOWS_AGENT_TIMEOUT: &str = "crates/zephium-engine/src/platform/windows/timeout.rs";
const ENGINE_AGENT_NAVIGATION: &str = "crates/zephium-engine/src/platform/agent_navigation.rs";
const ENGINE_AGENT_SUSPENSION: &str = "crates/zephium-engine/src/platform/agent_suspension.rs";
const ENGINE_AGENT_SCREENSHOT_BUFFER: &str =
    "crates/zephium-engine/src/platform/agent_screenshot_buffer.rs";
const ENGINE_AGENT_COOKIE_PREFLIGHT: &str =
    "crates/zephium-engine/src/platform/agent_cookie_preflight.rs";
const ENGINE_WINDOWS_COOKIE_TRANSFER: &str =
    "crates/zephium-engine/src/platform/windows/cookie_transfer.rs";
const ENGINE_WINDOWS_SEMANTIC_PROTOCOL: &str =
    "crates/zephium-engine/src/platform/agent_semantic_cdp_protocol.rs";
const ENGINE_WINDOWS_SEMANTIC_RUNTIME: &str =
    "crates/zephium-engine/src/platform/windows/semantic_runtime.rs";
const ENGINE_WINDOWS_SEMANTIC_SCREENSHOT: &str =
    "crates/zephium-engine/src/platform/windows/semantic_screenshot.rs";
const ENGINE_HOST_CONTENT_RULES: &str = "crates/zephium-engine/src/host/content_rules.rs";
const ENGINE_AGENTIC_NATIVE_UNSAFE_MODULES: [&str; 9] = [
    ENGINE_MACOS_AGENT_CONTEXT,
    ENGINE_MACOS_SEMANTIC_RUNTIME,
    ENGINE_MACOS_SEMANTIC_ACTION,
    ENGINE_MACOS_SEMANTIC_SCREENSHOT,
    ENGINE_WINDOWS_AGENT_CONTEXT,
    ENGINE_WINDOWS_COOKIE_TRANSFER,
    ENGINE_WINDOWS_SEMANTIC_RUNTIME,
    ENGINE_WINDOWS_SEMANTIC_SCREENSHOT,
    ENGINE_WINDOWS_AGENT_TIMEOUT,
];
const ENGINE_AGENTIC_PRODUCTION_MODULES: [&str; 17] = [
    ENGINE_AGENT_CONTEXT_PORT,
    ENGINE_AGENT_CONTEXT_HOST,
    ENGINE_AGENT_COOKIE_SOURCE,
    ENGINE_AGENT_NAVIGATION,
    ENGINE_AGENT_SUSPENSION,
    ENGINE_AGENT_SCREENSHOT_BUFFER,
    ENGINE_AGENT_COOKIE_PREFLIGHT,
    ENGINE_WINDOWS_SEMANTIC_PROTOCOL,
    ENGINE_MACOS_AGENT_CONTEXT,
    ENGINE_MACOS_SEMANTIC_RUNTIME,
    ENGINE_MACOS_SEMANTIC_ACTION,
    ENGINE_MACOS_SEMANTIC_SCREENSHOT,
    ENGINE_WINDOWS_AGENT_CONTEXT,
    ENGINE_WINDOWS_COOKIE_TRANSFER,
    ENGINE_WINDOWS_SEMANTIC_RUNTIME,
    ENGINE_WINDOWS_SEMANTIC_SCREENSHOT,
    ENGINE_WINDOWS_AGENT_TIMEOUT,
];
const ENGINE_AGENTIC_NATIVE_UNSAFE_HEADER: &str = concat!(
    "#![deny(unsafe_op_in_unsafe_fn)]\n",
    "#![deny(clippy::undocumented_unsafe_blocks)]\n",
);
const AGENTIC_NO_DIRECT_LOGGING_ATTRIBUTE: &str =
    "#![deny(clippy::dbg_macro,clippy::print_stderr,clippy::print_stdout)]";
const AGENTIC_NO_INVARIANT_PANIC_ATTRIBUTE: &str =
    "#![cfg_attr(not(test),deny(clippy::panic,clippy::unreachable,clippy::unwrap_used))]";
const ENGINE_MACOS_PROBE_MODULE: &str =
    "crates/zephium-engine/src/platform/macos/agentic_input_probe.rs";
const ENGINE_WINDOWS_PROBE_MODULE: &str =
    "crates/zephium-engine/src/platform/windows/agentic_input_probe.rs";
const ENGINE_WINDOWS_PROBE_RESOURCES: &str =
    "crates/zephium-engine/src/platform/windows/agentic_probe_resources.rs";
const ENGINE_WINDOWS_PAGE_MEMORY: &str =
    "crates/zephium-engine/src/platform/windows/page_memory.rs";
const ENGINE_WINDOWS_SEMANTIC_PROBE_MODULE: &str =
    "crates/zephium-engine/src/platform/windows/agentic_semantic_probe.rs";
const ENGINE_MACOS_PROBE_BINARY: &str =
    "crates/zephium-engine/src/bin/macos_agentic_input_probe.rs";
const ENGINE_MACOS_SEMANTIC_PROBE_BINARY: &str =
    "crates/zephium-engine/src/bin/macos_agentic_semantic_probe.rs";
const ENGINE_WINDOWS_PROBE_BINARY: &str =
    "crates/zephium-engine/src/bin/windows_agentic_input_probe.rs";
const ENGINE_WINDOWS_SEMANTIC_PROBE_BINARY: &str =
    "crates/zephium-engine/src/bin/windows_agentic_semantic_probe.rs";
const NIGHTLY_WORKFLOW: &str = ".github/workflows/nightly.yml";
const WINDOWS_AGENTIC_QUALIFICATION_SCRIPT: &str = "scripts/qualification/windows-agentic.ps1";
const AGENTIC_SOURCE_DIRECTORY: &str = "crates/zephium-agentic/src";
const AGENTIC_DIAGNOSTIC_MODULES: [&str; 11] = [
    "contract.rs",
    "control.rs",
    "evidence.rs",
    "fixture_server.rs",
    "probe_evidence_path.rs",
    "probe_recipes.rs",
    "probe_qualification.rs",
    "protocol.rs",
    "semantic_probe_evidence.rs",
    "windows_agentic_input_evidence_review.rs",
    "windows_agentic_semantic_evidence_review.rs",
];
const SHIPPING_ROOTS: [&str; 2] = ["desktop", "crates/zephium-app"];
const RELEASE_REFUSAL: &str = concat!(
    "#[cfg(all(feature=\"probe-harness\",not(debug_assertions)))]",
    "compile_error!(\"theagenticprobeharnessisforbiddeninoptimizedbuilds\");"
);
const ENGINE_RELEASE_REFUSAL: &str = concat!(
    "#[cfg(all(feature=\"native-agentic-input-probe\",not(debug_assertions)))]",
    "compile_error!(\"thenativeagenticinputprobeisforbiddeninoptimizedbuilds\");"
);
const ENGINE_SEMANTIC_PROBE_RELEASE_REFUSAL: &str = concat!(
    "#[cfg(all(feature=\"native-agentic-semantic-probe\",not(debug_assertions)))]",
    "compile_error!(\"thenativeagenticsemanticprobeisforbiddeninoptimizedbuilds\");"
);

pub(crate) fn check(repository: &Path) -> Result<(), String> {
    crate::foreground_rendering_boundary::check(repository)?;
    crate::agentic_evidence::check(repository)?;
    validate_manifest(&read(repository.join(AGENTIC_MANIFEST))?)?;
    validate_probe_evidence_path(&read(repository.join(AGENTIC_PROBE_EVIDENCE_PATH))?)?;
    validate_windows_review_binary(
        &read(repository.join(AGENTIC_WINDOWS_REVIEW_BINARY))?,
        &read(repository.join(AGENTIC_PROBE_QUALIFICATION))?,
    )?;
    validate_windows_semantic_review_binary(
        &read(repository.join(AGENTIC_WINDOWS_SEMANTIC_REVIEW_BINARY))?,
        &read(repository.join(AGENTIC_WINDOWS_SEMANTIC_EVIDENCE))?,
    )?;
    let agentic_root = read(repository.join(AGENTIC_ROOT))?;
    let fixed_runtime = compact(&read(
        repository.join("crates/zephium-agentic/src/semantic_runtime.rs"),
    )?);
    for required in [
        "MAX_SEMANTIC_RUNTIME_SOURCE_BYTES:usize=216*1024;",
        "MAX_SEMANTIC_RUNTIME_REQUEST_BYTES:usize=2*1024;",
        "MAX_SEMANTIC_ACTION_RUNTIME_RESULT_BYTES:usize=512;",
        "SEMANTIC_RUNTIME_SOURCE_SHA256:[u8;32]",
        "#[cfg(target_os=\"windows\")]\"engine_native_input\"=>crate::SemanticActionExecutionBackend::EngineNativeInput",
    ] {
        if !fixed_runtime.contains(required) {
            return Err(format!("fixed semantic runtime lost its reviewed budget/platform contract: {required}"));
        }
    }
    validate_root(&agentic_root)?;
    validate_agentic_no_direct_logging_attribute(AGENTIC_ROOT, &agentic_root)?;
    validate_agentic_no_invariant_panic_attribute(AGENTIC_ROOT, &agentic_root)?;
    validate_agent_metrics_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_METRICS))?,
    )?;
    validate_agent_action_metrics_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_ACTION_METRICS))?,
    )?;
    validate_agent_input_metrics_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_INPUT_METRICS))?,
        &read(repository.join(AGENTIC_PROVIDER_REQUEST))?,
        &read(repository.join(AGENTIC_POLICY))?,
        &read(repository.join(PROVIDER_TRANSPORT_ROOT))?,
    )?;
    validate_agent_progress_metrics_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_PROGRESS_METRICS))?,
        &read(repository.join(AGENTIC_AUDIT))?,
    )?;
    validate_agent_supervisor_cancellation_finalization(&read(
        repository.join(AGENTIC_SUPERVISOR_RUNTIME),
    )?)?;
    validate_agentic_default_core_panic_boundary(
        &read(repository.join(AGENTIC_SEMANTIC))?,
        &read(repository.join(AGENTIC_SEMANTIC_WIRE))?,
        &read(repository.join(AGENTIC_SEMANTIC_EXECUTE))?,
        &read(repository.join(AGENTIC_PROVIDER_CONTINUATION))?,
        &read(repository.join(AGENTIC_PROVIDER_REQUEST))?,
    )?;
    validate_agent_metric_closure_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_METRIC_CLOSURE))?,
        &read(repository.join(AGENTIC_METRICS))?,
        &read(repository.join(AGENTIC_PROGRESS_METRICS))?,
        &read(repository.join(AGENTIC_ACTION_METRICS))?,
        &read(repository.join(AGENTIC_INPUT_METRICS))?,
    )?;
    validate_agent_policy_settlement_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_METRIC_CLOSURE))?,
        &read(repository.join(AGENTIC_METRICS))?,
        &read(repository.join(AGENTIC_AUDIT))?,
        &read(repository.join(AGENTIC_POLICY))?,
    )?;
    validate_provider_billing_contract(
        &read(repository.join(AGENTIC_PROVIDER_ROOT))?,
        &read(repository.join(AGENTIC_PROVIDER_REQUEST))?,
        &read(repository.join(AGENTIC_PROVIDER_OPENAI))?,
        &read(repository.join(AGENTIC_PROVIDER_ANTHROPIC))?,
    )?;
    validate_provider_pricing_contract(
        &read(repository.join(AGENTIC_PROVIDER_PRICING))?,
        &read(repository.join(AGENTIC_POLICY))?,
        &read(repository.join(AGENTIC_PROVIDER_SETTLEMENT))?,
    )?;
    validate_semantic_diff_policy_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_SEMANTIC_DIFF))?,
        &read(repository.join(AGENTIC_SEMANTIC_DIFF_MODEL))?,
        &read(repository.join(AGENTIC_POLICY))?,
    )?;
    validate_semantic_locate_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_SEMANTIC_LOCATE))?,
        &read(repository.join(AGENTIC_SEMANTIC_LOCATE_MODEL))?,
        &read(repository.join(AGENTIC_PROVIDER_ROOT))?,
        &read(repository.join(AGENTIC_PROVIDER_TOOL))?,
        &read(repository.join(AGENTIC_PROVIDER_CONTINUATION))?,
        &read(repository.join(AGENTIC_PROVIDER_REQUEST))?,
        &read(repository.join(AGENTIC_POLICY))?,
    )?;
    validate_semantic_read_continuation_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_SEMANTIC_READ))?,
        &read(repository.join(AGENTIC_SEMANTIC_READ_MODEL))?,
        &read(repository.join(AGENTIC_PROVIDER_ROOT))?,
        &read(repository.join(AGENTIC_PROVIDER_CONTINUATION))?,
        &read(repository.join(AGENTIC_PROVIDER_REQUEST))?,
        &read(repository.join(AGENTIC_POLICY))?,
    )?;
    validate_semantic_extraction_provider_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_SEMANTIC_EXTRACT))?,
        &read(repository.join(AGENTIC_SEMANTIC_EXTRACT_MODEL))?,
        &read(repository.join(AGENTIC_PROVIDER_ROOT))?,
        &read(repository.join(AGENTIC_PROVIDER_TOOL))?,
        &read(repository.join(AGENTIC_PROVIDER_CONTINUATION))?,
        &read(repository.join(AGENTIC_PROVIDER_REQUEST))?,
        &read(repository.join(AGENTIC_PROVIDER_EXTRACTION))?,
        &read(repository.join(AGENTIC_POLICY))?,
    )?;
    validate_semantic_execution_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_EFFECT_POLICY))?,
        &read(repository.join(AGENTIC_SEMANTIC_ACTION))?,
        &read(repository.join(AGENTIC_SEMANTIC_EXECUTE))?,
        &read(repository.join(AGENTIC_SEMANTIC_EXECUTE_COORDINATOR))?,
        &read(repository.join(AGENTIC_CONTEXT_PORT))?,
        (
            &read(repository.join(ENGINE_AGENT_CONTEXT_PORT))?,
            &read(repository.join(ENGINE_MACOS_SEMANTIC_ACTION))?,
        ),
    )?;
    validate_snapshot_action_kinds(&read(repository.join(AGENTIC_PROVIDER_ROOT))?)?;
    validate_semantic_settle_wake(&read(repository.join(AGENTIC_SEMANTIC_SETTLE))?)?;
    validate_semantic_settlement_coordinator(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_SEMANTIC_SETTLE_COORDINATOR))?,
    )?;
    validate_semantic_terminal_verification(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_SEMANTIC_VERIFY))?,
        &read(repository.join(AGENTIC_EFFECT_POLICY))?,
    )?;
    validate_accounted_action_result(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_EFFECT_POLICY))?,
        &read(repository.join(AGENTIC_SEMANTIC_ACTION_RESULT))?,
        &read(repository.join(AGENTIC_SEMANTIC_ACTION_BATCH_RESULT))?,
    )?;
    validate_provider_input_evidence_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_PROVIDER_REQUEST))?,
        &read(repository.join(AGENTIC_PROVIDER_ROOT))?,
        &read(repository.join(AGENTIC_POLICY))?,
        &read(repository.join(AGENTIC_SEMANTIC_DIFF_MODEL))?,
        &read(repository.join(PROVIDER_TRANSPORT_ROOT))?,
    )?;
    validate_progress_manifest_revision_contract(
        &read(repository.join(AGENTIC_POLICY))?,
        &read(repository.join(AGENTIC_EFFECT_POLICY))?,
        &read(repository.join(AGENTIC_SUPERVISOR))?,
        &read(repository.join(AGENTIC_SUPERVISOR_PROGRESS))?,
        &read(repository.join(AGENTIC_AUDIT))?,
    )?;
    validate_provider_call_identity_progress_boundary(
        &read(repository.join(AGENTIC_POLICY))?,
        &read(repository.join(AGENTIC_PROVIDER_ROOT))?,
        &read(repository.join(AGENTIC_PROVIDER_CONTINUATION))?,
        &read(repository.join(AGENTIC_SUPERVISOR_PROGRESS))?,
    )?;
    validate_profile_lease_release_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_PROFILE_LEASE))?,
        &read(repository.join(AGENTIC_SUPERVISOR_CONTEXT_SCHEDULE))?,
    )?;
    validate_context_shutdown_retention_contract(&read(
        repository.join(AGENTIC_CONTEXT_REGISTRY),
    )?)?;
    validate_cookie_transfer_deadline_contract(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_COOKIE_TRANSFER))?,
    )?;
    validate_agent_native_shutdown_coordinator(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_LIFECYCLE))?,
        &read(repository.join(AGENTIC_NATIVE_SHUTDOWN))?,
        &read(repository.join(AGENTIC_SEMANTIC_SCREENSHOT))?,
    )?;
    validate_agent_native_shutdown_driver(
        &read(repository.join(AGENTIC_ROOT))?,
        &read(repository.join(AGENTIC_NATIVE_SHUTDOWN_DRIVER))?,
    )?;
    validate_agent_app_lifecycle(
        &read(repository.join(APP_MANIFEST))?,
        &read(repository.join(APP_ROOT))?,
        &read(repository.join(APP_API))?,
        &read(repository.join(APP_ACTOR))?,
        &read(repository.join(APP_SHELL))?,
        &read(repository.join(DESKTOP_MANIFEST))?,
    )?;
    validate_store_agent_audit_boundary(
        &read(repository.join(STORE_ACTOR))?,
        &read(repository.join(STORE_AGENT_AUDIT_ACTOR))?,
        &read(repository.join(STORE_AGENT_AUDIT_HUB))?,
    )?;
    validate_provider_transport_manifest(&read(repository.join(PROVIDER_TRANSPORT_MANIFEST))?)?;
    validate_provider_transport_facade(
        &read(repository.join(PROVIDER_TRANSPORT_FACADE_MANIFEST))?,
        &read(repository.join(PROVIDER_TRANSPORT_FACADE_ROOT))?,
    )?;
    let provider_transport_root = read(repository.join(PROVIDER_TRANSPORT_ROOT))?;
    validate_provider_transport_root(&provider_transport_root)?;
    validate_provider_transport_commit_boundary(&provider_transport_root)?;
    validate_abortable_provider_operation_contract(&provider_transport_root)?;
    validate_immediate_provider_settlement_recovery_contract(&provider_transport_root)?;
    validate_provider_consumer_panic_boundary(
        &provider_transport_root,
        &read(repository.join(AGENTIC_PROVIDER_ROOT))?,
    )?;
    validate_provider_transport_response_header_boundary(&provider_transport_root)?;
    validate_provider_transport_http2_ingress_boundary(&provider_transport_root)?;
    validate_provider_transport_shutdown_contract(
        &provider_transport_root,
        &read(repository.join("crates/zephium-agentic/src/provider_transport/lead/keys.rs"))?,
    )?;
    validate_agentic_no_direct_logging_attribute(
        PROVIDER_TRANSPORT_ROOT,
        &provider_transport_root,
    )?;
    validate_agentic_no_invariant_panic_attribute(
        PROVIDER_TRANSPORT_ROOT,
        &provider_transport_root,
    )?;
    validate_agentic_no_direct_logging_calls(PROVIDER_TRANSPORT_ROOT, &provider_transport_root)?;
    validate_provider_secret_diagnostic_contract(&provider_transport_root)?;
    validate_provider_revocation_before_secret_materialization(&provider_transport_root)?;
    validate_chosen_model_drive_contract(&read(
        repository.join("crates/zephium-agentic/src/provider_transport/model_client.rs"),
    )?)?;
    validate_engine_manifest(&read(repository.join(ENGINE_MANIFEST))?)?;
    validate_engine_process_memory_users(repository)?;
    validate_engine_page_memory(&read(repository.join(ENGINE_WINDOWS_PAGE_MEMORY))?)?;
    validate_engine_root(&read(repository.join(ENGINE_ROOT))?)?;
    for path in ENGINE_AGENTIC_PRODUCTION_MODULES {
        let source = read(repository.join(path))?;
        validate_agentic_no_direct_logging_attribute(path, &source)?;
        validate_agentic_no_invariant_panic_attribute(path, &source)?;
        validate_agentic_no_direct_logging_calls(path, &source)?;
    }
    for path in ENGINE_AGENTIC_NATIVE_UNSAFE_MODULES {
        validate_engine_agentic_native_unsafe_contract(path, &read(repository.join(path))?)?;
    }
    validate_engine_macos_agent_main_thread_contract(&read(
        repository.join(ENGINE_MACOS_AGENT_CONTEXT),
    )?)?;
    validate_engine_agent_context_boundary(
        &read(repository.join(ENGINE_ROOT))?,
        &read(repository.join(ENGINE_HOST_ROOT))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_PORT))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_HOST))?,
        &read(repository.join(ENGINE_MACOS_AGENT_CONTEXT))?,
        &read(repository.join(ENGINE_AGENT_NAVIGATION))?,
    )?;
    validate_engine_agent_context_panic_boundary(&read(
        repository.join(ENGINE_AGENT_CONTEXT_PORT),
    )?)?;
    validate_engine_agent_lifetime_boundary(&read(repository.join(ENGINE_AGENT_CONTEXT_PORT))?)?;
    validate_engine_agent_redirect_contract(
        &read(repository.join(AGENTIC_CONTEXT_PORT))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_HOST))?,
        &read(repository.join(ENGINE_AGENT_NAVIGATION))?,
        &read(repository.join(AGENTIC_FIXTURE_SERVER))?,
    )?;
    validate_engine_agent_cookie_preflight(&read(repository.join(ENGINE_AGENT_COOKIE_PREFLIGHT))?)?;
    validate_agent_context_shutdown_barrier_contract(
        &read(repository.join(AGENTIC_CONTEXT_PORT))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_PORT))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_HOST))?,
    )?;
    validate_engine_semantic_runtime_boundary(&read(
        repository.join(ENGINE_MACOS_SEMANTIC_RUNTIME),
    )?)?;
    validate_engine_semantic_screenshot_boundary(&read(
        repository.join(ENGINE_MACOS_SEMANTIC_SCREENSHOT),
    )?)?;
    let macos_module = read(repository.join(ENGINE_MACOS_MODULE))?;
    validate_engine_platform_module(&macos_module, "macOS")?;
    validate_macos_semantic_probe(
        &macos_module,
        &read(repository.join(ENGINE_MACOS_SEMANTIC_PROBE))?,
        &read(repository.join(ENGINE_MACOS_SEMANTIC_PROBE_BINARY))?,
        &read(repository.join(AGENTIC_FIXTURE_SERVER))?,
    )?;
    validate_macos_rendering_probe(
        &read(repository.join(ENGINE_MACOS_RENDERING_PROBE))?,
        &read(repository.join(ENGINE_MACOS_SEMANTIC_PROBE))?,
        &read(repository.join(ENGINE_MACOS_SEMANTIC_PROBE_BINARY))?,
        &read(repository.join(AGENTIC_FIXTURE_SERVER))?,
    )?;
    validate_macos_rendering_opportunity_probe(
        &read(repository.join(ENGINE_MACOS_RENDERING_OPPORTUNITY_PROBE))?,
        &read(repository.join(ENGINE_MACOS_SEMANTIC_PROBE))?,
        &read(repository.join(ENGINE_MACOS_SEMANTIC_PROBE_BINARY))?,
    )?;
    validate_macos_rendering_presented_probe(
        &read(repository.join(ENGINE_MACOS_RENDERING_PRESENTED_PROBE))?,
        &read(repository.join(ENGINE_MACOS_SEMANTIC_PROBE))?,
        &read(repository.join(ENGINE_MACOS_SEMANTIC_PROBE_BINARY))?,
    )?;
    let windows_module = read(repository.join(ENGINE_WINDOWS_MODULE))?;
    validate_engine_platform_module(&windows_module, "Windows")?;
    validate_windows_probe_resource_sampler(
        &windows_module,
        &read(repository.join(ENGINE_WINDOWS_PROBE_RESOURCES))?,
    )?;
    validate_engine_windows_agent_context_boundary(
        &windows_module,
        &read(repository.join(ENGINE_WINDOWS_AGENT_CONTEXT))?,
        &read(repository.join(ENGINE_WINDOWS_AGENT_TIMEOUT))?,
        &read(repository.join(ENGINE_AGENT_NAVIGATION))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_HOST))?,
    )?;
    validate_engine_agent_location_observation(
        &read(repository.join(ENGINE_AGENT_CONTEXT_PORT))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_HOST))?,
        &read(repository.join(ENGINE_MACOS_AGENT_CONTEXT))?,
        &read(repository.join(ENGINE_WINDOWS_AGENT_CONTEXT))?,
        &read(repository.join(ENGINE_AGENT_NAVIGATION))?,
    )?;
    validate_engine_windows_agent_cookie_source(
        &read(repository.join(ENGINE_HOST_ROOT))?,
        &read(repository.join(ENGINE_AGENT_COOKIE_SOURCE))?,
    )?;
    validate_engine_windows_agent_suspension_boundary(
        &read(repository.join(ENGINE_PLATFORM_MODULE))?,
        &read(repository.join(ENGINE_WINDOWS_AGENT_CONTEXT))?,
        &read(repository.join(ENGINE_AGENT_SUSPENSION))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_HOST))?,
        &read(repository.join(ENGINE_HOST_CONTENT_RULES))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_PORT))?,
    )?;
    validate_engine_windows_cookie_transfer(
        &windows_module,
        &read(repository.join(ENGINE_WINDOWS_COOKIE_TRANSFER))?,
    )?;
    validate_engine_windows_semantic_protocol(
        &read(repository.join(ENGINE_PLATFORM_MODULE))?,
        &read(repository.join(ENGINE_WINDOWS_SEMANTIC_PROTOCOL))?,
    )?;
    validate_engine_windows_semantic_runtime(
        &windows_module,
        &read(repository.join(ENGINE_WINDOWS_SEMANTIC_RUNTIME))?,
    )?;
    validate_engine_windows_semantic_screenshot(
        &windows_module,
        &read(repository.join(ENGINE_WINDOWS_AGENT_CONTEXT))?,
        &read(repository.join(ENGINE_AGENT_SCREENSHOT_BUFFER))?,
        &read(repository.join(ENGINE_WINDOWS_SEMANTIC_SCREENSHOT))?,
    )?;
    validate_windows_semantic_probe(
        &windows_module,
        &read(repository.join(ENGINE_WINDOWS_SEMANTIC_PROBE_MODULE))?,
        &read(repository.join(ENGINE_WINDOWS_SEMANTIC_PROBE_BINARY))?,
        &read(repository.join(AGENTIC_FIXTURE_SERVER))?,
    )?;
    validate_owned_context_viewport_contract(
        &read(repository.join(AGENTIC_CONTEXT_PORT))?,
        &read(repository.join(ENGINE_AGENT_CONTEXT_HOST))?,
        &read(repository.join(ENGINE_MACOS_AGENT_CONTEXT))?,
        &read(repository.join(ENGINE_WINDOWS_AGENT_CONTEXT))?,
    )?;
    let _ = read(repository.join(ENGINE_MACOS_PROBE_BINARY))?;
    validate_macos_probe_source(&read(repository.join(ENGINE_MACOS_PROBE_MODULE))?)?;
    validate_windows_probe_binary(
        &read(repository.join(ENGINE_WINDOWS_PROBE_BINARY))?,
        &read(repository.join(AGENTIC_PROBE_QUALIFICATION))?,
    )?;
    validate_windows_probe_ci(&read(repository.join(NIGHTLY_WORKFLOW))?)?;
    validate_windows_physical_workflow(&read(
        repository.join(WINDOWS_AGENTIC_QUALIFICATION_SCRIPT),
    )?)?;
    validate_windows_probe_source(&read(repository.join(ENGINE_WINDOWS_PROBE_MODULE))?)?;
    validate_agentic_zero_idle_sources(repository)?;
    validate_shipping_sources(repository)?;
    let metadata = cargo_metadata(repository)?;
    validate_release_graph(&metadata)
}

fn validate_agentic_no_direct_logging_attribute(label: &str, source: &str) -> Result<(), String> {
    if !compact(source).contains(AGENTIC_NO_DIRECT_LOGGING_ATTRIBUTE) {
        return Err(format!(
            "production agentic module {label} must deny stdout, stderr, and dbg macros"
        ));
    }
    Ok(())
}

fn validate_agentic_no_invariant_panic_attribute(label: &str, source: &str) -> Result<(), String> {
    if !compact(source).contains(AGENTIC_NO_INVARIANT_PANIC_ATTRIBUTE) {
        return Err(format!(
            "production agentic module {label} must deny unwrap, panic, and unreachable macros"
        ));
    }
    Ok(())
}

fn validate_agentic_no_direct_logging_calls(label: &str, source: &str) -> Result<(), String> {
    let source = compact(source);
    for forbidden in [
        "print!(",
        "println!(",
        "eprint!(",
        "eprintln!(",
        "dbg!(",
        "tracing::",
        "log::",
        "slog::",
        "diagnostic!(",
        "std::io::stdout(",
        "std::io::stderr(",
        "NSLog(",
        "os_log",
        "OutputDebugString",
        "EventWrite",
        "fprintf(",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "production agentic module {label} acquired forbidden direct diagnostic output {forbidden}"
            ));
        }
    }

    Ok(())
}

fn validate_engine_agentic_native_unsafe_contract(path: &str, source: &str) -> Result<(), String> {
    if !source.starts_with(ENGINE_AGENTIC_NATIVE_UNSAFE_HEADER) {
        return Err(format!(
            "production agentic native module {path} must deny undocumented unsafe blocks and unsafe operations outside explicit blocks"
        ));
    }
    Ok(())
}

fn validate_engine_macos_agent_main_thread_contract(source: &str) -> Result<(), String> {
    let source = compact(source);
    if !source.contains(
        "fnharden_owned_agent_view(view:&WebView)->Result<(),AgentOwnedViewConstructionError>",
    ) || source
        .matches("let_mtm=MainThreadMarker::new().ok_or(AgentOwnedViewConstructionError::Native)?;")
        .count()
        != 2
    {
        return Err(
            "production macOS agent view must refuse off-main-thread hardening and attestation"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_engine_process_memory_users(repository: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_files(&repository.join("crates/zephium-engine/src"), &mut files)?;
    for file in files {
        let relative = file
            .strip_prefix(repository)
            .map_err(|_| format!("{} is outside the repository", file.display()))?;
        let allowed = [ENGINE_WINDOWS_PROBE_RESOURCES, ENGINE_WINDOWS_PAGE_MEMORY]
            .iter()
            .any(|path| relative == Path::new(path));
        if !allowed && read(&file)?.contains("ProcessStatus") {
            return Err(format!(
                "{} acquired process-memory authority outside the probe and page memory",
                relative.display()
            ));
        }
    }
    Ok(())
}

fn validate_engine_page_memory(source: &str) -> Result<(), String> {
    let code = compact(source);
    if code.matches("OpenProcess(").count() != 1
        || !code.contains("OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid)")
        || code.matches("GetProcessMemoryInfo(").count() != 1
        || code.matches("CloseHandle(process)").count() != 1
    {
        return Err(
            "page memory must open each renderer once with limited query rights and close it"
                .to_owned(),
        );
    }
    for forbidden in [
        "PROCESS_VM_READ",
        "PROCESS_VM_WRITE",
        "PROCESS_VM_OPERATION",
        "PROCESS_QUERY_INFORMATION",
        "PROCESS_ALL_ACCESS",
        "ReadProcessMemory",
        "DuplicateHandle",
    ] {
        if code.contains(forbidden) {
            return Err(format!(
                "page memory acquired process authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_manifest(source: &str) -> Result<(), String> {
    // Once for the probe's sampler, once for the shell's page-memory query.
    if source.matches("\"Win32_System_ProcessStatus\"").count() != 2 {
        return Err(
            "engine process-memory binding must be declared for the probe and page memory only"
                .to_owned(),
        );
    }
    let manifest: toml::Value = toml::from_str(source)
        .map_err(|error| format!("cannot parse {ENGINE_MANIFEST}: {error}"))?;
    let feature = manifest
        .get("features")
        .and_then(|features| features.get("native-agentic-input-probe"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "engine native-agentic-input-probe feature is missing".to_owned())?;
    let actual = feature
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let expected = [
        "dep:serde",
        "dep:tempfile",
        "dep:windows-probe-sys",
        "dep:zephium-agentic",
        "zephium-agentic/probe-harness",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if actual != expected || actual.len() != feature.len() {
        return Err("engine native-agentic-input-probe feature graph drifted".to_owned());
    }
    let windows_dependencies = manifest
        .get("target")
        .and_then(|targets| targets.get("cfg(target_os = \"windows\")"))
        .and_then(|target| target.get("dependencies"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "engine Windows target dependency table is missing".to_owned())?;
    let process_bindings = windows_dependencies
        .get("windows-probe-sys")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "engine probe-only process bindings are missing".to_owned())?;
    let process_binding_features = process_bindings
        .get("features")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "engine probe-only process binding features are missing".to_owned())?
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let expected_process_binding_features = [
        "Win32_Foundation",
        "Win32_System_ProcessStatus",
        "Win32_System_Threading",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if process_bindings
        .get("package")
        .and_then(toml::Value::as_str)
        != Some("windows-sys")
        || process_bindings
            .get("version")
            .and_then(toml::Value::as_str)
            != Some("=0.61.2")
        || process_bindings
            .get("optional")
            .and_then(toml::Value::as_bool)
            != Some(true)
        || process_binding_features != expected_process_binding_features
    {
        return Err("engine process-memory bindings must remain exact and probe-only".to_owned());
    }
    let production_feature = manifest
        .get("features")
        .and_then(|features| features.get("agentic-browser"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "engine agentic-browser feature is missing".to_owned())?;
    if production_feature.as_slice()
        != [
            toml::Value::String("dep:zephium-agentic".to_owned()),
            toml::Value::String("dep:image".to_owned()),
        ]
    {
        return Err(
            "engine production agentic-browser feature must remain probe-independent".to_owned(),
        );
    }
    // 801fcc49 moved the codec to both platforms for bounded record photos
    // (platform/record_media.rs caps bytes, 2048 px and 24 MiB allocation).
    if windows_dependencies.contains_key("image") {
        return Err("Work thumbnail codec must be declared once for every platform".to_owned());
    }
    let image = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("image"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "Work thumbnail codec is missing".to_owned())?;
    if image.get("version").and_then(toml::Value::as_str) != Some("=0.25.10")
        || image.get("optional").and_then(toml::Value::as_bool) != Some(true)
        || image.get("default-features").and_then(toml::Value::as_bool) != Some(false)
        || image.get("features").and_then(toml::Value::as_array)
            != Some(
                &["png", "jpeg", "webp", "gif"]
                    .map(|feature| toml::Value::String(feature.to_owned()))
                    .to_vec(),
            )
    {
        return Err(
            "Work thumbnail codec must remain optional and limited to PNG, JPEG, WebP and GIF"
                .to_owned(),
        );
    }
    let semantic_probe_feature = manifest
        .get("features")
        .and_then(|features| features.get("native-agentic-semantic-probe"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "engine native-agentic-semantic-probe feature is missing".to_owned())?;
    let semantic_actual = semantic_probe_feature
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let semantic_expected = [
        "agentic-browser",
        "dep:tempfile",
        "dep:windows-probe-sys",
        "zephium-agentic/probe-harness",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if semantic_actual != semantic_expected || semantic_actual.len() != semantic_probe_feature.len()
    {
        return Err("engine native-agentic-semantic-probe feature graph drifted".to_owned());
    }
    let agentic_dependency = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("zephium-agentic"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "engine zephium-agentic dependency is missing".to_owned())?;
    if agentic_dependency
        .get("optional")
        .and_then(toml::Value::as_bool)
        != Some(true)
    {
        return Err("engine zephium-agentic dependency must remain optional".to_owned());
    }
    // Ordinary pages wipe HTTP sign-in secrets too, so the wiping crate is
    // always compiled; it stays exactly pinned.
    if manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("zeroize"))
        .and_then(toml::Value::as_str)
        != Some("=1.9.0")
    {
        return Err(
            "engine secret-wiping zeroize dependency must remain exactly pinned".to_owned(),
        );
    }
    let binaries = manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "engine agentic probe binaries are missing".to_owned())?;
    for (name, path, platform) in [
        (
            "macos-agentic-input-probe",
            "src/bin/macos_agentic_input_probe.rs",
            "macOS",
        ),
        (
            "windows-agentic-input-probe",
            "src/bin/windows_agentic_input_probe.rs",
            "Windows",
        ),
    ] {
        let binary = binaries
            .iter()
            .find(|binary| binary.get("name").and_then(toml::Value::as_str) == Some(name))
            .ok_or_else(|| format!("engine {platform} agentic probe binary is missing"))?;
        if binary.get("path").and_then(toml::Value::as_str) != Some(path)
            || binary
                .get("required-features")
                .and_then(toml::Value::as_array)
                .is_none_or(|features| {
                    features.as_slice()
                        != [toml::Value::String("native-agentic-input-probe".into())]
                })
        {
            return Err(format!(
                "engine {platform} agentic probe binary gate drifted"
            ));
        }
    }
    for (name, path, platform) in [
        (
            "macos-agentic-semantic-probe",
            "src/bin/macos_agentic_semantic_probe.rs",
            "macOS",
        ),
        (
            "windows-agentic-semantic-probe",
            "src/bin/windows_agentic_semantic_probe.rs",
            "Windows",
        ),
        (
            "windows-cookie-persistence-control",
            "src/bin/windows_cookie_persistence_control.rs",
            "Windows cookie persistence",
        ),
    ] {
        let semantic_binary = binaries
            .iter()
            .find(|binary| binary.get("name").and_then(toml::Value::as_str) == Some(name))
            .ok_or_else(|| format!("engine {platform} semantic probe binary is missing"))?;
        if semantic_binary.get("path").and_then(toml::Value::as_str) != Some(path)
            || semantic_binary
                .get("required-features")
                .and_then(toml::Value::as_array)
                .is_none_or(|features| {
                    features.as_slice()
                        != [toml::Value::String("native-agentic-semantic-probe".into())]
                })
        {
            return Err(format!(
                "engine {platform} semantic probe binary gate drifted"
            ));
        }
    }
    Ok(())
}

fn validate_engine_root(source: &str) -> Result<(), String> {
    let source = compact(source);
    if !source.contains(ENGINE_RELEASE_REFUSAL) {
        return Err("engine must retain its optimized agentic-probe compile refusal".to_owned());
    }
    if !source.contains(ENGINE_SEMANTIC_PROBE_RELEASE_REFUSAL) {
        return Err("engine must retain its optimized semantic-probe compile refusal".to_owned());
    }
    for required in [
        "#[cfg(all(target_os=\"macos\",feature=\"native-agentic-semantic-probe\"))]",
        "pubfnrun_macos_agentic_semantic_probe()->Result<(),&'staticstr>",
        "platform::macos::run_agentic_semantic_probe()",
        "pubfnrun_macos_agentic_semantic_model_public_fill_probe(",
        "platform::macos::run_agentic_semantic_model_public_fill_probe(prepare)",
        "pubfnrun_macos_agentic_semantic_model_two_action_probe(",
        "platform::macos::run_agentic_semantic_model_two_action_probe(",
        "#[cfg(all(target_os=\"windows\",feature=\"native-agentic-semantic-probe\"))]",
        "pubfnrun_windows_agentic_semantic_probe(",
        "mode:zephium_agentic::WindowsSemanticProbeMode",
        "Result<zephium_agentic::WindowsSemanticProbeEvidence,zephium_agentic::WindowsSemanticProbeFailure,>",
        "platform::windows::run_agentic_semantic_probe(request_id,mode)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "engine semantic-probe public boundary lost required gate {required}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_agent_lifetime_boundary(source: &str) -> Result<(), String> {
    let source = compact(source);
    for boundary in [
        "pubstructAgentBrowserLifetimeFactory",
        "pubconstMAX_AGENT_BROWSER_LIFETIMES:u16=1024;",
        "ifstate.taken||state.sealed",
        "state.issued>=MAX_AGENT_BROWSER_LIFETIMES",
        "active.retire_for_successor()",
        "!state.native_shutdown_verified",
        "state.pending!=0",
        "state.physical_screenshots!=0",
        "state.retired=true;",
        "state.retired||self.lineage_failed()",
        "self.permit.admission.verify_native_shutdown(snapshot)",
        "self.permit.admission.invalidate_retirement_audit()",
        "Self::native_resources_are_empty(snapshot)",
        "settlement.outcome().is_ok_and(AgentPortAdmission::native_resources_are_empty)",
        "ContextNativeEvent::ShutdownAuditSettled(settlement)",
        "state.pending==1",
        "known_bindings:0,resident_views:0,owned_reservations:0,borrowed_leases:0,visible_surfaces:0,suspended_views:0,pending_operations:0,pending_captures:0,queued_tasks:0,",
        "factory.seal();",
        "failed.store(true,Ordering::Release)",
    ] {
        if !source.contains(boundary) {
            return Err(format!(
                "native lifetime factory lost exact ownership boundary: {boundary}"
            ));
        }
    }
    for forbidden in [
        "state.sealed=false",
        "state.retired=false",
        "implCloneforAgentBrowserLifetimeFactory",
        "#[derive(Clone)]pubstructAgentBrowserLifetimeFactory",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "native lifetime factory reopens authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_agent_context_boundary(
    engine_root: &str,
    host_root: &str,
    port: &str,
    host: &str,
    macos: &str,
    navigation: &str,
) -> Result<(), String> {
    let engine_root = compact(engine_root);
    for required in [
        "#[cfg(feature=\"agentic-browser\")]modagent_context_port;",
        "pubfntake_agent_browser_port(",
        "self.agent_context_port.seal();",
    ] {
        if !engine_root.contains(required) {
            return Err(format!(
                "production agent-context engine boundary lost required gate {required}"
            ));
        }
    }
    let host_root = compact(host_root);
    for required in [
        "#[cfg(feature=\"agentic-browser\")]modagent_context;",
        "#[cfg(all(feature=\"agentic-browser\",target_os=\"windows\"))]modagent_cookie_source;",
        "agent_contexts:HashMap<zephium_agentic::ContextId,agent_context::AgentOwnedContext>",
        "agent_cookie_transfers:HashMap<zephium_agentic::ContextCookieTransferId,agent_context::AgentPendingCookieTransfer",
        "agent_cookie_quarantined_profiles:HashSet<ProfileId>",
    ] {
        if !host_root.contains(required) {
            return Err(format!(
                "production agent-context identity island lost required gate {required}"
            ));
        }
    }
    let port = compact(port);
    for required in [
        "MAX_PENDING_NATIVE_CONTEXT_TASKS",
        "MAX_PENDING_SEMANTIC_SCREENSHOTS",
        "structAgentScreenshotPhysicalPermit",
        "physical_screenshots:usize",
        "fnreserve_screenshot",
        "fncapture_semantic_screenshot",
        "ContextOperationKind::Recover",
        "ContextOperationKind::Close",
        "constfnsupports_cookie_transfer()->bool{#[cfg(target_os=\"windows\")]{true}#[cfg(not(target_os=\"windows\"))]{false}}",
        "admitted_at:std::time::Instant",
        "pub(crate)fncookie(&self)->Option<(&ContextCookieTransferRequest,std::time::Instant)>",
        "Some(AgentPendingRequest::Cookie(request))=>Some((request,self.admitted_at))",
        "pub(crate)structAgentContextPortSlot",
        "catch_unwind",
        "fnemit_renderer_lost",
        "ContextDispatch::Unsupported",
    ] {
        if !port.contains(required) {
            return Err(format!(
                "production agent-context port lost required closed mechanism {required}"
            ));
        }
    }

    let host = compact(host);
    for required in [
        "profile_lease:ContextProfileLease",
        "install_content_policy_on_view(view.view(),&content_policy)",
        "NativeResourceClass::AgentContext",
        "ContextConstructionProof::MacOsOwnedSelectedProfileExtensionFree",
        "pending_navigation:Option<AgentPendingNavigation>",
        "pending_recovery:Option<AgentPendingRecovery>",
        "pending_screenshot:Option<AgentPendingScreenshot>",
        "pending_captures,",
        "renderer_loss_rejoin_pending:bool",
        "binding.view.attest(",
        "binding.view.prepare_semantic_document_load()",
        "fnstart_owned_agent_semantic_invocation",
        "fnstart_owned_agent_screenshot",
        "binding.semantic_snapshot_generation!=Some(snapshot_generation)",
        "binding.view.dispatch_screenshot(",
        "SemanticRuntimeSettlement::try_new",
        "double_full_successor(binding.join,requested)",
        "binding.view.view().reload()",
        "binding.view.view().load_url(\"about:blank\")",
        "AGENT_PAGE_LOAD_COMMIT_TIMEOUT",
        "try_with_agent_context_terminal",
        "emitter.emit_renderer_lost(prior)",
        "force_shutdown_agent_contexts",
    ] {
        if !host.contains(required) {
            return Err(format!(
                "production agent-context owner lost required obligation {required}"
            ));
        }
    }

    let macos = compact(macos);
    for required in [
        "with_visible(false)",
        "with_focused(false)",
        "configuration.webExtensionController()",
        "semantic.attest_configuration(&configuration)",
        "WKWebsiteDataStore::dataStoreForIdentifier(&identifier,mtm)",
        "Retained::as_ptr(&actual_store)==Retained::as_ptr(expected)",
        "semantic:Option<AgentSemanticRuntimeRegistration>",
        "pub(crate)fndispatch_screenshot(",
        "usecrate::platform::agent_navigation::AgentNavigationController",
        "with_on_web_content_process_terminate_handler",
        "renderer_lost_callback",
    ] {
        if !macos.contains(required) {
            return Err(format!(
                "production macOS agent-context attestation lost required check {required}"
            ));
        }
    }
    let navigation = compact(navigation);
    for required in [
        "structAgentNavigationController",
        "state.bootstrap_available=false",
        "native_id:Option<wry::NavigationId>",
        "armed.native_id!=Some(event.id)",
        "terminal_claimed.compare_exchange",
        "fndocument_finished_for_audit",
        "fnclaim_renderer_loss",
        "fnarm_recovery",
        "fnsettle_recovery",
    ] {
        if !navigation.contains(required) {
            return Err(format!(
                "shared production agent-context navigation lost required check {required}"
            ));
        }
    }
    for (label, source) in [
        ("port", port.as_str()),
        ("host", host.as_str()),
        ("macOS adapter", macos.as_str()),
        ("shared navigation", navigation.as_str()),
    ] {
        for forbidden in [
            "with_ipc_handler",
            "with_initialization_script",
            "evaluate_script",
            "querySelector",
            "CallDevToolsProtocolMethod",
            "native-agentic-input-probe",
        ] {
            if source.contains(forbidden) {
                return Err(format!(
                    "production agent-context {label} acquired forbidden surface {forbidden}"
                ));
            }
        }
    }
    Ok(())
}

fn validate_engine_agent_context_panic_boundary(source: &str) -> Result<(), String> {
    let source = compact(
        &source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "let_=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{(self.fatal)(\"agent-contextportslotinvariantfailed\");}));",
        "fncontain_agent_port_panic<T>(admission:&AgentPortAdmission,operation:implFnOnce()->T,)->Option<T>",
        "matchstd::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))",
        "Err(_)=>{admission.fail_invariant();None}",
        "letmutstate=poisoned.into_inner();state.sealed=true;ifletSome(admission)=&state.admission{admission.seal();}ifletSome(factory)=&state.factory{factory.seal();}drop(state);self.report_fatal_once();",
        "letaccepted=contain_agent_port_panic(&self.admission,||{dispatch(Box::new(move||{",
        "let_=contain_agent_port_panic(&callback_admission,||{",
        ".unwrap_or(false);",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "production agent-context dispatch lost panic containment {required}"
            ));
        }
    }
    if source
        .matches("letaccepted=contain_agent_port_panic")
        .count()
        != 3
        || source
            .matches("let_=contain_agent_port_panic(&callback_admission")
            .count()
            != 3
        || source
            .matches("matchstd::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))")
            .count()
            != 1
        || source
            .matches("letmutstate=poisoned.into_inner();state.sealed=true;ifletSome(admission)=&state.admission{admission.seal();}ifletSome(factory)=&state.factory{factory.seal();}drop(state);self.report_fatal_once();")
            .count()
            != 3
    {
        return Err(
            "production agent-context dispatch must contain context, screenshot, and action panic boundaries exactly once"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_engine_agent_redirect_contract(
    domain: &str,
    host: &str,
    navigation: &str,
    fixture: &str,
) -> Result<(), String> {
    let domain = compact(domain);
    for required in [
        "pubconstMAX_CONTEXT_NAVIGATION_REDIRECT_ORIGINS:usize=8;",
        "pubconstMAX_CONTEXT_NAVIGATION_REDIRECTS:usize=8;",
        "pubstructContextNavigationRedirectPolicy{allowed_origins:Vec<SemanticOrigin>,}",
        "ifallowed_origins.is_empty()",
        "ifallowed_origins.len()>MAX_CONTEXT_NAVIGATION_REDIRECT_ORIGINS",
        "allowed_origins.sort();",
        "allowed_origins.windows(2).any(|pair|pair[0]==pair[1])",
        "redirect_policy:Option<ContextNavigationRedirectPolicy>",
        "redirect_policy:None",
        "pubfntry_new_with_redirect_policy(",
        "pubfnallows_redirect_target(",
    ] {
        if !domain.contains(required) {
            return Err(format!(
                "agent navigation redirect domain lost bounded authority {required}"
            ));
        }
    }
    for forbidden in ["Vec<String>", "HashMap", "VecDeque", "unsafe{"] {
        if domain.contains(forbidden) {
            return Err(format!(
                "agent navigation redirect domain acquired forbidden state {forbidden}"
            ));
        }
    }

    let navigation = compact(navigation);
    for required in [
        "redirects_observed:usize",
        "fnarm_with_redirect_policy(",
        "NavigationEventPhase::Redirected",
        "ifarmed.native_id!=Some(event.id)",
        "armed.redirects_observed<MAX_CONTEXT_NAVIGATION_REDIRECTS",
        "armed.expected.allows_observed_redirect(&event.url)",
        "armed.redirects_observed+=1;",
        "expected.commit(&event.url,armed.redirects_observed)",
        "outcome:Err(ContextPortFailure::NativeRefused)",
        "#[cfg(all(feature=\"native-agentic-semantic-probe\",any(test,target_os=\"windows\")))]redirect_limit_refused:bool,",
        "#[cfg(all(feature=\"native-agentic-semantic-probe\",any(test,target_os=\"windows\")))]pub(crate)fnredirect_probe_audit(",
        "redirects_observed:u8::try_from(armed.redirects_observed).ok()?",
    ] {
        if !navigation.contains(required) {
            return Err(format!(
                "agent navigation redirect gate lost identity/bound check {required}"
            ));
        }
    }
    for forbidden in ["HashMap", "VecDeque", "thread::spawn", "channel("] {
        if navigation.contains(forbidden) {
            return Err(format!(
                "agent navigation redirect gate acquired unbounded work {forbidden}"
            ));
        }
    }

    let host = compact(host);
    for required in [
        "letredirect_policy=request.redirect_policy().cloned();",
        "Some(policy)=>binding.view.navigation().arm_with_redirect_policy(",
        "redirect_policy,watchdog,task,",
        "pending.accepts_committed_target(&committed)",
        "ifoutcome.is_err(){crate::platform::imp::stop_loading(binding.view.view());}",
    ] {
        if !host.contains(required) {
            return Err(format!(
                "agent navigation redirect host lost settlement obligation {required}"
            ));
        }
    }
    if host
        .matches("letredirect_policy=request.redirect_policy().cloned();")
        .count()
        != 2
        || host
            .matches("pending.accepts_committed_target(&committed)")
            .count()
            != 2
    {
        return Err(
            "agent navigation redirect contract must remain symmetric on macOS and Windows"
                .to_owned(),
        );
    }

    let fixture = compact(fixture);
    for required in [
        "Self::SemanticRedirectStart=>\"/semantic-redirect-start-v1\"",
        "Self::SemanticRedirectHop=>\"/semantic-redirect-hop-v1\"",
        "Self::SemanticRedirectFinal=>\"/semantic-redirect-final-v1.html\"",
        "Self::SemanticRedirectLoopA=>\"/semantic-redirect-loop-a-v1\"",
        "Self::SemanticRedirectLoopB=>\"/semantic-redirect-loop-b-v1\"",
        "Some(FixtureRoute::SemanticRedirectHop)",
        "Some(FixtureRoute::SemanticRedirectFinal)",
        "Some(FixtureRoute::SemanticRedirectLoopB)",
        "Some(FixtureRoute::SemanticRedirectLoopA)",
        "fnwrite_redirect(stream:&mutTcpStream,destination:FixtureRoute)",
        "Location:{location}\\r\\nContent-Length:0",
    ] {
        if !fixture.contains(required) {
            return Err(format!(
                "agent navigation redirect fixture lost closed route {required}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_agent_location_observation(
    port: &str,
    host: &str,
    macos: &str,
    windows: &str,
    navigation: &str,
) -> Result<(), String> {
    let port = compact(port);
    for required in [
        "fnemit_navigation_replaced",
        "ContextNativeEvent::NavigationReplaced(ContextNavigationReplacement::new(prior,target,))",
    ] {
        if !port.contains(required) {
            return Err(format!(
                "production location observer lost closed port event {required}"
            ));
        }
    }

    let host = compact(host);
    for required in [
        "navigation_replacement_rejoin_pending:bool",
        "renderer_loss_deferred_for_replacement:bool",
        "fnon_owned_agent_location_check",
        "crate::platform::imp::current_url(binding.view.view())",
        "ContextNavigationTarget::parse(&url)",
        "navigation_targets_share_origin(previous,&current)",
        "finish_location_check(true)",
        "pending.complete(Err(SemanticScreenshotNativeFailure::Stale))",
        "emitter.emit_navigation_replaced(prior,target)",
        "fnreplacement_rejoin_matches",
        "AgentReplacementAdvance::Navigation=>double_navigation_successor(prior,current)",
        "AgentReplacementAdvance::Full=>replacement_then_full_successor(prior,current)",
        "result_navigation.location_stable_for_result()",
    ] {
        if !host.contains(required) {
            return Err(format!(
                "production location observer host lost exact identity mechanism {required}"
            ));
        }
    }

    for (platform, source) in [("macOS", macos), ("Windows", windows)] {
        let source = compact(source);
        for required in [
            "_navigation_observer:super::InstalledNavigationObserver",
            "super::install_navigation_observer(&view",
            "location_events.request_location_check()",
            "observation.should_check_location()",
        ] {
            if !source.contains(required) {
                return Err(format!(
                    "production {platform} location observer lost native mechanism {required}"
                ));
            }
        }
        for forbidden in [
            "evaluate_script",
            "querySelector",
            "MutationObserver",
            "with_ipc_handler",
        ] {
            if source.contains(forbidden) {
                return Err(format!(
                    "production {platform} location observer acquired page surface {forbidden}"
                ));
            }
        }
    }

    let navigation = compact(navigation);
    for required in [
        "location_callback_pending:bool",
        "location_dirty:bool",
        "location_replacement_pending:bool",
        "fnrequest_location_check",
        "fnfinish_location_check",
        "fndefer_location_check",
        "fnrequest_deferred_location_check",
        "fnacknowledge_location_replacement",
        "fnlocation_stable_for_result",
        "state.location_callback_pending=true",
        "state.location_dirty=true",
        "state.location_replacement_pending=true",
        "||state.location_callback_pending||state.location_dirty||state.location_replacement_pending",
    ] {
        if !navigation.contains(required) {
            return Err(format!(
                "production location observer lost bounded state mechanism {required}"
            ));
        }
    }
    for forbidden in ["VecDeque", "HashMap", "thread::spawn", "channel("] {
        if navigation.contains(forbidden) {
            return Err(format!(
                "production location observer acquired unbounded/asynchronous state {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_agent_cookie_preflight(source: &str) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "pub(crate)fnmap_cookie_transfer_deadline(",
        "now.checked_duration_since(admitted_at)?",
        "admitted_at.checked_add(Duration::from_millis(window.duration_millis()))?",
        "(now<deadline).then_some(deadline)",
        "usezeroize::Zeroizing;",
        "structAgentCookieText(Zeroizing<String>);",
        "fntry_from_utf16(units:&[u16])",
        "char::decode_utf16(units.iter().copied())",
        "try_reserve_exact(utf8_bytes)",
        "name:AgentCookieText",
        "value:AgentCookieText",
        "domain:AgentCookieText",
        "path:AgentCookieText",
        "pub(crate)fntry_new(name:AgentCookieText,value:AgentCookieText,domain:AgentCookieText,path:AgentCookieText",
        "try_reserve_exact(MAX_COOKIES_PER_TRANSFER)",
        "current_origin_observations:u16",
        "observations:u16",
        "ifusize::from(current_origin_observations)>MAX_COOKIES_PER_TRANSFER",
        "MAX_COOKIE_TRANSFER_ORIGINS",
        "MAX_COOKIE_TRANSFER_BYTES",
        "MAX_COOKIE_BYTES",
        "existing.fields.same_identity(&fields)",
        "existing.fields.same_snapshot(&fields)",
        "failure:Option<AgentCookiePreflightFailure>",
        "self.poison(AgentCookiePreflightFailure::InvalidCookie)",
        "ifself.completed_origins!=self.requested_origins",
        "cookies:Vec<Option<ValidatedCookie<NativeCookie>>>",
        "and_then(Option::take)",
        "ContextCookieTransferStats::try_new(ContextCookieTransferCounts{",
        "ContextCookieTransferStats::try_new(self.counts)",
        "after_apply:ContextCookieTransferStats",
        "record_current_applied(&mutself,expected:ContextCookieTransferStats,",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "production Windows cookie preflight lost required closed mechanism {required}"
            ));
        }
    }
    let fields_marker = "pub(crate)structAgentCookieFields";
    let fields_offset = source
        .find(fields_marker)
        .ok_or_else(|| "production Windows cookie fields owner is missing".to_owned())?;
    let fields_prefix = &source[..fields_offset];
    if fields_prefix
        .rsplit('}')
        .next()
        .is_some_and(|item| item.contains("#[derive"))
    {
        return Err(
            "production Windows cookie fields must not derive diagnostic or serialization traits"
                .to_owned(),
        );
    }
    for forbidden in [
        "implfmt::DebugforAgentCookieFields",
        "SerializeforAgentCookieFields",
        "DeserializeforAgentCookieFields",
        "pubfn",
        "pub(crate)fnvalue(",
        "pub(crate)fnfields(",
        "unsafe{",
        "HashMap<",
        "unwrap_or(char::REPLACEMENT_CHARACTER)",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "production Windows cookie preflight acquired forbidden surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_cookie_transfer_deadline_contract(root: &str, source: &str) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "ContextCookieTransferInstant",
        "ContextCookieTransferWindow",
        "MAX_COOKIE_TRANSFER_MILLIS",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agent cookie transfer root lost bounded deadline export {required}"
            ));
        }
    }

    let source = compact(source);
    for required in [
        "pubconstMAX_COOKIE_TRANSFER_MILLIS:u64=30_000;",
        "pubstructContextCookieTransferInstant(u64);",
        "pubstructContextCookieTransferWindow{requested_at:ContextCookieTransferInstant,deadline:ContextCookieTransferInstant,}",
        "deadline.millis().checked_sub(requested_at.millis())",
        "ifduration==0||duration>MAX_COOKIE_TRANSFER_MILLIS",
        "pubconstfnduration_millis(self)->u64",
        "window:ContextCookieTransferWindow,",
        "pubconstfnwindow(&self)->ContextCookieTransferWindow",
        ".field(\"window\",&self.window)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent cookie transfer lost bounded deadline rule {required}"
            ));
        }
    }
    if source
        .matches("window:ContextCookieTransferWindow,")
        .count()
        != 3
    {
        return Err(
            "cookie transfer window must be retained once and required by both constructors"
                .to_owned(),
        );
    }
    for forbidden in [
        "std::time::Instant",
        "SystemTime",
        "thread::",
        "tokio::time",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "functional cookie transfer deadline acquired runtime clock authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_windows_cookie_transfer(module: &str, source: &str) -> Result<(), String> {
    let module_marker = "mod cookie_transfer;";
    let module_at = module
        .find(module_marker)
        .ok_or_else(|| "production Windows cookie adapter module is missing".to_owned())?;
    // Only the attributes attached to the item gate it: stop at the first line
    // that is neither an attribute nor a comment, so a gated sibling declared
    // just above cannot stand in for this module's gate.
    let line_start = module[..module_at].rfind('\n').map_or(0, |at| at + 1);
    let attached = module[..line_start]
        .lines()
        .rev()
        .map(str::trim)
        .take_while(|line| line.starts_with("#[") || line.starts_with("//"))
        .filter(|line| line.starts_with("#["))
        .map(compact)
        .collect::<Vec<_>>();
    if !attached
        .iter()
        .any(|gate| gate == "#[cfg(feature=\"agentic-browser\")]")
    {
        return Err(
            "production Windows cookie adapter must remain agentic-browser gated".to_owned(),
        );
    }

    let source = compact(source);
    for required in [
        "pub(crate)constAGENT_COOKIE_CLEANUP_RESERVE:Duration=Duration::from_secs(10);",
        "pub(crate)fnselected_profile_cookie_manager(",
        "super::same_environment(&view.environment(),expected_environment)",
        "let(controller_environment,manager)=unsafe{(core.Environment(),core.CookieManager())};",
        "super::same_environment(&controller_environment,expected_environment)",
        "state:RefCell<Option<TransferState>>",
        "cancellation:Cell<Option<ContextCookieTransferFailure>>",
        "terminal:Cell<bool>",
        "request:&ContextCookieTransferRequest",
        "admitted_at:Instant",
        "map_cookie_transfer_deadline(request.window(),admitted_at,now)",
        "terminal_deadline.checked_sub(AGENT_COOKIE_CLEANUP_RESERVE)",
        "Self::start_scoped(source,destination,destination_profile,request.scope().clone(),terminal_deadline,completion,callback_panicked,)",
        "AgentCookiePreflight::try_new(scope.len())",
        "pub(crate)fnstart_scoped(",
        "pub(crate)fnstart_work_sync(",
        "fnstart_mode(",
        "work_seed:retained.then(||WorkSeed{existing:Vec::new(),attempted:Vec::new(),})",
        "ifretained{check_work_destination(&shared);}else{start_origin(&shared,0);}",
        "countasusize>MAX_COOKIES_PER_TRANSFER",
        "bytes>MAX_COOKIE_TRANSFER_BYTES",
        "Ok(current)if!fields.supersedes(&current)=>continue,",
        "seed.attempted.push(cookie.clone());",
        "rollback_work_seed(shared,failure,stats);",
        ".find(|existing|existing.identity==identity).map(|existing|existing.native.clone());journal.push((cookie.clone(),identity,prior));",
        "let_=unsafe{destination.AddOrUpdateCookie(prior)};",
        "let_=unsafe{destination.DeleteCookie(attempted)};",
        "expected.iter().all(|(identity,restored)|{current.iter().any(|cookie|cookie.identity==*identity)==*restored})",
        "GetCookies(PCWSTR::from_raw(origin.as_ptr()),&handler)",
        "count>maximum",
        "state.destination.CopyCookie(&source_cookie)",
        "AgentCookieText::try_from_utf16(units)",
        "TransferPhase::Applying{in_flight:false}",
        "TransferPhase::Applying{in_flight:true}",
        "Some(TransferPhase::Applying{in_flight:true})=>{}",
        "destination.AddOrUpdateCookie(&cookie)",
        "application.record_current_applied(after_apply)",
        "destination.DeleteAllCookies()",
        "profile.ClearBrowsingDataAll(&handler)",
        "destination.GetCookies(PCWSTR::null(),&handler)",
        "count==0",
        "WindowsAgentCookieCleanup::Proven",
        "WindowsAgentCookieCleanup::Unproven",
        "ContextCookieTransferOutcome::Partial{failure,stats}",
        "shared.cancellation.set(None);",
        "shared.terminal.replace(true)",
        "std::panic::catch_unwind",
        "implDropforWindowsAgentCookieTransfer",
        "ContextCookieTransferFailure::Shutdown",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "production Windows cookie adapter lost required closed mechanism {required}"
            ));
        }
    }

    // Legacy transfers retain profile-wide rollback. A retained Work seed must
    // select its separate destination preflight and journal-only rollback.
    let legacy_scoped = source
        .split_once("fnstart_scoped(")
        .and_then(|(_, body)| body.split_once("fnstart_work_sync("))
        .map(|(body, _)| body)
        .ok_or_else(|| "production Windows legacy scoped cookie entry is missing".to_owned())?;
    let work_scoped = source
        .split_once("fnstart_work_sync(")
        .and_then(|(_, body)| body.split_once("fnstart_mode("))
        .map(|(body, _)| body)
        .ok_or_else(|| "production Windows retained scoped cookie entry is missing".to_owned())?;
    if !legacy_scoped.contains("callback_panicked,false,)")
        || !work_scoped.contains("callback_panicked,true,)")
    {
        return Err(
            "production Windows cookie entries lost exact legacy/retained mode separation"
                .to_owned(),
        );
    }

    let rollback = source
        .split_once("fnrollback_work_seed(")
        .and_then(|(_, body)| body.split_once("fnfinish_work_rollback("))
        .map(|(body, _)| body)
        .ok_or_else(|| "production Windows retained cookie rollback owner is missing".to_owned())?;
    for forbidden in ["DeleteAllCookies", "ClearBrowsingData"] {
        if rollback.contains(forbidden) {
            return Err(
                "production Windows retained cookie rollback may delete only its seed journal"
                    .to_owned(),
            );
        }
    }
    let journal = source
        .find("seed.attempted.push(cookie.clone());")
        .ok_or_else(|| {
            "production Windows retained cookie mutation journal is missing".to_owned()
        })?;

    let entered = source
        .find("state.phase=TransferPhase::Applying{in_flight:true};")
        .ok_or_else(|| "production Windows cookie write interlock is missing".to_owned())?;
    let write = source
        .find("destination.AddOrUpdateCookie(&cookie)")
        .ok_or_else(|| "production Windows cookie write is missing".to_owned())?;
    let accounted = source
        .find("application.record_current_applied(after_apply)")
        .ok_or_else(|| "production Windows cookie write accounting is missing".to_owned())?;
    if !(journal < entered && entered < write && write < accounted) {
        return Err(
            "production Windows cookie write must be interlocked before mutation and accounted afterward"
                .to_owned(),
        );
    }

    let cleanup = source
        .split_once("fnbegin_cleanup(")
        .map(|(_, cleanup)| cleanup)
        .ok_or_else(|| "production Windows cookie cleanup owner is missing".to_owned())?;
    let clear_cancellation = cleanup
        .find("shared.cancellation.set(None);")
        .ok_or_else(|| {
            "production Windows cookie cleanup cannot run after cancellation".to_owned()
        })?;
    let delete = cleanup
        .find("destination.DeleteAllCookies()")
        .ok_or_else(|| "production Windows cookie cleanup delete is missing".to_owned())?;
    let clear_all = cleanup
        .find("profile.ClearBrowsingDataAll(&handler)")
        .ok_or_else(|| "production Windows profile cleanup is missing".to_owned())?;
    let verify = cleanup
        .find("destination.GetCookies(PCWSTR::null(),&handler)")
        .ok_or_else(|| "production Windows cookie cleanup verification is missing".to_owned())?;
    if !(clear_cancellation < delete && delete < clear_all && clear_all < verify) {
        return Err(
            "production Windows cookie cleanup must delete, clear, then verify the whole profile"
                .to_owned(),
        );
    }

    for forbidden in [
        "std::sync::mpsc",
        "channel(",
        "thread::spawn",
        "thread::sleep",
        "WaitForSingleObject",
        "MsgWaitForMultipleObjects",
        "PeekMessage",
        "GetMessage",
        "DispatchMessage",
        "cookies_for_url",
        "string_from_pcwstr",
        "String::from_utf16_lossy",
        "unwrap_or(char::REPLACEMENT_CHARACTER)",
        "with_ipc_handler",
        "PostWebMessage",
        "evaluate_script",
        "ExecuteScript",
        "CallDevToolsProtocolMethod",
        "querySelector",
        "document.cookie",
        "Serialize",
        "Deserialize",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "production Windows cookie adapter acquired forbidden surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_semantic_screenshot_boundary(source: &str) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "takeSnapshotWithConfiguration_completionHandler",
        "objc2::exception::catch",
        "configuration.setAfterScreenUpdates(true)",
        "configuration.setSnapshotWidth(Some(&width))",
        "CGImageDestinationCreateWithDataConsumer",
        "CGDataConsumerCallbacks",
        "end>self.limit",
        "try_reserve_exact",
        "budget.max_png_bytes()",
        "cancelled.load(Ordering::Acquire)",
        "completed_at>request.deadline()",
        "SemanticScreenshotPaintEvidence::ExactDocumentContentAvailable",
        "CFRelease(self.0.as_ptr())",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "production macOS semantic screenshot lost required bounded mechanism {required}"
            ));
        }
    }
    for forbidden in [
        "NSMutableData",
        "TIFFRepresentation",
        "evaluateJavaScript",
        "callAsyncJavaScript",
        "evaluate_script",
        "querySelector",
        "CGEvent",
        "NSEvent",
        "write(",
        "File::create",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "production macOS semantic screenshot acquired forbidden surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_windows_semantic_screenshot(
    module: &str,
    agent_context: &str,
    buffer: &str,
    adapter: &str,
) -> Result<(), String> {
    let module = compact(module);
    let agent_context = compact(agent_context);
    let buffer = compact(buffer);
    let adapter = compact(adapter);
    if !module
        .contains("#[cfg(feature=\"agentic-browser\")]#[allow(dead_code)]modsemantic_screenshot;")
    {
        return Err("production Windows semantic screenshot lost its gated module".into());
    }
    let screenshot = agent_context
        .split_once("pub(crate)fndispatch_screenshot(")
        .map(|(_, method)| method.split("pub(crate)fn").next().unwrap_or(method))
        .ok_or("production Windows semantic screenshot lost its dispatch method")?;
    // ad2ef7cb: the capture additionally runs only behind the pixel privacy proof.
    let guarded_capture = "ifself.semantic().is_none_or(|semantic|semantic.document_content_available_for_audit()!=Some(true))||(self.work_navigation.is_none()&&attest_hidden_owner(&self.view,self.expected_parent,self.viewport).is_err()){returnErr(SemanticScreenshotNativeFailure::NotReady);}letSome(semantic)=self.semantic().cloned()else{returnErr(SemanticScreenshotNativeFailure::NotReady);};letnative=self.view.webview();letcapture_cancelled=cancelled.clone();letpublic=request.requires_public_page();crate::platform::semantic_pixel_privacy::guarded_model_capture(public,Rc::new(move|completion|{ifpublic{semantic.dispatch_public_privacy(completion)}else{semantic.dispatch_privacy(completion)}}),move|completion|{super::semantic_screenshot::capture_viewport(";
    if !screenshot.contains(guarded_capture) {
        return Err("production Windows semantic screenshot lost its exact early ownership refusal before capture".into());
    }
    for required in [
        "structBoundedScreenshotBuffer",
        "checked_add(source.len())",
        "end>self.limit",
        "try_reserve_exact",
        "fnbounded_png_dimensions(",
        "budget.max_png_bytes()",
        "header.get(12..16)!=Some(b\"IHDR\".as_slice())",
        "pixels>budget.max_pixels()",
    ] {
        if !buffer.contains(required) {
            return Err(format!(
                "production Windows semantic screenshot lost bounded buffer rule {required}"
            ));
        }
    }
    for required in [
        "#[windows_core::implement(IStream)]",
        "implISequentialStream_ImplforBoundedCaptureStream_Impl",
        "implIStream_ImplforBoundedCaptureStream_Impl",
        "buffer.write(source)",
        "COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG",
        ".CapturePreview(",
        "request.budget().max_png_bytes()",
        "cancelled.load(Ordering::Acquire)",
        "completed_at>request.deadline()",
        "bounded_png_dimensions(&png,request.budget())",
        "SemanticScreenshotPaintEvidence::ExactDocumentContentAvailable",
    ] {
        if !adapter.contains(required) {
            return Err(format!(
                "production Windows semantic screenshot lost bounded native mechanism {required}"
            ));
        }
    }
    for forbidden in [
        "SHCreateStreamOnFile",
        "CreateStreamOnHGlobal",
        "HGLOBAL",
        "File::create",
        "OpenOptions",
        "std::fs",
        "COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG",
        "ExecuteScript",
        "evaluate_script",
        "querySelector",
        "CallDevToolsProtocolMethod",
        "Input.dispatch",
        "SendInput",
        "SetFocus",
        "SetForegroundWindow",
        "println!",
    ] {
        if buffer.contains(forbidden) || adapter.contains(forbidden) {
            return Err(format!(
                "production Windows semantic screenshot acquired forbidden surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_windows_agent_context_boundary(
    module: &str,
    adapter: &str,
    timeout: &str,
    navigation: &str,
    host: &str,
) -> Result<(), String> {
    let module = compact(module);
    for required in [
        "#[cfg(feature=\"agentic-browser\")]modagent_context;",
        "#[allow(dead_code)]modsemantic_runtime;",
        "modtimeout;",
        "build_owned_agent_view",
        "pub(crate)usecookie_transfer::{selected_profile_cookie_manager,WindowsAgentCookieCleanup,WindowsAgentCookieTerminal,WindowsAgentCookieTransfer,};",
        "schedule_content_policy_timeout",
    ] {
        if !module.contains(required) {
            return Err(format!(
                "production Windows agent-context module lost required gate {required}"
            ));
        }
    }

    let adapter = compact(adapter);
    for required in [
        "with_url(\"about:blank\")",
        "with_visible(false)",
        "with_focused(false)",
        "with_devtools(false)",
        "with_clipboard(false)",
        "with_permission_handler(|_|wry::PermissionResponse::Deny)",
        "with_download_policy(DownloadPolicy::DenyWithoutMetadata)",
        "with_environment(environment.clone())",
        "builder.with_profile_name(name.clone())",
        "builder.with_incognito(true)",
        "attest_profile(&profile,&view.environment(),&view.webview(),",
        "super::attest_environment(environment,expected_user_data_folder)",
        "controller_environment_matches(environment,core)",
        "profile_is_private(&profile)",
        "IsWindowVisible(container)",
        "GetParent(container)",
        "controller.ParentWindow(&mutcontroller_parent)",
        "GetFocus()",
        "super::install_crash_handler",
        "AgentSemanticRuntimePlan::prepare()",
        "semantic_plan.bind(&view.webview(),semantic_invariant,semantic_panic)",
        "semantic_navigation.document_committed()",
        "semantic_renderer.renderer_lost()",
        "semantic.controller().attest(&view.webview())",
        "pub(crate)fnretire_semantic_runtime(&mutself)->bool",
        "pub(crate)fncookie_destination(",
        "matches!(self.profile,AgentOwnedProfile::Automation{..})",
        "super::same_environment(&self.view.environment(),expected_environment)",
        "self.attest()?",
        "ifself.attest_suspension_state()?",
        "core.CookieManager()",
        "profile.cast::<ICoreWebView2Profile2>()",
        "pub(crate)fnclose(&mutself)->Result<(),wry::WebView2CleanupDebt>",
        "ContextConstructionProof::WindowsOwnedSelectedProfileEmptyInventory",
        "ContextConstructionProof::WindowsOwnedAutomationSubprofileEmptyInventory",
        "ifextensions_enabled{if!matches!(&profile,AgentOwnedProfile::Automation{..}){returnErr(AgentOwnedViewConstructionError::ExtensionIsolation);}",
        "builder=builder.with_controller_initialization_hook(|_,core|crate::reputation::configure(core));",
        "builder.with_browser_extension_startup_gate(move|environment,core|{",
        "if!super::same_environment(&expected_environment,environment){returnErr(windows::Win32::Foundation::E_ACCESSDENIED.into());}",
        "attest_profile(&expected_profile,environment,core,storage_class,&expected_path,)",
        "letitems=super::extensions::list(&profile)",
        "if!super::extensions::is_runtime_component(&id){returnErr(windows::Win32::Foundation::E_ACCESSDENIED.into());}",
    ] {
        if !adapter.contains(required) {
            return Err(format!(
                "production Windows agent-context adapter lost required check {required}"
            ));
        }
    }

    validate_engine_windows_timeout_thread_binding(timeout)?;
    let timeout = compact(timeout);
    for required in [
        "MAX_PENDING_NATIVE_CONTEXT_TASKS",
        "[Option<TimerEntry>;MAX_UI_TIMERS]",
        "schedule_timeout(duration,callback,0..MAX_AGENT_UI_TIMERS)",
        "SetTimer(None,0,interval,Some(timer_proc))",
        "KillTimer(None,self.timer)",
        "catch_unwind",
    ] {
        if !timeout.contains(required) {
            return Err(format!(
                "production Windows agent-context timeout lost required bound {required}"
            ));
        }
    }
    for forbidden in ["HashMap", "thread::spawn", "thread::sleep", "channel("] {
        if timeout.contains(forbidden) {
            return Err(format!(
                "production Windows agent-context timeout acquired forbidden authority {forbidden}"
            ));
        }
    }

    let navigation = compact(navigation);
    for required in [
        "native_id:Option<wry::NavigationId>",
        "armed.native_id!=Some(event.id)",
        "terminal_claimed.compare_exchange",
        "state.bootstrap_available=false",
        "fnclaim_renderer_loss",
        "fnarm_recovery",
        "fnsettle_recovery",
        "NavigationEventPhase::Redirected",
    ] {
        if !navigation.contains(required) {
            return Err(format!(
                "production Windows agent navigation gate lost required mechanism {required}"
            ));
        }
    }

    let host = compact(host);
    for required in [
        "ensure_windows_profile_environment_at_path",
        "browser_process(view.view())",
        "install_content_policy_on_view(view.view(),&content_policy)",
        "resource.reclassify(NativeResourceClass::AgentContext)",
        "OwnedWindowsCleanupDebt::new",
        "binding.retire(ContextPortFailure::Shutdown)",
        "fnclose_unpublished_windows_agent_view",
        "binding.view.prepare_semantic_document_load()",
        "letsemantic_clean=self.view.retire_semantic_runtime();",
        "semantic_clean:bool",
        "letsemantic_pending=self.view.semantic_pending_for_audit()?;",
        "structAgentPendingCookieTransfer{",
        "pending_cookie_transfer:Option<ContextCookieTransferId>",
        "cookie_contaminated:bool",
        "cookie_bindings_consistent",
        "fnstart_windows_agent_cookie_transfer",
        "ContextCookieTransferDirection::SelectedProfileToOwned",
        "self.agent_cookie_transfers.len()>=MAX_PENDING_COOKIE_TRANSFERS",
        "pending.destination_profile==destination_profile",
        "self.selected_profile_cookie_source(",
        ".cookie_destination(&expected_environment)",
        "map_cookie_transfer_deadline(request.window(),admitted_at,Instant::now(),)",
        "schedule_content_policy_timeout(watchdog_duration",
        "fnfinish_windows_agent_cookie_transfer",
        "WindowsAgentCookieCleanup::Proven",
        "agent_cookie_quarantined_profiles.insert(destination_profile)",
        "binding.cookie_contaminated=true",
        ".cancel(ContextCookieTransferFailure::Cancelled)",
        "std::mem::take(&mutself.agent_cookie_transfers)",
    ] {
        if !host.contains(required) {
            return Err(format!(
                "production Windows agent-context host lost required obligation {required}"
            ));
        }
    }

    let windows_host = host
        .split_once("fnclose_unpublished_windows_agent_view")
        .and_then(|(_, source)| source.split_once("fnstart_owned_agent_navigation"))
        .map(|(_, source)| source)
        .ok_or_else(|| "production Windows navigation owner is missing".to_owned())?;
    let (windows_navigation, windows_host) = windows_host
        .split_once("fnstart_owned_agent_recovery")
        .ok_or_else(|| "production Windows recovery owner is missing".to_owned())?;
    let windows_recovery = windows_host
        .split_once("fnon_owned_agent_navigation_terminal")
        .map(|(source, _)| source)
        .ok_or_else(|| "production Windows recovery terminal boundary is missing".to_owned())?;
    if !windows_navigation.contains("binding.view.prepare_semantic_document_load()")
        || !windows_recovery.contains("binding.view.prepare_semantic_document_load()")
    {
        return Err(
            "production Windows navigation and recovery must rotate semantic authority".to_owned(),
        );
    }

    for (label, source) in [
        ("adapter", adapter.as_str()),
        ("navigation gate", navigation.as_str()),
        ("host", host.as_str()),
    ] {
        // Reputation configuration is independent of extension enablement.
        // Only the admitted automation subprofile owns an extension startup gate.
        let expected_gates = if label == "adapter" { 1 } else { 0 };
        if source
            .matches("with_browser_extension_startup_gate")
            .count()
            != expected_gates
        {
            return Err(format!(
                "production Windows agent-context {label} changed its startup gate authority"
            ));
        }
        for forbidden in [
            "with_ipc_handler",
            "with_initialization_script",
            "evaluate_script",
            "querySelector",
            "CallDevToolsProtocolMethod",
            "SendInput",
            "mouse_event",
            "keybd_event",
            "CGEvent",
            "native-agentic-input-probe",
            "with_browser_extensions_enabled",
            "extensions::add(",
            "extensions::enable(",
            "extensions::remove(",
            "AddBrowserExtension",
        ] {
            if source.contains(forbidden) {
                return Err(format!(
                    "production Windows agent-context {label} acquired forbidden surface {forbidden}"
                ));
            }
        }
    }
    Ok(())
}

fn validate_engine_windows_agent_cookie_source(module: &str, source: &str) -> Result<(), String> {
    let module = compact(module);
    if !module.contains(
        "#[cfg(all(feature=\"agentic-browser\",target_os=\"windows\"))]modagent_cookie_source;",
    ) {
        return Err(
            "ordinary-profile agent cookie authority lost its Windows feature gate".to_owned(),
        );
    }

    let source = compact(source);
    for required in [
        "pub(super)fnselected_profile_cookie_source(",
        "expected_environment:&ICoreWebView2Environment",
        "for(id,partition)in&self.partitions",
        "ifpartition.profile()!=profile",
        "self.views.get(id).ok_or(ContextCookieTransferFailure::SourceUnavailable)?",
        "selected_profile_cookie_manager(&view.view,expected_environment,)?",
        ".filter(|spare|spare.partition.profile()==profile)",
        "selected_profile_cookie_manager(&spare.view.view,expected_environment,)?",
        "source.ok_or(ContextCookieTransferFailure::SourceUnavailable)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "ordinary-profile agent cookie authority lost required check {required}"
            ));
        }
    }
    for forbidden in [
        "GetCookies",
        "AddOrUpdateCookie",
        "DeleteAllCookies",
        "usezephium_core::ids::ItemId",
        "self.stages",
        "extension_document_authority",
        "extension_browser_surfaces",
        "navigation_snapshots",
        "println!",
        "eprintln!",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "ordinary-profile agent cookie authority acquired forbidden surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_windows_agent_suspension_boundary(
    platform_module: &str,
    adapter: &str,
    suspension: &str,
    host: &str,
    content_rules: &str,
    port: &str,
) -> Result<(), String> {
    let platform_module = compact(platform_module);
    if !platform_module.contains(
        "#[cfg(all(feature=\"agentic-browser\",any(target_os=\"windows\",test)))]pub(crate)modagent_suspension;",
    ) {
        return Err(
            "production Windows suspension claim lost its target/feature gate".to_owned(),
        );
    }

    let adapter = compact(adapter);
    for required in [
        "core.TrySuspend(&handler)",
        "core.IsSuspended(&mutsuspended)",
        "core.Resume()",
        "fnattest_suspension_state",
        "fnresume_and_attest_active",
        "catch_unwind",
    ] {
        if !adapter.contains(required) {
            return Err(format!(
                "production Windows suspension adapter lost native proof {required}"
            ));
        }
    }

    let suspension = compact(suspension);
    for required in [
        "state:Arc<AtomicU8>",
        "compare_exchange(PENDING,TIMED_OUT",
        "compare_exchange(PENDING,CANCELLED",
        "LATE_NATIVE_COMPLETED",
        "AgentSuspendNativeDisposition::Terminal",
        "AgentSuspendNativeDisposition::Reconcile",
        "AgentSuspendNativeDisposition::Retired",
        "AgentSuspendNativeDisposition::Duplicate",
        "self.state.store(RETIRED,Ordering::Release)",
    ] {
        if !suspension.contains(required) {
            return Err(format!(
                "production Windows suspend ownership lost required mechanism {required}"
            ));
        }
    }
    for forbidden in [
        "thread::spawn",
        "thread::sleep",
        "channel(",
        "Mutex<",
        "Condvar",
        "println!",
        "eprintln!",
    ] {
        if suspension.contains(forbidden) {
            return Err(format!(
                "production Windows suspend ownership acquired forbidden work {forbidden}"
            ));
        }
    }

    let host = compact(host);
    for required in [
        "constAGENT_SUSPEND_TIMEOUT:Duration=Duration::from_secs(10);",
        "pending_suspend:Option<AgentPendingSuspend>",
        "late_suspend_claim:Option<(",
        "enumAgentNativeSuspendState",
        "fnstart_owned_agent_suspend",
        "fnfinish_owned_agent_suspend_timeout",
        "fnon_owned_agent_suspend_native",
        "fnreconcile_owned_agent_suspend",
        "fnresume_owned_agent_context",
        "schedule_content_policy_timeout(AGENT_SUSPEND_TIMEOUT",
        "binding.suspend_state=AgentNativeSuspendState::Uncertain(operation)",
        "suspended_views:suspended_view_count",
        "pending.claim.retire()",
    ] {
        if !host.contains(required) {
            return Err(format!(
                "production Windows suspend host lost required obligation {required}"
            ));
        }
    }

    let suspend_start = host
        .split_once("fnstart_owned_agent_suspend(")
        .and_then(|(_, source)| {
            source
                .split_once("fnfinish_owned_agent_suspend_timeout(")
                .map(|(source, _)| source)
        })
        .ok_or_else(|| "production Windows suspend start boundary is missing".to_owned())?;
    let join = suspend_start
        .find("binding.join=requested;")
        .ok_or_else(|| "production Windows suspend does not retain the accepted join".to_owned())?;
    let dynamic_admission = suspend_start
        .find("letnative_failure=")
        .ok_or_else(|| "production Windows suspend dynamic admission is missing".to_owned())?;
    let watchdog = suspend_start
        .find("schedule_content_policy_timeout(AGENT_SUSPEND_TIMEOUT")
        .ok_or_else(|| "production Windows suspend watchdog is missing".to_owned())?;
    let publish_pending = suspend_start
        .find("binding.pending_suspend=Some(AgentPendingSuspend")
        .ok_or_else(|| "production Windows suspend pending publication is missing".to_owned())?;
    let claim_native = suspend_start
        .find("callback_claim.native_completed()")
        .ok_or_else(|| {
            "production Windows suspend callback ownership claim is missing".to_owned()
        })?;
    let queue_native = suspend_start[claim_native..]
        .find("try_with_agent_context_terminal(move|host|")
        .map(|offset| claim_native + offset)
        .ok_or_else(|| {
            "production Windows suspend callback terminal dispatch is missing".to_owned()
        })?;
    let dispatch_native = suspend_start
        .find(".try_suspend(")
        .ok_or_else(|| "production Windows suspend native dispatch is missing".to_owned())?;
    if !(join < dynamic_admission
        && dynamic_admission < watchdog
        && watchdog < publish_pending
        && publish_pending < dispatch_native
        && claim_native < queue_native)
    {
        return Err(
            "production Windows suspend must rejoin, admit, arm, publish, then dispatch, and its callback must claim before queuing"
                .to_owned(),
        );
    }

    let suspend_timeout = host
        .split_once("fnfinish_owned_agent_suspend_timeout(")
        .and_then(|(_, source)| {
            source
                .split_once("fnfinish_owned_agent_suspend_immediate_failure(")
                .map(|(source, _)| source)
        })
        .ok_or_else(|| "production Windows suspend timeout boundary is missing".to_owned())?;
    let retain_late = suspend_timeout
        .find("binding.late_suspend_claim=Some((operation,pending.claim.clone()));")
        .ok_or_else(|| "production Windows suspend timeout drops late cleanup debt".to_owned())?;
    let settle_timeout = suspend_timeout
        .find("pending.complete(Err(ContextPortFailure::TimedOut));")
        .ok_or_else(|| "production Windows suspend timeout settlement is missing".to_owned())?;
    if retain_late >= settle_timeout {
        return Err(
            "production Windows suspend timeout must retain late cleanup before external settlement"
                .to_owned(),
        );
    }

    let resume = host
        .split_once("fnresume_owned_agent_context(")
        .and_then(|(_, source)| {
            source
                .split_once("fnon_owned_agent_navigation_terminal(")
                .map(|(source, _)| source)
        })
        .ok_or_else(|| "production Windows resume boundary is missing".to_owned())?;
    let resume_join = resume
        .find("binding.join=requested;")
        .ok_or_else(|| "production Windows resume does not retain the accepted join".to_owned())?;
    let resume_native = resume
        .find("binding.view.resume_and_attest_active()")
        .ok_or_else(|| "production Windows resume native postcondition is missing".to_owned())?;
    if resume_join >= resume_native {
        return Err(
            "production Windows resume must retain the accepted join before native work".to_owned(),
        );
    }

    for required in [
        "ifletSome((_,claim))=self.late_suspend_claim.take(){claim.retire();}",
        "ifletSome((_,claim))=binding.late_suspend_claim.take(){claim.retire();}",
    ] {
        if !host.contains(required) {
            return Err(format!(
                "production Windows suspend teardown lost retained callback retirement {required}"
            ));
        }
    }

    let content_rules = compact(content_rules);
    if !content_rules.contains("if!context.permits_content_policy_install()") {
        return Err(
            "Windows content-policy replacement can touch a suspended owned context".to_owned(),
        );
    }

    let port = compact(port);
    for required in [
        "#[cfg(target_os=\"windows\")]",
        "ContextOperationKind::Suspend|ContextOperationKind::Resume|ContextOperationKind::Recover|ContextOperationKind::Close",
    ] {
        if !port.contains(required) {
            return Err(format!(
                "Windows native context port lost suspension support gate {required}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_windows_timeout_thread_binding(source: &str) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "_thread_bound:PhantomData<Rc<()>>",
        "ContentPolicyTimeout{timer,generation,_thread_bound:PhantomData",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "production Windows UI timeout lost creating-thread confinement {required}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_windows_semantic_protocol(module: &str, source: &str) -> Result<(), String> {
    let module = compact(module);
    if !module.contains(
        "#[cfg(all(feature=\"agentic-browser\",any(target_os=\"windows\",test)))]",
    ) || !module.contains(
        "#[cfg_attr(all(target_os=\"windows\",not(test)),allow(dead_code))]modagent_semantic_cdp_protocol;",
    ) {
        return Err(
            "production Windows semantic CDP protocol escaped its agentic-browser platform gate"
                .to_owned(),
        );
    }

    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let production = compact(production);
    let source = compact(source);
    for required in [
        "enumFixedSemanticCdpMethod",
        "Page.getFrameTree",
        "Runtime.enable",
        "Page.createIsolatedWorld",
        "Runtime.disable",
        "Runtime.callFunctionOn",
        // Windows Work actions use only these closed CDP input methods after
        // isolated target preparation, with a guard retained through delivery.
        "Self::DispatchMouseEvent=>\"Input.dispatchMouseEvent\"",
        "Self::DispatchKeyEvent=>\"Input.dispatchKeyEvent\"",
        "Self::InsertText=>\"Input.insertText\"",
        "fnprepare_action_command",
        "fnnext_action_command",
        "fndecode_action_response",
        "plan.len()!=4",
        "invocation.attempt().get()",
        "payload.len()==6",
        "payload.len()==5",
        "payload.len()==1",
        "text.len()<=4096",
        "install_runtime_in_context_command",
        // 812c53c4: CDP installs the hash-pinned whitespace-compacted twin.
        "SEMANTIC_RUNTIME_PROGRAM.cdp_source()",
        "MAX_SEMANTIC_RUNTIME_SOURCE_BYTES",
        "MAX_SEMANTIC_RUNTIME_REQUEST_BYTES",
        "MAX_SEMANTIC_WIRE_BYTES",
        "MAX_CONTROL_PARAMETERS_BYTES:usize=128*1_024",
        "constEMPTY_CONTROL_PARAMETERS:&str=\"{}\";",
        "assert!(EMPTY_CONTROL_PARAMETERS.len()<=MAX_CONTROL_PARAMETERS_BYTES);",
        "fnempty_control_command(method:FixedSemanticCdpMethod)->FixedSemanticCdpCommand",
        "parameters:EMPTY_CONTROL_PARAMETERS.to_owned()",
        "MAX_CONTEXT_EVENTS_PER_INVOCATION:u16=512",
        "MAX_CONTEXT_EVENT_BYTES_PER_INVOCATION:usize=512*1_024",
        "SemanticWorldName::from_nonce",
        "ifepoch==0||unpredictable==0",
        "declaration.push_str(source)",
        "\"grantUniveralAccess\":false",
        "\"uniqueContextId\":context.unique_id()",
        "\"returnByValue\":true",
        "\"generatePreview\":false",
        "\"userGesture\":false",
        "\"awaitPromise\":false",
        "auxiliary.get(\"isDefault\").and_then(Value::as_bool)!=Some(false)",
        "auxiliary.get(\"type\").and_then(Value::as_str)!=Some(\"isolated\")",
        "created==observed.id",
        "object.contains_key(\"error\")",
        "object.contains_key(\"exceptionDetails\")",
        "SemanticWorldName([redacted])",
        "SemanticContextDiscovery",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "production Windows semantic CDP protocol lost required closed mechanism {required}"
            ));
        }
    }
    if source.contains("\"userGesture\":true") {
        return Err("production Windows semantic CDP protocol grants user activation".to_owned());
    }
    if production.contains("expect(")
        || production.contains("unwrap(")
        || production.contains("panic!")
        || production.contains("unreachable!")
        || production.matches("empty_control_command(").count() != 4
    {
        return Err(
            "production Windows semantic CDP protocol regained an invariant abort or non-fixed empty command"
                .to_owned(),
        );
    }
    for (offset, _) in production.match_indices("\"Input.") {
        let method = production[offset + 1..]
            .split('"')
            .next()
            .unwrap_or_default();
        if !matches!(
            method,
            "Input.dispatchMouseEvent" | "Input.dispatchKeyEvent" | "Input.insertText"
        ) {
            return Err(
                "production semantic protocol acquired an unreviewed native input method"
                    .to_owned(),
            );
        }
    }
    for forbidden in [
        "Page.addScriptToEvaluateOnNewDocument",
        "Page.removeScriptToEvaluateOnNewDocument",
        "Runtime.evaluate",
        "Page.bringToFront",
        "Page.captureScreenshot",
        "Input.dispatchTouchEvent",
        "Input.dispatchDragEvent",
        "Input.synthesize",
        "Runtime.addBinding",
        "PostWebMessage",
        "with_ipc_handler",
        "querySelector",
        "outerHTML",
        "document.cookie",
        "localStorage",
        "sessionStorage",
        "method:&str",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "production Windows semantic CDP protocol acquired forbidden surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_windows_semantic_runtime(module: &str, source: &str) -> Result<(), String> {
    let module = compact(module);
    if !module.contains("#[allow(dead_code)]modsemantic_runtime;") {
        return Err(
            "production Windows semantic runtime escaped its agentic-browser gate".to_owned(),
        );
    }

    let source = compact(source);
    for required in [
        "windows_core::GUID::new()",
        "SemanticWorldName::from_nonce(epoch,unpredictable)",
        "letnext_world=next_world_name()?;",
        "state.world=next_world;",
        "ICoreWebView2CallDevToolsProtocolMethodCompletedHandler",
        "GetDevToolsProtocolEventReceiver(&event_name)",
        "Runtime.executionContextCreated",
        "CallDevToolsProtocolMethod(&method,&parameters,&handler)",
        "borrowed_pcwstr_bounded(response,self.response_limit,self.response_limit)",
        "super::take_pwstr_bounded(raw,MAX_CONTEXT_EVENT_BYTES,MAX_CONTEXT_EVENT_BYTES)",
        "MAX_CLEANUP_DISABLE_ATTEMPTS:u8=1",
        "in_flight:Option<InFlightCommand>",
        "state.context_events_enabled=true",
        "CommandStage::CleanupRuntimeDisable",
        "install_runtime_in_context_command(&context)",
        "decode_runtime_install_response(&response)",
        "state.pending.as_ref()?.invocation.command(context)",
        "enumReadInvocation{HumanWall,Observation(Box<SemanticRuntimeInvocation>),Privacy(bool),Media(MediaInvocation),}",
        "Self::HumanWall=>{crate::platform::agent_semantic_cdp_protocol::invoke_human_wall_command(context).ok()}",
        "Self::Observation(value)=>invoke_runtime_command(context,value).ok(),",
        "Self::Privacy(public)=>{crate::platform::agent_semantic_cdp_protocol::invoke_privacy_command_mode(context,*public,).ok()}",
        "Self::Media(value)=>{crate::platform::agent_semantic_cdp_protocol::invoke_media_command(context,value).ok()}",
        "state.installed_context=None",
        "state.document_generation=state.document_generation.checked_add(1)",
        "fnwork_drained_for_audit(&self)->Option<bool>",
        "state.pending.is_none()&&state.pending_action.is_none()&&state.in_flight.is_none()&&state.discovery.is_none()&&!state.context_events_enabled&&!state.cleanup_disable_pending",
        "fnrevoke_document_authority(&self)",
        "pending_action:Option<PendingAction>",
        "state.pending_action.take()",
        "self.action_permitted()",
        "pending.document_generation",
        "self.dispatch_action_next()",
        "state.settling_revoked=true",
        "SemanticRuntimePortFailure::DocumentReplaced",
        "SemanticRuntimePortFailure::RendererLost",
        "SemanticRuntimePortFailure::TimedOut",
        "remove_DevToolsProtocolEventReceived(self.token)",
        "same_interface(&self.core,core)",
        "formatter.write_str(\"AgentSemanticRuntimeController([native,redacted])\")",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "production Windows semantic runtime lost required bounded mechanism {required}"
            ));
        }
    }
    for forbidden in [
        "Page.addScriptToEvaluateOnNewDocument",
        "Page.removeScriptToEvaluateOnNewDocument",
        "Runtime.evaluate",
        "ExecuteScript(",
        "evaluate_script",
        "querySelector",
        "outerHTML",
        "Input.dispatch",
        "PostWebMessage",
        "with_ipc_handler",
        "with_initialization_script",
        "AddHostObject",
        "MsgWaitForMultipleObjectsEx",
        "PeekMessageW",
        "thread::spawn",
        "channel(",
        "method:&str",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "production Windows semantic runtime acquired forbidden authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_owned_context_viewport_contract(
    context_port: &str,
    host: &str,
    macos: &str,
    windows: &str,
) -> Result<(), String> {
    let context_port = compact(context_port);
    for required in [
        "pubstructContextOwnedViewport{",
        "width:u16,",
        "height:u16,",
        "pubconstSTANDARD:Self=Self{width:1_280,height:800,}",
        "Self::Owned=>Some(ContextOwnedViewport::STANDARD)",
        "pubconstfnlogical_area(self)->u32",
    ] {
        if !context_port.contains(required) {
            return Err(format!(
                "owned-context viewport contract lost closed value {required}"
            ));
        }
    }
    if context_port
        .matches("pubconstfnowned_viewport(self)->Option<ContextOwnedViewport>")
        .count()
        != 2
    {
        return Err(
            "owned-context source and construction request must both retain the fixed viewport"
                .to_owned(),
        );
    }

    let host = compact(host);
    if host.matches("request.owned_viewport()").count() != 2
        || host.matches("ContextOwnedViewport::STANDARD").count() != 2
        || host.matches("build_owned_agent_view(").count() < 2
    {
        return Err(
            "both production platform owners must derive and pass the closed viewport".to_owned(),
        );
    }

    let macos = compact(macos);
    for required in [
        "viewport:ContextOwnedViewport",
        "with_bounds(Rect{",
        "self.viewport",
        "native_view.setAutoresizingMask(Mask::ViewNotSizable)",
        "page.isInspectable()",
        "native_view.autoresizingMask()!=Mask::ViewNotSizable",
        ".with_background_throttling(wry::BackgroundThrottlingPolicy::Throttle)",
        "configuration.preferences().inactiveSchedulingPolicy()",
        "scheduling!=objc2_web_kit::WKInactiveSchedulingPolicy::Throttle",
        "frame.size.width!=f64::from(viewport.width())",
        "frame.size.height!=f64::from(viewport.height())",
    ] {
        if !macos.contains(required) {
            return Err(format!(
                "production macOS owned viewport lost native attestation {required}"
            ));
        }
    }

    let windows = compact(windows);
    for required in [
        "viewport:ContextOwnedViewport",
        "with_bounds(Rect{",
        "self.viewport",
        "GetDpiForWindow(container)",
        "expected_physical_extent(viewport.width(),dpi)",
        "expected_physical_extent(viewport.height(),dpi)",
        "GetClientRect(container,&mutcontainer_bounds)",
        "controller.Bounds(&mutcontroller_bounds)",
    ] {
        if !windows.contains(required) {
            return Err(format!(
                "production Windows owned viewport lost DPI-aware attestation {required}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_semantic_runtime_boundary(source: &str) -> Result<(), String> {
    let source = compact(source);
    if source.contains("program_probe") {
        for required in [
            "#[cfg(feature=\"native-agentic-semantic-probe\")]#[path=\"agentic_semantic_program_probe.rs\"]modprogram_probe;",
            "#[cfg(any(not(feature=\"native-agentic-semantic-probe\"),test))]usezephium_agentic::SEMANTIC_RUNTIME_PROGRAM;",
            "#[cfg(not(feature=\"native-agentic-semantic-probe\"))]letsource=NSString::from_str(SEMANTIC_RUNTIME_PROGRAM.installation_source());",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]letsource=NSString::from_str(&program_probe::source());",
            "#[cfg(not(feature=\"native-agentic-semantic-probe\"))]letsource_mismatch=source.to_string()!=SEMANTIC_RUNTIME_PROGRAM.installation_source();",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]letsource_mismatch=source.to_string()!=program_probe::source();",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]letbytes=program_probe::normalize_diagnostic(bytes);",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]prepared_fill:Option<program_probe::PreparedFill>,",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]prepared_fill:None,",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]ifletSome(mutprepared)=self.prepared_fill.take(){ifletSome(reply)=prepared.stop(){actions.push_reply(reply);}}",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]ifbody==program_probe::PREPARE_MESSAGE&&program_probe::preparation_selected(){returnself.on_fill_preparation(reply);}",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]ifself.prepared_fill.as_ref().is_some_and(program_probe::PreparedFill::waiting){returnself.fail_transport(Some(reply));}",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]{self.prepared_fill=None;}",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]pub(crate)fnpoll_prepared_fill(&self){letactions=self.transition(SemanticRuntimeChannelState::poll_fill_preparation);self.execute(actions);}",
            "#[cfg(feature=\"native-agentic-semantic-probe\")]ifletErr(AgentSemanticActionRuntimeFailure::Result(SemanticActionRuntimeResultError::Runtime(fault),))=&outcome{program_probe::record_fault(fault);}",
        ] {
            if !source.contains(required) {
                return Err(format!(
                    "fixed semantic qualification source escaped its exact gate: {required}"
                ));
            }
        }
        if source.matches("program_probe::source()").count() != 2 {
            return Err("fixed semantic qualification acquired another source call".to_owned());
        }
    }
    let expected_probe_gates = if source.contains("program_probe") {
        // Includes stopping the existing preparation reply on authority loss
        // and the regression proving late preparation cannot reopen it, plus
        // the probe-only record media and form receipt diagnostics.
        28
    } else {
        0
    };
    if source.matches("native-agentic-semantic-probe").count() != expected_probe_gates {
        return Err("semantic runtime acquired an unreviewed diagnostic gate".to_owned());
    }
    for required in [
        "addScriptMessageHandlerWithReply_contentWorld_name",
        "WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld",
        "WKUserScriptInjectionTime::AtDocumentStart,true,&world",
        "message.world()",
        "message.name()",
        "message.webView()",
        "message.frameInfo()",
        "frame.isMainFrame()",
        "NEXT_SEMANTIC_RUNTIME_WORLD.fetch_update",
        "SEMANTIC_RUNTIME_WORLD_NAME_PREFIX",
        "channel.world_matches(&world)",
        "body.lengthOfBytesUsingEncoding(NSUTF8StringEncoding)",
        "MAX_SEMANTIC_RUNTIME_CHANNEL_RESULT_BYTES",
        "MAX_SEMANTIC_RUNTIME_DOCUMENT_INVOCATIONS",
        "SEMANTIC_RUNTIME_PROGRAM.installation_source()",
        "scripts.count()!=1",
        "scripts.objectAtIndex(0)",
        "Retained::as_ptr(&script)!=Retained::as_ptr(&active.script)",
        "source.to_string()!=SEMANTIC_RUNTIME_PROGRAM.installation_source()",
        "removeScriptMessageHandlerForName_contentWorld",
        "controller.removeAllScriptMessageHandlers()",
        "controller.removeAllUserScripts()",
        "fnbegin_document_load",
        "fnprepare_document_load",
        "fndocument_committed",
        "fnrenderer_lost",
        "fncancel",
        "fnrevoke_document_authority",
        "fndraining_action",
        "fnrevoked_settling_action",
        "self.phase==DocumentPhase::AuthorityRevoked&&self.settling_action(attempt)",
        "self.settling_action==Some(attempt)",
        "super::semantic_action::map_runtime_fault(*fault)==zephium_agentic::SemanticActionNativeFailure::AppliedUnverified",
        "DocumentPhase::AuthorityRevoked",
        "lethanded_off=self.awaiting_result&&matches!(self.pending,Some(PendingInvocation::Action{..}|PendingInvocation::Privacy{..}|PendingInvocation::Media{..}));",
        "fnretire",
        "fnadmission_failure(&mutself)->Option<AgentSemanticRuntimeDispatchError>",
        "DocumentPhase::Loading=>{returnSome(AgentSemanticRuntimeDispatchError::NotReady);}",
        "ifself.pending.is_some()||self.awaiting_result{returnSome(AgentSemanticRuntimeDispatchError::Busy);}",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "production macOS semantic runtime lost required closed mechanism {required}"
            ));
        }
    }
    for forbidden in [
        "evaluateJavaScript",
        "callAsyncJavaScript",
        "evaluate_script",
        "with_ipc_handler",
        "std::env",
        "PAGE_WORLD_COMPATIBILITY_FILL_PROGRAM",
        "WKContentWorld::pageWorld",
        "page_relay",
        "ZEPHIUM_PAGE_WORLD_FILL_RELAY_PROBE",
        "native-agentic-input-probe",
        "unreachable!",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "production macOS semantic runtime acquired forbidden surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_engine_platform_module(source: &str, platform: &str) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "#[cfg(feature=\"native-agentic-input-probe\")]modagentic_input_probe;",
        "#[cfg(feature=\"native-agentic-input-probe\")]pub(crate)useagentic_input_probe::runasrun_agentic_input_matrix;",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "{platform} agentic probe module escaped or drifted from its feature gate"
            ));
        }
    }
    Ok(())
}

fn validate_macos_rendering_presented_probe(
    source: &str,
    parent: &str,
    binary: &str,
) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "constPRESENTED_TIMEOUT:Duration=Duration::from_secs(5);",
        "constMAX_APPKIT_EVENTS:usize=32;",
        "actual==expected&&!active&&windows==0",
        "app.activationPolicy()!=NSApplicationActivationPolicy::Prohibited&&!app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited)",
        "app.activationPolicy()!=NSApplicationActivationPolicy::Accessory&&!app.setActivationPolicy(NSApplicationActivationPolicy::Accessory)",
        "app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited)",
        "app.finishLaunching();",
        "app.setActivationPolicy(NSApplicationActivationPolicy::Accessory)",
        "self.app.activationPolicy()==NSApplicationActivationPolicy::Accessory",
        "event_type==NSEventType::AppKitDefined&&dispatched<MAX_APPKIT_EVENTS",
        "NSEventMask::AppKitDefined,None,objc2_foundation::ns_string!(\"NSDefaultRunLoopMode\"),true,",
        "!appkit_event_permitted(event.r#type(),self.appkit_events_dispatched.get())",
        "self.app.sendEvent(&event);self.sample();",
        "operation.context()!=context",
        "document_finished_for_audit(operation)",
        "original.window.canBecomeKeyWindow()",
        "original.window.canBecomeMainWindow()",
        "!self.app.isActive()",
        "!self.window.isKeyWindow()&&!self.window.canBecomeKeyWindow()",
        "!self.window.isMainWindow()&&!self.window.canBecomeMainWindow()",
        "self.window.ignoresMouseEvents()",
        "self.first_responder",
        "responder_is_owned(responder_identity,self.first_responder,exact_page,)",
        "exact_page!=0&&(actual==original||actual==Some(exact_page))",
        "Some(\"presented_responder_changed\")=>Some(self.responder_refusal(responder))",
        "view.isDescendantOf(self.page)",
        "!self.page.isHiddenOrHasHiddenAncestor()",
        "self.window.alphaValue()==1.0",
        "self.page.alphaValue()==1.0",
        "std::ptr::eq(&*window,self.window)",
        "Some(\"presented_surface_geometry\")=>geometry_failure",
        "self.window.frame()!=self.expected_window_frame",
        "admit_actual_frame(scope.window.frame(),visible_screen)?",
        "actual.size!=NSSize::new(1280.0,800.0)",
        "right>max_x||top>max_y",
        "expected_window_frame:presented_frame",
        "self.page.frame()!=viewport_frame()",
        "scope.page.setFrame(viewport_frame())",
        "self.page.setFrame(self.original_page_frame)",
        "self.page.frame()!=self.original_page_frame",
        "NSWindowOcclusionState::Visible",
        "self.page.visibleRect()==viewport_frame()",
        "visible.size.width<1280.0||visible.size.height<800.0",
        "Instant::now()<self.deadline",
        "scope.window.setIgnoresMouseEvents(true)",
        "scope.window.setOpaque(true)",
        "scope.window.orderFrontRegardless()",
        "guard.require_visible_pixels.set(true)",
        "rendering::measure(view,context,operation,url,&presented_runtime)",
        "letrestore=scope.hide();",
        "restore.err().or_else(||runtime.failure_stage())",
        "MacosAgenticPresentedRenderingFailure::supersede(stage,Some(outcome),)",
        "self.page.setHidden(true)",
        "self.window.orderOut(None)",
        "implDropforPresentedScope",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "presented rendering lost isolated lease contract {required}"
            ));
        }
    }
    // The release-excluded module owns two fixed presentation cohorts: the
    // rendering witness and the AX-only witness. Each may present once; event
    // dispatch remains owned solely by the rendering witness.
    if source.matches(".orderFrontRegardless()").count() != 2
        || source.matches(".setHidden(false)").count() != 2
        || source
            .matches(".nextEventMatchingMask_untilDate_inMode_dequeue(")
            .count()
            != 1
        || source.matches(".sendEvent(").count() != 1
    {
        return Err("presented rendering acquired repeated presentation or event dispatch".into());
    }
    for forbidden in [
        "makeKey",
        "activateIgnoringOtherApps",
        "activateWithOptions",
        "setAlphaValue",
        "setInactiveSchedulingPolicy",
        "setValue_forKey",
        "evaluateJavaScript",
        "evaluate_script",
        "dispatch_semantic_action",
        "CGEvent",
        "NSEvent::",
        "NSEventMask::Any",
        "NSEventMask::Key",
        "NSEventMask::Left",
        "NSEventMask::Right",
        "NSEventMask::System",
        "NSEventMask::Application",
        "postEvent",
        "AXUIElement",
        "std::env",
        "std::net",
        "reqwest",
        "Keychain",
        "AgentBrowserSession",
        "std::thread",
        "setLevel",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "presented rendering acquired forbidden authority {forbidden}"
            ));
        }
    }
    let parent = compact(parent);
    for required in [
        "#[path=\"agentic_rendering_presented_probe.rs\"]modrendering_presented;",
        "begin(ProbeMode::RenderingPresented(&mutreport))",
        "matchfinish(pending)",
        "Err(stage)=>Err(MacosAgenticPresentedRenderingFailure::supersede(stage,report,))",
        "**report=Some(outcome);returnrefusal.map_or(Ok(None),Err);",
        "letnative_guard=ProbeNativeState::Hidden(NativeStateGuard{",
        "Self::Hidden(guard)=>guard.sample()",
        "ifletProbeNativeState::Presented(guard)=self.native_guard{guard.pump_appkit_event();}",
        "ifmatches!(&mode,ProbeMode::RenderingPresented(_))||ax_fill_probe||responder_case.is_some()||trusted_case.is_some()||surface_case.is_some(){rendering_presented::initialize_inactive(&app)?;}",
        "Self::Presented(guard)=>guard.sample()",
        "rendering_presented::measure(&view,context,operation,&url,&runtime,&host.view)",
    ] {
        if !parent.contains(required) {
            return Err(format!(
                "presented rendering escaped original cohort {required}"
            ));
        }
    }
    if !compact(binary).contains("arguments.as_slice()==[\"--ci-presented-rendering-readiness\"]")
        || !binary.contains("provisional_measurement_is_not_qualification=true")
    {
        return Err("presented rendering lost closed CLI".into());
    }
    Ok(())
}

fn validate_macos_rendering_opportunity_probe(
    source: &str,
    parent: &str,
    binary: &str,
) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "constSNAPSHOT_OPPORTUNITY_TIMEOUT:Duration=Duration::from_secs(1);",
        "original!=WKInactiveSchedulingPolicy::Throttle",
        "scope.policy()?!=WKInactiveSchedulingPolicy::None",
        "self.policy()?!=WKInactiveSchedulingPolicy::Throttle",
        "implDropforSchedulingScope",
        "scope.restore()?",
        "snapshot.attest()?",
        "NSSize::new(1.0,1.0)",
        "setSnapshotWidth(Some(&NSNumber::new_f64(1.0)))",
        "setAfterScreenUpdates(true)",
        "Instant::now()>=deadline",
        "operation.context()!=context",
        "document_finished_for_audit(operation)",
        "rendering::measure(view,context,operation,url,runtime)",
        "runtime.native_guard.sample()",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "rendering opportunity lost closed contract {required}"
            ));
        }
    }
    if source.matches(".setInactiveSchedulingPolicy(").count() != 2
        || source
            .matches(".takeSnapshotWithConfiguration_completionHandler(")
            .count()
            != 1
    {
        return Err("rendering opportunity acquired extra native dispatch".into());
    }
    for forbidden in [
        "setHidden",
        "setVisible",
        "setAlphaValue",
        "orderFront",
        "orderWindow",
        "setIgnoresMouseEvents",
        "evaluateJavaScript",
        "evaluate_script",
        "setValue_forKey",
        "dispatch_semantic_action",
        "setBackgroundThrottling",
        "std::env",
        "std::net",
        "reqwest",
        "Keychain",
        "AgentBrowserSession",
        "std::thread",
        "png",
        "CGImage",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "rendering opportunity acquired forbidden authority {forbidden}"
            ));
        }
    }
    let parent = compact(parent);
    for required in [
        "#[path=\"agentic_rendering_opportunity_probe.rs\"]modrendering_opportunity;",
        "begin(ProbeMode::RenderingOpportunity{opportunity,report:&mutreport,})",
        "iffinish(pending)?.is_some()",
        "report.ok_or(\"rendering_opportunity_report_missing\")",
        "rendering_opportunity::measure(&view,context,operation,&url,&runtime,*opportunity,)",
    ] {
        if !parent.contains(required) {
            return Err(format!(
                "rendering opportunity escaped original ownership {required}"
            ));
        }
    }
    let binary = compact(binary);
    for required in [
        "arguments.as_slice()==[\"--ci-hidden-rendering-unthrottled\"]",
        "arguments.as_slice()==[\"--ci-hidden-rendering-snapshot\"]",
        "run_macos_agentic_rendering_opportunity_probe(opportunity)",
    ] {
        if !binary.contains(required) {
            return Err("rendering opportunity lost closed CLI".into());
        }
    }
    Ok(())
}

fn validate_macos_rendering_probe(
    source: &str,
    parent: &str,
    binary: &str,
    fixture: &str,
) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "constSAMPLE_OFFSETS_MS:[u64;8]=[0,50,100,200,400,800,1_600,3_200];",
        "constMEASUREMENT_TIMEOUT:Duration=Duration::from_secs(5);",
        "operation.context()!=context",
        "document_finished_for_audit(operation)",
        "dispatch_invocation(view,invocation,runtime,deadline)",
        "snapshot.frame().context()!=context",
        "snapshot.frame().origin()!=origin",
        "snapshot.completeness()!=SemanticCompleteness::Complete",
        "SemanticRuntimeFault::DocumentLoading",
        "AnimationFrameNotObservedWithinWindow",
        "ControlsIncomplete",
        "sample.elapsed_ms>=SAMPLE_OFFSETS_MS[7]",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "rendering diagnostic lost bounded evidence contract {required}"
            ));
        }
    }
    for forbidden in [
        "unsafe{",
        "evaluate_script",
        "evaluateJavaScript",
        "dispatch_semantic_action",
        "setHidden",
        "setVisible",
        "setInactiveSchedulingPolicy",
        "BackgroundThrottlingPolicy",
        "std::env",
        "std::net",
        "reqwest",
        "Keychain",
        "AgentBrowserSession",
        "std::thread::spawn",
        "setInterval",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "rendering diagnostic acquired forbidden authority {forbidden}"
            ));
        }
    }
    let parent = compact(parent);
    for required in [
        "#[path=\"agentic_rendering_probe.rs\"]modrendering;",
        "begin(ProbeMode::Rendering(&mutreport))",
        "iffinish(pending)?.is_some()",
        "report.ok_or(\"rendering_report_missing\")",
        "ifrendering_probe{&[ContextCapability::Observe,ContextCapability::Navigate]}",
        "server.url(FixtureRoute::SemanticRendering)",
        "rendering::measure(&view,context,operation,&url,&runtime,)",
    ] {
        if !parent.contains(required) {
            return Err(format!(
                "rendering diagnostic escaped original ownership {required}"
            ));
        }
    }
    let binary = compact(binary);
    if !binary.contains("arguments.as_slice()==[\"--ci-hidden-rendering-readiness\"]")
        || !binary.contains("run_macos_agentic_rendering_probe()")
    {
        return Err("rendering diagnostic lost its closed provider-free CLI".into());
    }
    let fixture = fixture
        .split("const SEMANTIC_RENDERING_HTML:")
        .nth(1)
        .and_then(|source| source.split("const SEMANTIC_RUNTIME_HTML:").next())
        .ok_or("rendering fixture boundary missing")?;
    let fixture = fixture.replace("\r\n", "\n");
    // This exact, finite opt-in is the only interval admitted in the shared
    // fixture. Preserve the default rendering qualifier's interval-free page.
    const RETAINED_PROGRESS_BRANCH: &str = r#"  if (window.location.hash === '#windows-retained-progress') {
    const progress = document.createElement('h3');
    let ticks = 0;
    progress.textContent = 'Retained native progress 0';
    document.body.append(progress);
    const timer = setInterval(() => {
      ticks += 1;
      progress.textContent = 'Retained native progress ' + ticks;
      if (ticks === 4096) clearInterval(timer);
    }, 500);
  }
"#;
    if fixture.matches(RETAINED_PROGRESS_BRANCH).count() != 1 {
        return Err("rendering fixture lost its exact bounded retained-progress opt-in".into());
    }
    let (before_progress, after_progress) = fixture
        .split_once(RETAINED_PROGRESS_BRANCH)
        .ok_or("rendering fixture retained-progress boundary missing")?;
    let default_rendering = format!("{before_progress}{after_progress}");
    for forbidden in [
        "setInterval(",
        "clearInterval(",
        "Retained native progress",
        "windows-retained-progress",
        "window.location.hash",
    ] {
        if default_rendering.contains(forbidden) {
            return Err(format!(
                "rendering fixture escaped its bounded progress opt-in {forbidden}"
            ));
        }
    }
    for required in [
        "Promise.resolve().then(",
        "setTimeout(",
        "requestAnimationFrame(",
        "window.addEventListener('load'",
        "document.addEventListener('readystatechange'",
        "id=\"animation\" hidden",
    ] {
        if !default_rendering.contains(required) {
            return Err(format!(
                "rendering diagnostic lost independent fixture control {required}"
            ));
        }
    }
    for forbidden in ["fetch(", "src=", "https://", "messageHandlers", "focus("] {
        if fixture.contains(forbidden) {
            return Err(format!(
                "rendering fixture acquired external work {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_macos_semantic_probe(
    module: &str,
    source: &str,
    binary: &str,
    fixture: &str,
) -> Result<(), String> {
    let module = compact(module);
    for required in [
        "#[cfg(feature=\"native-agentic-semantic-probe\")]modagentic_semantic_probe;",
        "#[cfg(feature=\"native-agentic-semantic-probe\")]pub(crate)useagentic_semantic_probe::runasrun_agentic_semantic_probe;",
        "pub(crate)useagentic_semantic_probe::run_model_public_fillasrun_agentic_semantic_model_public_fill_probe;",
        "pub(crate)useagentic_semantic_probe::run_model_two_actionasrun_agentic_semantic_model_two_action_probe;",
        "#[cfg(feature=\"native-agentic-semantic-probe\")]pub(crate)useagentic_semantic_probe::run_model_workflowasrun_agentic_semantic_model_workflow_probe;",
    ] {
        if !module.contains(required) {
            return Err(format!(
                "macOS semantic probe escaped or drifted from required gate {required}"
            ));
        }
    }

    let source = compact(source);
    for required in [
        "FixtureRoute::SemanticRuntime",
        "FixtureRoute::SemanticRuntimeMutation",
        "FixtureRoute::SemanticRuntimeReplacement",
        "FixtureServer::start()",
        "new_ephemeral_data_store()",
        "ContextProfileStorageClass::Ephemeral",
        "ContextOwnedViewport::STANDARD",
        "build_owned_agent_view(",
        "NativeContentPolicy::AllowAll",
        "NSApplicationActivationPolicy::Accessory",
        "NSWindowStyleMask::Borderless",
        "window.orderOut(None)",
        "!self.page.isHidden()",
        "self.app.isActive()",
        "returnErr(\"focus_baseline\")",
        "view.prepare_semantic_document_load()",
        "view.dispatch_semantic(",
        "ContextCapability::Act",
        "verify_primary_click_execution(",
        "verify_hostile_fill_refusal(",
        "SemanticActionNativeFailure::AppliedUnverified",
        ".settle_and_verify(pending.settlement,snapshot,observed_at)",
        "SemanticObservationAssembler::new(",
        "SemanticFrameUnsupported::PlatformIsolationUnavailable",
        "SemanticRuntimeFault::AnchorMissing",
        "wait_for_mutation_gate(",
        "server.release_semantic_mutation()",
        "wait_for_mutation_application(",
        "verify_mutation_before(",
        "verify_mutation_after(",
        "server.semantic_mutation_completed()",
        "structNativeStateGuard",
        "pump_once(self.run_loop,Some(self.native_guard))",
        "view.attest(",
        "SemanticRuntimeFault::DocumentLoading",
        "MAX_DOCUMENT_LOADING_RETRIES",
        "view.retire_semantic_runtime()",
        "Weak::from_retained(&page)",
        "server.shutdown()",
        "constPUBLIC_DISCOVERY_PROBE_URL:&str=\"https://www.wikipedia.org/\";",
        "ProbeMode::ModelPublicFill",
        "MacosAgenticSemanticTwoActionScenario::PublicFillSelect",
        "SemanticActionKind::Select",
        "url==PUBLIC_DISCOVERY_PROBE_URL",
        "allow_hidden_responder_change:public_fill_probe",
        "ProbeMode::ModelWorkflow{initial,next}",
        "for_in0..8{",
        ".acknowledge_observation(identity.id(),first)",
        "generation=capture.snapshot.generation()",
        "None=>returnOk(None)",
        "letexecution=execution.and_then(|terminal|",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "macOS semantic probe lost required production-path mechanism {required}"
            ));
        }
    }
    // Admits one evaluation (d006e649): the history witness arms and reads a
    // pageshow counter through two fixed scripts; no page script is chosen at runtime.
    const HISTORY_EVALUATION: &str =
        "unsafe{page.evaluateJavaScript_completionHandler(&source,Some(&completion))};";
    let scanned = if source.matches("evaluate_history_probe(").count() == 3
        && source
            .matches("evaluate_history_probe(page,\"(()=>{")
            .count()
            == 2
        && source.matches(HISTORY_EVALUATION).count() == 1
    {
        source.replacen(HISTORY_EVALUATION, "", 1)
    } else {
        source.clone()
    };
    // Admits one activation (fd2d43d4): the Work qualifier host brings its own
    // window forward only when it runs the shipping foreground presentation.
    const FOREGROUND_HOST: &str = "app.activate();#[allow(deprecated)]app.activateIgnoringOtherApps(true);window.makeKeyAndOrderFront(None);window.makeMainWindow();";
    let scanned = match scanned.split_once("fnrun_work_host(") {
        Some((head, host))
            if host.contains("letwindow=ifforeground{")
                && scanned.matches(FOREGROUND_HOST).count() == 1 =>
        {
            format!(
                "{head}fnrun_work_host({}",
                host.replacen(FOREGROUND_HOST, "", 1)
            )
        }
        _ => scanned,
    };
    for forbidden in [
        "evaluateJavaScript",
        "callAsyncJavaScript",
        "evaluate_script",
        "with_ipc_handler",
        "CGEvent",
        "AXUIElement",
        "accessibilityPerformPress",
        "NSEvent::",
        "orderFront",
        "makeKeyAndOrderFront",
        "activateIgnoringOtherApps",
        "activateWithOptions",
    ] {
        if scanned.contains(forbidden) {
            return Err(format!(
                "macOS semantic probe acquired forbidden authority {forbidden}"
            ));
        }
    }

    let binary = compact(binary);
    for required in [
        "arguments.as_slice()!=[\"--ci-hidden-fixed-dom\"]",
        "run_macos_agentic_semantic_probe()",
        "profile=ephemeral",
        "viewport=1280x800-logical",
        "fixture=loopback-only",
        "snapshots=18",
        "snapshots=22",
        "world_epochs=4",
        "fixed_click=verified",
        "click_postcondition=expanded",
        "isolated_fixed_fill=verified",
        "controls=text-input,search-input,textarea,contenteditable,nested-leaf",
        "nested_delegated_model=retained",
        "nested_context_attacks=6-refused-nonretryable",
        "fill_postcondition=exact-value",
        "event_trust=untrusted",
        "user_activation=0",
        "popup_admitted=0",
        "mutation_gate=host-released",
        "stale_anchor=refused",
        "mutation_recovery=verified",
        "epoch_rotation_fill=verified",
        "focus_theft=0",
        "retained_views=0",
        "hostile_terminal_spoof=ignored",
        "hostile_cross_node_terminal=refused",
        "hostile_reparent_type_repurpose=applied-unverified",
        "hostile_reparent_recovery=verified",
        "hostile_credential_relabel=applied-unverified",
        "hostile_credential_recovery=verified",
    ] {
        if !binary.contains(required) {
            return Err(format!(
                "macOS semantic probe binary lost required closed output {required}"
            ));
        }
    }

    let fixture = compact(fixture);
    for required in [
        "TcpListener::bind((Ipv4Addr::LOCALHOST,0))",
        "if!address.ip().is_loopback()",
        "format!(\"http://127.0.0.1:{}{}\"",
        "Self::SemanticRuntime=>\"/semantic-runtime-v1.html\"",
        "Self::SemanticRuntimeRelayHostile=>\"/semantic-runtime-relay-hostile-v1.html\"",
        "Self::SemanticRuntimeMutation=>\"/semantic-runtime-mutation-v1.html\"",
        "Self::SemanticRuntimeReplacement=>\"/semantic-runtime-replacement-v1.html\"",
        "constSEMANTIC_MUTATION_GATE_TIMEOUT:Duration=Duration::from_secs(15);",
        "constSEMANTIC_MUTATION_TRIGGER_PATH:&str=\"/semantic-runtime-mutation-trigger-v1.js\";",
        "structSemanticGate",
        "wake:Condvar",
        "ifstate.waiting||state.released||state.completed",
        "if!state.waiting||state.released||state.completed",
        "semantic_mutation.wait_for_release(stop,SEMANTIC_MUTATION_GATE_TIMEOUT)",
        "semantic_mutation.mark_completed()",
        "application/javascript;charset=utf-8",
        "<scriptdefersrc=\"/semantic-runtime-mutation-trigger-v1.js\"></script>",
        "FixtureScriptPolicy::SameOrigin",
        "connect-src'none'",
        "form-action'none'",
        "frame-src'self'",
        // 697c56d0: the primary action is a native disclosure, expanded by the page.
        "<details><summaryid=\"primary-semantic-action\"><span>Primarysemanticaction</span></summary><p>Nativedisclosurecontent</p></details>",
        "Semantichostilefill",
        "Semantichostilerecovery",
        "Semantichostilecredentialfill",
        "semantic-hostile-credential-label",
        "credential.setAttribute('aria-labelledby','semantic-hostile-credential-label')",
        "credential.removeAttribute('aria-labelledby')",
        "main.insertBefore(hostile,hostileNextSibling)",
    ] {
        if !fixture.contains(required) {
            return Err(format!(
                "macOS semantic probe fixture lost loopback confinement {required}"
            ));
        }
    }
    Ok(())
}

fn validate_windows_probe_binary(source: &str, qualification: &str) -> Result<(), String> {
    validate_agentic_no_invariant_panic_attribute(ENGINE_WINDOWS_PROBE_BINARY, source)?;
    let source = compact(source);
    for required in [
        "--allow-visible-focused",
        "--evidence-directory",
        "eval/agentic-browsing/local-results",
        "WindowsProbeMode::from_argument",
        "mode.local_result_filename()",
        "evidence_metadata_is_direct_directory(&metadata)",
        "NamedTempFile::new_in(directory)",
        "pending.file.as_file().sync_all()",
        "persist_noclobber(pending.destination)",
        "qualify_windows_probe_evidence(mode,&evidence)",
        "encode_response_line(&response)",
        "ProbeReply::RunCompleted(evidence)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "Windows physical qualification runner lost required closed gate {required}"
            ));
        }
    }
    let qualification = compact(qualification);
    for required in [
        "--ci-hidden-fixed-dom",
        "--ci-hidden-hwnd",
        "--ci-hidden-cdp",
        "--visible-background-windows-all",
        "--visible-focused-windows-all",
        "evidence.runtime.adapter_revision.as_str()!=\"native-input-m1-resources-v4\"",
        "focus_sequence_matches_presentation(actual.focus,mode.presentation())",
        "focus.before==baseline&&focus.during==baseline&&focus.after==after",
        "actual.focus.target_received_dom_focus!=target_focus_event_observed(actual)",
        "if!no_dispatch_evidence_is_empty(actual)",
        "elseif!actual.target.target_verified||!has_qualifying_event(actual)||(backend_requires_trusted_qualifying_event(backend)&&!has_trusted_qualifying_event(actual))",
        "fnhas_qualifying_event(evidence:&CaseEvidence)->bool{letrequired=qualifying_event_kind(evidence.case);evidence.events.iter().any(|event|event.kind==required&&event.target==evidence.target.intended)}",
        "fnhas_trusted_qualifying_event(evidence:&CaseEvidence)->bool{letrequired=qualifying_event_kind(evidence.case);evidence.events.iter().any(|event|{event.is_trusted&&event.kind==required&&event.target==evidence.target.intended})}",
        "fntarget_focus_event_observed(evidence:&CaseEvidence)->bool{evidence.events.iter().any(|event|{event.kind==InputEventKind::Focus&&event.target==evidence.target.intended})}",
        "constfnbackend_requires_trusted_qualifying_event(backend:InputBackend)->bool{matches!(backend,InputBackend::WindowsHwndInput|InputBackend::WindowsCdpInput)}",
        "actual.resources_before.native_views!=1",
        "actual.resources_after.native_views!=1",
        "actual.resources_before.queued_actions!=0",
        "actual.resources_after.queued_actions!=0",
        "actual.resources_before.helper_processes.is_none_or(|count|count==0)",
        "actual.resources_after.helper_processes.is_none_or(|count|count==0)",
        "actual.resources_before.resident_bytes.is_none_or(|bytes|bytes==0)",
        "actual.resources_after.resident_bytes.is_none_or(|bytes|bytes==0)",
        "maximum_helper_processes",
        "maximum_resident_bytes",
        "fnno_dispatch_evidence_is_empty(evidence:&CaseEvidence)->bool",
        "actual.target.navigation_observed!=(case==FixtureCase::Link&&!does_not_dispatch)",
        "GateOutcome::Denied|GateOutcome::Indeterminate",
        "ifactual.target.popup_requested{actual.outcome==CaseOutcome::Verified}else{actual.outcome==CaseOutcome::Unsupported}",
    ] {
        if !qualification.contains(required) {
            return Err(format!(
                "Windows physical qualification modes lost required closed gate {required}"
            ));
        }
    }
    Ok(())
}

fn validate_windows_probe_ci(source: &str) -> Result<(), String> {
    let source = compact(source).replace('\\', "");
    for required in [
        "name:ParsetheWindowsagenticqualificationworkflow",
        "if:runner.os=='Windows'",
        "shell:pwsh",
        "$scriptPath=Join-Path$env:GITHUB_WORKSPACE\"scriptsqualificationwindows-agentic.ps1\"",
        "[void][System.Management.Automation.Language.Parser]::ParseFile(",
        "[ref]$tokens,[ref]$parseErrors",
        "if($parseErrors.Count-ne0)",
        "throw\"physicalWindowsagenticorchestratordidnotparse\"",
        "cargoclippy--locked-pzephium-engine--featuresnative-agentic-input-probe--binwindows-agentic-input-probe",
        "cargobuild--locked-pzephium-engine--featuresnative-agentic-input-probe--binwindows-agentic-input-probe",
        "cargoclippy--locked-pzephium-engine--featuresnative-agentic-semantic-probe--binwindows-agentic-semantic-probe",
        "cargobuild--locked-pzephium-engine--featuresnative-agentic-semantic-probe--binwindows-agentic-semantic-probe",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "Windows native-input CI lost required native compile gate {required}"
            ));
        }
    }
    Ok(())
}

fn validate_windows_physical_workflow(source: &str) -> Result<(), String> {
    let compact = compact(source);
    for required in [
        "[ValidateSet(\"CollectNonDebugger\",\"ReviewAfterDebugger\")]",
        "[switch]$AuthorizedPhysicalWindows",
        "if(-not$AuthorizedPhysicalWindows.IsPresent)",
        "&cargo--offline@Arguments",
        "[System.Environment]::OSVersion.Platform-ne[System.PlatformID]::Win32NT",
        "[System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture-ne[System.Runtime.InteropServices.Architecture]::X64",
        "$rustcVersion=@(&rustc-vV)",
        "$rustcVersion-ccontains\"host:x86_64-pc-windows-msvc\"",
        "$metadataLines=@(&cargo--offlinemetadata--locked--format-version1--no-deps--manifest-path(Join-Path$Repository\"Cargo.toml\"))",
        "[System.IO.Path]::GetFullPath([string]$metadata.target_directory)",
        "status--porcelain=v1--untracked-files=all",
        "rev-parse--verifyHEAD",
        "if($revision-cnotmatch'^[0-9a-f]{40}$')",
        "$sourceStampName=\"windows-qualification-source-v1.txt\"",
        "$debuggerBinaryHashStampName=\"windows-semantic-debugger-binary-sha256-v1.txt\"",
        "Get-FileHash-LiteralPath$Path-AlgorithmSHA256",
        "if($hash-notmatch'^[0-9a-fA-F]{64}$')",
        "if($Hash-cnotmatch'^[0-9a-f]{64}$')",
        "[System.IO.FileMode]::CreateNew",
        "$stream.Flush($true)",
        "Assert-DirectDirectory-Path$evidenceDirectory-Create$true",
        "Assert-ExactDirectoryEntries-Directory$evidenceDirectory-ExpectedNames@()",
        "Assert-ExactDirectoryEntries-Directory$evidenceDirectory-ExpectedNames$expectedBeforeDebugger",
        "if($recordedRevision-cne$sourceRevision)",
        "Assert-ExactDirectoryEntries-Directory$evidenceDirectory-ExpectedNames$expectedBeforeReview",
        "Assert-ExactDirectoryEntries-Directory$evidenceDirectory-ExpectedNames$expectedAfterReview",
        "Invoke-Cargo-Arguments@(\"xtask\",\"check-agentic-probe-boundary\")",
        "Invoke-Cargo-Arguments@(\"test\",\"--locked\",\"-p\",\"zephium-agentic\",\"--features\",\"probe-harness\")",
        "Invoke-Cargo-Arguments@(\"test\",\"--locked\",\"-p\",\"zephium-engine\",\"--features\",\"agentic-browser\",\"--lib\")",
        "Invoke-Cargo-Arguments@(\"check\",\"--locked\",\"-p\",\"zephium-engine\",\"--features\",\"agentic-browser\")",
        "Invoke-Cargo-Arguments@(\"clippy\",\"--locked\",\"-p\",\"zephium-engine\",\"--features\",\"native-agentic-input-probe\",\"--bin\",\"windows-agentic-input-probe\")",
        "Invoke-Cargo-Arguments@(\"build\",\"--locked\",\"-p\",\"zephium-engine\",\"--features\",\"native-agentic-input-probe\",\"--bin\",\"windows-agentic-input-probe\")",
        "Invoke-Cargo-Arguments@(\"clippy\",\"--locked\",\"-p\",\"zephium-engine\",\"--features\",\"native-agentic-semantic-probe\",\"--bin\",\"windows-agentic-semantic-probe\")",
        "Invoke-Cargo-Arguments@(\"build\",\"--locked\",\"-p\",\"zephium-engine\",\"--features\",\"native-agentic-semantic-probe\",\"--bin\",\"windows-agentic-semantic-probe\")",
        "$preflightRevision=Get-SourceRevision-Repository$repository",
        "if($preflightRevision-cne$sourceRevision)",
        "$semanticProbeBinary=Join-Path$cargoTargetDirectory\"debug\\windows-agentic-semantic-probe.exe\"",
        "Write-BinaryHashStamp-Path(Join-Path$evidenceDirectory$debuggerBinaryHashStampName)-Hash$semanticProbeBinaryHash",
        "if($semanticProbeBinaryHash-cne$preflightSemanticProbeBinaryHash)",
        "if($recordedSourceStamp-cnotmatch'^[0-9a-f]{40}\\n$')",
        "$recordedRevision=$recordedSourceStamp.Substring(0,40)",
        "if($recordedDebuggerBinaryHashStamp-cnotmatch'^[0-9a-f]{64}\\n$')",
        "$recordedDebuggerBinaryHash=$recordedDebuggerBinaryHashStamp.Substring(0,64)",
        "if($currentDebuggerBinaryHash-cne$recordedDebuggerBinaryHash)",
        "Write-Host$semanticProbeBinary",
        "\"--ci-hidden-debugger-coexistence--evidence-directoryeval/agentic-browsing/local-results\"",
    ] {
        if !compact.contains(required) {
            return Err(format!(
                "physical Windows qualification workflow lost fail-closed requirement {required}"
            ));
        }
    }

    for exact_once in [
        "--ci-hidden-fixed-dom",
        "--ci-hidden-hwnd",
        "--ci-hidden-cdp",
        "--visible-background-windows-all",
        "--ci-hidden-fixed-documents",
        "--ci-hidden-redirect-lifecycle",
        "--ci-hidden-location-replacement",
        "--ci-hidden-suspend-resume",
        "--ci-hidden-event-flood",
        "--ci-hidden-renderer-loss",
        "--ci-hidden-debugger-coexistence",
        "windows-hidden-fixed-dom.jsonl",
        "windows-hidden-hwnd.jsonl",
        "windows-hidden-cdp.jsonl",
        "windows-visible-background-all.jsonl",
        "windows-semantic-fixed-documents.jsonl",
        "windows-semantic-redirect-lifecycle.jsonl",
        "windows-semantic-location-replacement.jsonl",
        "windows-semantic-suspend-resume.jsonl",
        "windows-semantic-event-flood.jsonl",
        "windows-semantic-renderer-loss.jsonl",
        "windows-semantic-debugger-coexistence.jsonl",
        "windows-review-summary-v2.json",
        "windows-semantic-review-summary-v5.json",
        "windows-semantic-debugger-binary-sha256-v1.txt",
    ] {
        if source.matches(exact_once).count() != 1 {
            return Err(format!(
                "physical Windows qualification workflow must retain exactly one {exact_once}"
            ));
        }
    }

    if source.matches("Invoke-Cargo -Arguments").count() != 21 {
        return Err(
            "physical Windows qualification workflow changed its exact command inventory"
                .to_owned(),
        );
    }
    if source.matches("[System.IO.FileMode]::CreateNew").count() != 2 {
        return Err("physical Windows qualification stamps must both be create-new".to_owned());
    }
    if source
        .matches("Get-CargoTargetDirectory -Repository $repository")
        .count()
        != 2
        || source
            .matches("Get-FileSha256 -Path $semanticProbeBinary")
            .count()
            != 3
        || source
            .matches("Assert-DirectDirectory -Path $cargoTargetDirectory -Create $false")
            .count()
            != 2
        || source
            .matches(
                "$semanticProbeBinary = Join-Path $cargoTargetDirectory \"debug\\windows-agentic-semantic-probe.exe\"",
            )
            .count()
            != 2
    {
        return Err(
            "physical Windows qualification must rejoin the exact debugger binary during collection and review"
                .to_owned(),
        );
    }
    if source
        .matches("Assert-CleanCheckout -Repository $repository")
        .count()
        != 2
    {
        return Err(
            "physical Windows qualification must recheck source cleanliness after preflight"
                .to_owned(),
        );
    }
    if source
        .matches("Assert-ExactDirectoryEntries -Directory $evidenceDirectory -ExpectedNames @()")
        .count()
        != 2
    {
        return Err(
            "physical Windows qualification must recheck the empty evidence directory after preflight"
                .to_owned(),
        );
    }
    let preflight = compact
        .find("Invoke-Cargo-Arguments@(\"xtask\",\"check-agentic-probe-boundary\")")
        .ok_or_else(|| "physical Windows qualification preflight is missing".to_owned())?;
    let clean_recheck = compact
        .rfind("Assert-CleanCheckout-Repository$repository")
        .ok_or_else(|| "physical Windows qualification clean recheck is missing".to_owned())?;
    let preflight_binary_hash = compact
        .find("$preflightSemanticProbeBinaryHash=Get-FileSha256-Path$semanticProbeBinary")
        .ok_or_else(|| {
            "physical Windows qualification debugger binary preflight is missing".to_owned()
        })?;
    let stamp = compact
        .find("Write-SourceStamp-Path(Join-Path$evidenceDirectory$sourceStampName)")
        .ok_or_else(|| "physical Windows qualification source stamp is missing".to_owned())?;
    let first_native_run = compact
        .find("Invoke-Cargo-Arguments@(\"run\",\"--locked\",\"-p\",\"zephium-engine\",\"--features\",\"native-agentic-input-probe\",\"--bin\",\"windows-agentic-input-probe\",\"--\",\"--ci-hidden-fixed-dom\"")
        .ok_or_else(|| "physical Windows qualification native run is missing".to_owned())?;
    if !(preflight < preflight_binary_hash
        && preflight_binary_hash < clean_recheck
        && clean_recheck < stamp
        && stamp < first_native_run)
    {
        return Err(
            "physical Windows qualification must preflight, recheck source, stamp, then run"
                .to_owned(),
        );
    }
    let final_debugger_build = compact
        .rfind("Invoke-Cargo-Arguments@(\"build\",\"--locked\",\"-p\",\"zephium-engine\",\"--features\",\"native-agentic-semantic-probe\",\"--bin\",\"windows-agentic-semantic-probe\")")
        .ok_or_else(|| "physical Windows qualification debugger build is missing".to_owned())?;
    let binary_hash_stamp = compact
        .find("Write-BinaryHashStamp-Path(Join-Path$evidenceDirectory$debuggerBinaryHashStampName)-Hash$semanticProbeBinaryHash")
        .ok_or_else(|| "physical Windows qualification debugger binary stamp is missing".to_owned())?;
    let collection_binary_hash_rejoin = compact
        .find("if($semanticProbeBinaryHash-cne$preflightSemanticProbeBinaryHash)")
        .ok_or_else(|| {
            "physical Windows qualification collection binary hash rejoin is missing".to_owned()
        })?;
    let debugger_instruction = compact
        .find(
            "Write-Host\"Launchthisexactsource-boundbinaryundertheseparatelyauthorizeddebugger:\"",
        )
        .ok_or_else(|| {
            "physical Windows qualification debugger instruction is missing".to_owned()
        })?;
    if !(final_debugger_build < collection_binary_hash_rejoin
        && collection_binary_hash_rejoin < binary_hash_stamp
        && binary_hash_stamp < debugger_instruction)
    {
        return Err(
            "physical Windows qualification must build, hash, then hand off the debugger binary"
                .to_owned(),
        );
    }
    let recorded_binary_hash = compact
        .find("$recordedDebuggerBinaryHashStamp=[System.IO.File]::ReadAllText($debuggerBinaryHashStamp)")
        .ok_or_else(|| "physical Windows qualification recorded binary hash is missing".to_owned())?;
    let binary_hash_rejoin = compact
        .find("if($currentDebuggerBinaryHash-cne$recordedDebuggerBinaryHash)")
        .ok_or_else(|| "physical Windows qualification binary hash rejoin is missing".to_owned())?;
    let semantic_review = compact
        .rfind("Invoke-Cargo-Arguments@(\"run\",\"--locked\",\"-p\",\"zephium-agentic\",\"--features\",\"probe-harness\",\"--bin\",\"windows-agentic-semantic-evidence-review\"")
        .ok_or_else(|| "physical Windows qualification semantic reviewer is missing".to_owned())?;
    if !(recorded_binary_hash < binary_hash_rejoin && binary_hash_rejoin < semantic_review) {
        return Err(
            "physical Windows qualification must rejoin the debugger binary before review"
                .to_owned(),
        );
    }
    for forbidden in [
        "--visible-focused-windows-all",
        "--allow-visible-focused",
        "Remove-Item",
        "Clear-Content",
        "Set-Content",
        "Out-File",
        "System.IO.FileMode]::OpenOrCreate",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "physical Windows qualification workflow acquired forbidden behavior {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_macos_probe_source(source: &str) -> Result<(), String> {
    validate_engine_agentic_native_unsafe_contract(ENGINE_MACOS_PROBE_MODULE, source)?;
    let source = compact(source);
    for required in [
        "letpartition=Partition::Ephemeral(ProfileId::generate());",
        "super::new_ephemeral_data_store()",
        ".with_incognito(true).with_visible(false).with_webview_configuration(configuration)",
        "ifunsafe{configuration.webExtensionController()}.is_some()",
        "letvalid=!page_store.isPersistent()&&page_store.identifier().is_none()&&page_configuration.webExtensionController().is_none();",
        "WKContentWorld::worldWithName(&world_name,mtm)",
        "controller.addScriptMessageHandler_contentWorld_name(protocol_handler,&world,&handler_name,);",
        "WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(",
        "letmutcontrol=NativeDispatchControl{permit,poll_control,deadline,};control.check()?;",
        "(self.poll_control)();ifself.permit.is_cancelled(){returnErr(AdapterError::Cancelled);}ifInstant::now()>=self.deadline{returnErr(AdapterError::Timeout);}",
        "admission_control.check().map_err(|error|adapter_failure(error,ProbeStage::Admit,None,None))?;",
        "control.check()?;webview.load_url(&url)",
        "control.check()?;webview.set_visible(false)",
        "control.check()?;window.orderOut(None);",
        "control.check()?;webview.set_visible(true)",
        "control.check()?;window.orderFront(None);",
        "control.check()?;app.activate();",
        "control.check()?;app.activateIgnoringOtherApps(true);",
        "control.check()?;if!window.makeFirstResponder(Some(page))",
        "control.check()?;window.makeKeyAndOrderFront(None);",
        "control.check()?;letSome(element)=page.accessibilityHitTest(screen)else",
        "control.check()?;letpressed:bool={",
        "ifRetained::as_ptr(&message_page).cast::<c_void>()!=Retained::as_ptr(&expected_page).cast::<c_void>()",
        "if!unsafe{frame.isMainFrame()}",
        "ifexpected_url.as_deref()!=Some(frame_url)",
        "ifbody.length()>MAX_RUNTIME_MESSAGE_UTF16",
        "ifbody.len()>MAX_RUNTIME_MESSAGE_UTF8",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "macOS native-input probe lost required closed mechanism {required}"
            ));
        }
    }
    if source.matches("window.sendEvent(&event);").count() != 2
        || source
            .matches("control.check()?;window.sendEvent(&event);")
            .count()
            != 2
        || source
            .matches("window.makeFirstResponder(Some(page))")
            .count()
            != 2
        // c8b75eba: the presentation responder may be owned native chrome, still preflighted.
        || source
            .matches("control.check()?;if!window.makeFirstResponder(Some(page))")
            .count()
            != 1
        || source
            .matches("control.check()?;if!matchowned_surface{Some(surface)=>surface.focus_chrome(),None=>window.makeFirstResponder(Some(page)),}")
            .count()
            != 1
        || source.matches("page.accessibilityHitTest(screen)").count() != 1
        || source.matches("accessibilityPerformPress").count() != 2
    {
        return Err(
            "macOS native-input probe must preflight every closed native dispatch exactly once"
                .to_owned(),
        );
    }
    for forbidden in [
        "CGEvent",
        "CGEventPost",
        "CGWarpMouseCursorPosition",
        "AXIsProcessTrusted",
        "AXUIElementCreateSystemWide",
        "evaluateJavaScript",
        "with_ipc_handler",
        "with_initialization_script",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "macOS native-input probe contains forbidden authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_windows_probe_resource_sampler(module: &str, source: &str) -> Result<(), String> {
    validate_engine_agentic_native_unsafe_contract(ENGINE_WINDOWS_PROBE_RESOURCES, source)?;
    validate_agentic_no_direct_logging_attribute(ENGINE_WINDOWS_PROBE_RESOURCES, source)?;
    validate_agentic_no_invariant_panic_attribute(ENGINE_WINDOWS_PROBE_RESOURCES, source)?;
    validate_agentic_no_direct_logging_calls(ENGINE_WINDOWS_PROBE_RESOURCES, source)?;
    let module = compact(module);
    for required in [
        "#[cfg(any(feature=\"native-agentic-input-probe\",feature=\"native-agentic-semantic-probe\"))]modagentic_probe_resources;",
    ] {
        if !module.contains(required) {
            return Err(format!(
                "Windows probe resource sampler escaped its diagnostic feature gate {required}"
            ));
        }
    }

    let code = compact(
        &source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "pub(crate)structWebView2ResourceSample{pub(crate)processes:u8,pub(crate)resident_bytes:u64,}",
        "typeProcessCohort=[(u32,i32);MAX_RESOURCE_WEBVIEW2_PROCESSESasusize];",
        "pub(crate)fnsample_webview2_resources<E>(environment:&ICoreWebView2Environment,check_control:&mutimplFnMut()->Result<(),E>,)->Result<Option<WebView2ResourceSample>,E>",
        "sample_resources_inner(environment,check_control,&mutfailure)",
        "pub(crate)fnsample_webview2_resources_diagnostic<E>(",
        "Ok((sample,failure))",
        "usezephium_agentic::{MAX_RESOURCE_RESIDENT_BYTES,MAX_RESOURCE_WEBVIEW2_PROCESSES};",
        "letenvironment=environment.cast::<ICoreWebView2Environment8>();check_control()?;letOk(environment)=environmentelse{*failure=Some(ResourceSampleFailure::Environment8Interface);returnOk(None);};",
        "letprocesses=unsafe{environment.GetProcessInfos()};check_control()?;letOk(processes)=processeselse{*failure=Some(ResourceSampleFailure::ProcessInfosQuery);returnOk(None);};",
        "letcount_result=unsafe{processes.Count(&mutcount)};check_control()?;ifcount_result.is_err()",
        ".filter(|count|(1..=MAX_RESOURCE_WEBVIEW2_PROCESSES).contains(count))",
        "letmutprocess_cohort=[(0_u32,0_i32);MAX_RESOURCE_WEBVIEW2_PROCESSESasusize];",
        "letmutprocess_handles:[Option<ProbeProcessHandle>;MAX_RESOURCE_WEBVIEW2_PROCESSESasusize]",
        "active_process_cohort.windows(2).any(|pair|pair[0].0==pair[1].0)",
        "letprocess=unsafe{processes.GetValueAtIndex(index)};check_control()?;letOk(process)=processelse{*failure=Some(ResourceSampleFailure::ProcessEntryQuery);returnOk(None);};",
        "letkind_result=unsafe{process.Kind(&mutkind)};check_control()?;ifkind_result.is_err()",
        "ifkind==COREWEBVIEW2_PROCESS_KIND_BROWSER",
        "letprocess_id_result=unsafe{process.ProcessId(&mutprocess_id)};check_control()?;ifprocess_id_result.is_err()",
        "*slot=(process_id,kind.0);",
        "unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,process_id)}",
        "(!handle.is_null()).then_some(Self(handle))",
        "usewindows_probe_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo,PROCESS_MEMORY_COUNTERS,};",
        "letmutcounters=PROCESS_MEMORY_COUNTERS::default();letcounter_bytes=u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;counters.cb=counter_bytes;",
        "ifunsafe{GetProcessMemoryInfo(self.raw(),&mutcounters,counter_bytes)}==0",
        "lethandle=std::mem::replace(&mutself.0,std::ptr::null_mut());",
        "unsafe{CloseHandle(handle)!=0}",
        "all_closed&=handle.close()",
        "letSome((process_cohort,process_count,helper_processes))=webview2_process_cohort(&environment,check_control,failure)?else{returnOk(None);};",
        "letSome((rejoined_process_cohort,rejoined_count,rejoined_helper_processes))=webview2_process_cohort(&environment,check_control,failure)?else{returnOk(None);};",
        "ifrejoined_count!=process_count||rejoined_helper_processes!=helper_processes||rejoined_process_cohort.get(..process_count_usize)!=Some(active_process_cohort)",
        "ifbrowser_processes!=1",
        "process_count.checked_sub(browser_processes).filter(|count|*count!=0)",
        "letopened_handle=ProbeProcessHandle::open(process_id,failure);check_control()?;letSome(handle)=opened_handleelse{returnOk(None);};letprocess_resident_bytes=handle.resident_bytes(failure);check_control()?;letSome(process_resident_bytes)=process_resident_byteselse{returnOk(None);};",
        "resident_bytes.checked_add(process_resident_bytes)",
        "ifresident_bytes>MAX_RESOURCE_RESIDENT_BYTES",
        "letall_handles_closed=close_process_handles(active_handles);check_control()?;if!all_handles_closed",
        "(resident_bytes!=0).then_some(WebView2ResourceSample{processes:process_count,resident_bytes,})",
    ] {
        if !code.contains(required) {
            return Err(format!(
                "Windows probe resource sampler lost required bounded mechanism {required}"
            ));
        }
    }
    if code.matches("check_control()?;").count() != 14
        || code.matches("environment.GetProcessInfos()").count() != 1
        || code.matches("processes.GetValueAtIndex(index)").count() != 1
        || code.matches("process.Kind(&mutkind)").count() != 1
        || code.matches("OpenProcess(").count() != 1
        || code.matches("GetProcessMemoryInfo(").count() != 1
        || code.matches("CloseHandle(").count() != 1
        || code
            .matches("webview2_process_cohort(&environment,check_control,failure)?")
            .count()
            != 2
    {
        return Err(
            "Windows probe resource sampler must retain exact bounded native/control call sites"
                .to_owned(),
        );
    }
    for forbidden in [
        "PROCESS_ALL_ACCESS",
        "PROCESS_QUERY_INFORMATION",
        "PROCESS_VM_",
        "CreateToolhelp32Snapshot",
        "EnumProcesses(",
        "NtQuerySystemInformation",
        "Process32First",
        "Process32Next",
        "DuplicateHandle(",
        "CreateProcess",
        "TerminateProcess(",
        "ReadProcessMemory(",
        "WriteProcessMemory(",
        "Vec<",
        "HashMap<",
        "serde",
        "std::fs",
        "std::net",
        "std::process",
        "MAX_RESOURCE_HELPER_PROCESSES",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "Windows probe resource sampler acquired forbidden authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_windows_probe_source(source: &str) -> Result<(), String> {
    validate_engine_agentic_native_unsafe_contract(ENGINE_WINDOWS_PROBE_MODULE, source)?;
    validate_agentic_no_invariant_panic_attribute(ENGINE_WINDOWS_PROBE_MODULE, source)?;
    let code = compact(
        &source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    let source = compact(source);
    for required in [
        "attest_native_view(&host,view,matrix.presentation)",
        "IsWindow(Some(host.hwnd))",
        "GetParent(container)",
        "controller.ParentWindow(&mutcontroller_parent)",
        "controller.IsVisible(&mutcontroller_visible)",
        "GetClientRect(container,&mutcontainer_bounds)",
        "controller.Bounds(&mutcontroller_bounds)",
        "GetDpiForWindow(container)",
        "expected_physical_extent(PROBE_WIDTH,dpi)",
        "GetWindow(container,GW_CHILD)",
        "GetWindow(self.container,GW_CHILD)",
        "GetParent(self.document)",
        "GetWindowThreadProcessId(document,Some(&mutowner_process_id))",
        "GetCurrentThreadId()",
        "owner_thread_id==current_thread_id",
        "owner_thread_id==self.owner_thread_id",
        "owner_process_id==self.owner_process_id",
        "letkeyboard_layout=unsafe{GetKeyboardLayout(owner_thread_id)};",
        "keyboard_layout.is_invalid()",
        "keyboard_layout==self.keyboard_layout",
        "SendMessageTimeoutW(",
        "SMTO_ABORTIFHUNG|SMTO_BLOCK|SMTO_ERRORONEXIT,timeout_ms",
        "check_dispatch_control(permit,&mutpoll_control,run_deadline).map_err(|error|adapter_failure(error,ProbeStage::Admit,None,None))?;",
        "check_dispatch_control(permit,&mutpoll_control,run_deadline).map_err(|error|adapter_failure(error,ProbeStage::Construct,None,None))?;letmutwebview=builder.build_as_child(&host).ok();",
        "check_dispatch_control(permit,&mutpoll_control,run_deadline).map_err(|error|adapter_failure(error,ProbeStage::Navigate,Some(case),Some(backend)),)?;view.load_url(&url)",
        "check_dispatch_control(permit,poll_control,deadline)?;observe_focus()?;match*step",
        "send_message(self,message,wparam,lparam,permit,poll_control,observe_focus,deadline,)",
        "letmutresult=0_usize;check_dispatch_control(permit,poll_control,deadline)?;observe_focus()?;if!target.is_current(){returnErr(AdapterError::NativeConstruction);}lettimeout_ms=message_timeout_ms(deadline)?;iftimeout_ms==0||timeout_ms>SEND_TIMEOUT_MS{returnErr(AdapterError::Timeout);}letsent=unsafe{SendMessageTimeoutW(target.document,",
        "ifsent.0==0{returnErr(AdapterError::Timeout);}target.is_current().then_some(()).ok_or(AdapterError::NativeConstruction)",
        "observe_focus()?;check_dispatch_control(permit,poll_control,deadline)?;",
        "FixedProbeScript::DomRecipe(case),permit,poll_control,Some(&mut*observe_focus),deadline,",
        "poll_control,Some(&mut*observe_focus),deadline,)?;validate_cdp_response(&response)?;observe_focus()?;",
        "letparameters=HSTRING::from(parameters);check_dispatch_control(permit,poll_control,deadline)?;ifletSome(before_dispatch)=before_dispatch{before_dispatch()?;}unsafe{core.CallDevToolsProtocolMethod(",
        "check_dispatch_control(permit,poll_control,deadline)?;view.set_visible(false)",
        "check_dispatch_control(permit,poll_control,deadline)?;let_=unsafe{ShowWindow(host.hwnd,SW_HIDE)};",
        "check_dispatch_control(permit,poll_control,deadline)?;unsafe{SetWindowPos(",
        "check_dispatch_control(permit,poll_control,deadline)?;let_=unsafe{ShowWindow(host.hwnd,SW_SHOWNOACTIVATE)};",
        "check_dispatch_control(permit,poll_control,deadline)?;let_=unsafe{ShowWindow(host.hwnd,SW_SHOW)};",
        "check_dispatch_control(permit,poll_control,deadline)?;if!unsafe{SetForegroundWindow(host.hwnd)}.as_bool()",
        "check_dispatch_control(permit,poll_control,deadline)?;unsafe{SetFocus(Some(view.hwnd()))}",
        "MAPVK_VK_TO_VSC_EX",
        "MapVirtualKeyExW(",
        "Some(target.keyboard_layout)",
        "windows_key_message_lparam(mapped_scan,down)",
        "windows_key_message_lparam(mapped_scan,true)",
        "target.send(WM_CHAR,WPARAM(usize::from(b'x')),LPARAM(lparam),permit,poll_control,observe_focus,deadline,)",
        "verify_nonactivating_presentation(&host,view,matrix.presentation)",
        "letnative_focus_before=native_focus_sample(view).map_err(",
        "letnative_focus_during=native_focus_sample(view).map_err(",
        "letnative_focus_after=native_focus_sample(view).map_err(",
        "letmutdocument_gui=GUITHREADINFO{cbSize:u32::try_from(std::mem::size_of::<GUITHREADINFO>()).map_err(|_|AdapterError::InvalidEvidence)?,..Default::default()};",
        "GetGUIThreadInfo(target.owner_thread_id,&mutdocument_gui).map_err(|_|AdapterError::FocusPolicy)?;",
        "(GetForegroundWindow(),GetActiveWindow(),GetFocus())",
        "document_active:document_gui.hwndActive,document_focus:document_gui.hwndFocus,",
        "focus_is_owned_by_view(view,self.caller_focus)||focus_is_owned_by_view(view,self.document_focus)",
        "letfocus=native_focus_sample(view)?;iffocus.probe_host_is_key(host)||focus.view_has_focus(view)",
        "ICoreWebView2CallDevToolsProtocolMethodCompletedHandler",
        "borrowed_pcwstr_bounded(response,MAX_CDP_RESPONSE_UTF16_UNITS,MAX_CDP_RESPONSE_BYTES,).ok_or(AdapterError::InvalidEvidence)",
        "method:FixedCdpMethod",
        "Self::InputDispatchMouseEvent=>\"Input.dispatchMouseEvent\"",
        "Self::InputDispatchKeyEvent=>\"Input.dispatchKeyEvent\"",
        "Self::RuntimeEvaluate=>\"Runtime.evaluate\"",
        "\"userGesture\":false",
        "EvidenceLabel::new(\"native-input-m1-resources-v4\")",
        "letmutcheck_control=||check_dispatch_control(permit,poll_control,deadline);",
        "super::agentic_probe_resources::sample_webview2_resources(environment,&mutcheck_control)?",
        "helper_processes:sample.and_then(|sample|sample.processes.checked_sub(1)),resident_bytes:sample.map(|sample|sample.resident_bytes)",
        "resources_before:unavailable_resource_sample(),resources_after:unavailable_resource_sample()",
    ] {
        if !code.contains(required) {
            return Err(format!(
                "Windows native-input probe lost required bounded mechanism {required}"
            ));
        }
    }
    if code
        .matches("check_dispatch_control(permit,&mutpoll_control,run_deadline)")
        .count()
        != 3
        || code.matches("view.load_url(&url)").count() != 1
        || code.matches("SendMessageTimeoutW(").count() != 1
        || code.matches("core.CallDevToolsProtocolMethod(").count() != 1
        || code.matches("GetGUIThreadInfo(").count() != 1
        || code.matches("view.set_visible(true)").count() != 2
        || code
            .matches("check_dispatch_control(permit,poll_control,deadline)?;view.set_visible(true)")
            .count()
            != 2
        || code.matches("ShowWindow(host.hwnd").count() != 3
        || code.matches("SetWindowPos(").count() != 1
        || code.matches("SetForegroundWindow(host.hwnd)").count() != 1
        || code.matches("SetFocus(Some(view.hwnd()))").count() != 1
        || code
            .matches("super::agentic_probe_resources::sample_webview2_resources(")
            .count()
            != 1
    {
        return Err(
            "Windows native-input probe must retain exact effect and shared resource-sampling call sites"
                .to_owned(),
        );
    }
    for forbidden in [
        "SendInput(",
        "SendMessageW(",
        "SendNotifyMessageW(",
        "SendMessageCallbackW(",
        "PostMessageW(",
        "PostThreadMessageW(",
        "BroadcastSystemMessage",
        "SetCursorPos(",
        "mouse_event(",
        "keybd_event(",
        "InjectMouseInput(",
        "InjectKeyboardInput(",
        "InjectSyntheticPointerInput(",
        "CreateSyntheticPointerDevice(",
        "SetCapture(",
        "ReleaseCapture(",
        "SetActiveWindow(",
        "BringWindowToTop(",
        "SendMouseInput(",
        "SendPointerInput(",
        "SendKeyEvent(",
        "PostWebMessage",
        "ExecuteScript(",
        ".eval(",
        "with_ipc_handler",
        "with_initialization_script",
        "GetProcessInfos(",
        "OpenProcess(",
        "GetProcessMemoryInfo(",
        "CloseHandle(",
        "AddHostObject",
        "AttachThreadInput(",
        "unwrap_or(char::REPLACEMENT_CHARACTER)",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "Windows native-input probe contains forbidden authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_windows_semantic_probe(
    module: &str,
    source: &str,
    binary: &str,
    fixture: &str,
) -> Result<(), String> {
    validate_engine_agentic_native_unsafe_contract(ENGINE_WINDOWS_SEMANTIC_PROBE_MODULE, source)?;
    validate_agentic_no_invariant_panic_attribute(ENGINE_WINDOWS_SEMANTIC_PROBE_MODULE, source)?;
    validate_agentic_no_invariant_panic_attribute(ENGINE_WINDOWS_SEMANTIC_PROBE_BINARY, binary)?;
    let module = compact(module);
    for required in [
        "#[cfg(feature=\"native-agentic-semantic-probe\")]modagentic_semantic_probe;",
        "#[cfg(feature=\"native-agentic-semantic-probe\")]pub(crate)useagentic_semantic_probe::runasrun_agentic_semantic_probe;",
    ] {
        if !module.contains(required) {
            return Err(format!(
                "Windows semantic probe escaped or drifted from required gate {required}"
            ));
        }
    }

    let source = compact(source);
    for required in [
        "FixtureRoute::SemanticRuntime",
        "FixtureRoute::SemanticRuntimeReplacement",
        "FixtureRoute::SemanticRuntimeEventFlood",
        "FixtureRoute::SemanticLocationMutation",
        "FixtureRoute::SemanticRedirectStart",
        "FixtureRoute::SemanticRedirectFinal",
        "FixtureRoute::SemanticRedirectLoopA",
        "FixtureServer::start()",
        "tempfile::tempdir()",
        "WebContext::new(Some(profile.path().to_path_buf()))",
        ".with_incognito(true)",
        ".with_visible(false)",
        ".with_focused(false)",
        "ContextProfileStorageClass::Ephemeral",
        "ContextOwnedViewport::STANDARD",
        "AgentOwnedProfile::automation(profile_id)",
        "build_owned_agent_view(",
        "move||location_callbacks.request_location_check()",
        "ContextConstructionProof::WindowsOwnedAutomationSubprofileEmptyInventory",
        "NativeContentPolicy::AllowAll",
        "view.prepare_semantic_document_load()",
        "view.navigation().document_finished_for_audit(operation)",
        "super::current_url(view.view())",
        "view.navigation().finish_location_check(false).map_err(|_|ProbeError::verify(WindowsSemanticProbeStage::Navigate))",
        "view.navigation().location_stable_for_result()",
        "!callbacks.location_check_pending.get()",
        "view.dispatch_semantic(",
        "view.semantic_work_drained_for_audit()",
        "AgentSuspendClaim::new()",
        "view.try_suspend(",
        "view.attest_suspension_state()==Ok(true)",
        "view.resume_and_attest_active()==Ok(true)",
        "WindowsSemanticProbeMode::HiddenSuspendResume",
        "WindowsSemanticProbeMode::HiddenRedirectLifecycle",
        "WindowsSemanticProbeMode::HiddenLocationReplacement",
        "server.semantic_location_waiting()",
        "server.release_semantic_location()",
        "server.semantic_location_completed()",
        "server.semantic_location_replacement_url()",
        "view.navigation().finish_location_check(true)",
        "registry.observe_navigation_replacement(context_id,prior)",
        "view.navigation().acknowledge_location_replacement()",
        "view.navigation().location_state_for_audit()!=Some((false,false,false))",
        "verify_location_before_snapshot(&before)?;",
        "verify_location_after_snapshot(&after)?;",
        "\"semantic-runtime-m3-lifecycle-m2-redirect-location-resources-v4\"",
        "facts.resources_before=Some(sample_resources(",
        "facts.resources_after=Some(sample_resources(",
        "super::agentic_probe_resources::sample_webview2_resources_diagnostic(environment,&mutcheck_control,)?",
        "constRESOURCE_STARTUP_TIMEOUT:Duration=Duration::from_secs(2);",
        "constRESOURCE_STARTUP_RETRY_INTERVAL:Duration=Duration::from_millis(25);",
        "constMAX_RESOURCE_STARTUP_SAMPLES:u8=64;",
        "&native_guard,construction_deadline,WindowsSemanticProbeStage::Construct,",
        "letretry=stage==WindowsSemanticProbeStage::Construct&&samples<MAX_RESOURCE_STARTUP_SAMPLES&&Instant::now()<startup_deadline",
        "ResourceSampleFailure::HelperProcessesEmpty|super::agentic_probe_resources::ResourceSampleFailure::ProcessCohortChanged",
        "ifletSome(sample)=sample{breaksample;}",
        "if!retry{ifstage==WindowsSemanticProbeStage::Construct{construction_failure_checkpoint(\"resource_sample_unavailable\");}returnErr(ProbeError::verify(stage));}",
        "resources_before:WindowsSemanticResourceEvidence{webview2_processes:resources_before.processes,resident_bytes:resources_before.resident_bytes,}",
        "resources_after:WindowsSemanticResourceEvidence{webview2_processes:resources_after.processes,resident_bytes:resources_after.resident_bytes,}",
        "SemanticRuntimePortFailure::Transport",
        "SemanticRuntimePortFailure::RendererLost",
        "SemanticRuntimeFault::DocumentLoading",
        "MAX_DOCUMENT_LOADING_RETRIES:u16=512",
        "HSTRING::from(\"Page.crash\")",
        "IsDebuggerPresent()",
        "GetForegroundWindow()",
        "GetActiveWindow()",
        "GetFocus()",
        "MsgWaitForMultipleObjectsEx(",
        "for_in0..256",
        "browser_process_for_environment(&captured)",
        "install_browser_process_exit_observer(&captured,browser.id(),|_|{})",
        "observer.observed_expected_exit()&&process.has_exited()",
        "profile.close().is_ok()",
        "server.shutdown().is_ok()",
        "verify_first_snapshot(&first_snapshot).inspect_err(|_|{ifaction_guard{action_guard_checkpoint(\"initial_snapshot_verification\");}})?;",
        "facts.snapshots=1;facts.first_snapshot_verified=true;facts.page_world_bridge_absent=true;facts.secrets_redacted=true;",
        "verify_replacement_snapshot(&snapshot)?;",
        "facts.snapshots=2;facts.replacement_snapshot_verified=true;facts.replacement_stale_state_absent=true;",
        "ifoutcome!=Err(SemanticRuntimePortFailure::Transport)||callbacks.invariant_failures.get()!=1",
        "facts.event_flood_refused=true;",
        "facts.recovered_after_event_flood=true;",
        "ifoutcome!=Err(SemanticRuntimePortFailure::RendererLost)",
        "facts.renderer_loss_observed=true;facts.renderer_lost_refused=true;",
        "facts.suspend_callback_succeeded=true;facts.suspended_state_attested=true;facts.resume_state_attested=true;",
        "ContextNavigationRedirectPolicy::same_origin(&loop_target)",
        "refuse_redirect_limit(",
        "navigate_redirect(",
        "view.navigation().redirect_probe_audit(operation)",
        "super::stop_loading(view.view());",
        "ifrefused_hops!=MAX_CONTEXT_NAVIGATION_REDIRECTSasu8",
        "ifchain_hops!=2",
        "facts.redirect_limit_refused=true;",
        "facts.redirect_chain_verified=true;",
        "facts.redirect_recovery_verified=true;",
        "facts.same_document_replacement_observed=true;",
        "facts.same_document_replacement_rejoined=true;",
        "facts.stale_location_join_refused=true;",
        "facts.post_location_snapshot_verified=true;",
        "first_snapshot_verified:facts.first_snapshot_verified",
        "replacement_snapshot_verified:facts.replacement_snapshot_verified",
        "event_flood_refused:facts.event_flood_refused",
        "renderer_loss_observed:facts.renderer_loss_observed",
        "suspend_callback_succeeded:facts.suspend_callback_succeeded",
        "redirect_chain_verified:facts.redirect_chain_verified",
        "redirect_limit_refused:facts.redirect_limit_refused",
        "redirect_recovery_verified:facts.redirect_recovery_verified",
        "same_document_replacement_observed:facts.same_document_replacement_observed",
        "same_document_replacement_rejoined:facts.same_document_replacement_rejoined",
        "stale_location_join_refused:facts.stale_location_join_refused",
        "post_location_snapshot_verified:facts.post_location_snapshot_verified",
        "semantic_work_drained:facts.semantic_work_drained",
        "runtime_retired:teardown.runtime_retired",
        "work_drained:teardown.work_drained",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "Windows semantic probe lost required production-path mechanism {required}"
            ));
        }
    }
    if source.matches("CallDevToolsProtocolMethod(").count() != 1 {
        return Err(
            "Windows semantic probe must expose exactly one fixed renderer-crash CDP call"
                .to_owned(),
        );
    }
    for forbidden in [
        "Runtime.evaluate",
        "Page.addScriptToEvaluateOnNewDocument",
        "Page.removeScriptToEvaluateOnNewDocument",
        "Page.bringToFront",
        "Page.captureScreenshot",
        "Input.dispatch",
        "ExecuteScript(",
        "evaluate_script",
        "querySelector",
        "outerHTML",
        "innerHTML",
        "document.cookie",
        "PostWebMessage",
        "with_ipc_handler",
        "with_initialization_script",
        "AddHostObject",
        "SendInput(",
        "SetCursorPos(",
        "SetFocus(",
        "SetForegroundWindow(",
        "mouse_event(",
        "keybd_event(",
        "http://",
        "https://",
        "method:&str",
        "serde_json::Value",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "Windows semantic probe acquired forbidden authority {forbidden}"
            ));
        }
    }

    let binary = compact(binary);
    for required in [
        "WindowsSemanticProbeMode::from_argument",
        "--evidence-directory",
        "eval/agentic-browsing/local-results",
        "mode.local_result_filename()",
        "evidence_metadata_is_direct_directory(&metadata)",
        "NamedTempFile::new_in(directory)",
        "pending.file.as_file().sync_all()",
        "persist_noclobber(pending.destination)",
        "qualify_windows_semantic_probe_evidence(mode,&evidence)",
        "encode_windows_semantic_probe_response(&response)",
        "WindowsSemanticProbeReply::Completed(Box::new(evidence))",
    ] {
        if !binary.contains(required) {
            return Err(format!(
                "Windows semantic qualification runner lost required closed gate {required}"
            ));
        }
    }
    for forbidden in ["stdout", ".display()"] {
        if binary.contains(forbidden) {
            return Err(format!(
                "Windows semantic qualification runner exposes forbidden output {forbidden}"
            ));
        }
    }

    let fixture = compact(fixture);
    for required in [
        "Self::SemanticRuntimeEventFlood=>\"/semantic-runtime-event-flood-v1.html\"",
        "index<512",
        "frame.hidden=true",
        "frame.srcdoc=",
        "connect-src'none'",
        "form-action'none'",
        "frame-src'self'",
        "Self::SemanticLocationMutation=>\"/semantic-location-mutation-v1.html\"",
        "window.addEventListener('load'",
        "trigger.src='/semantic-location-trigger-v1.js'",
        "history.replaceState(null,'','/semantic-location-replaced-v1.html')",
    ] {
        if !fixture.contains(required) {
            return Err(format!(
                "Windows semantic pressure fixture lost required bound {required}"
            ));
        }
    }
    Ok(())
}

fn validate_manifest(source: &str) -> Result<(), String> {
    let manifest: toml::Value = toml::from_str(source)
        .map_err(|error| format!("cannot parse {AGENTIC_MANIFEST}: {error}"))?;
    if manifest
        .get("package")
        .and_then(|package| package.get("publish"))
        .and_then(toml::Value::as_bool)
        != Some(false)
    {
        return Err("zephium-agentic must remain an unpublished internal crate".to_owned());
    }
    let harness = manifest
        .get("features")
        .and_then(|features| features.get("probe-harness"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "zephium-agentic probe-harness feature is missing".to_owned())?;
    // The harness may switch on the decision layer's eval corpus and nothing else.
    let harness = harness
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    if harness != ["zephium-decision?/evals"] {
        return Err(
            "probe-harness must not activate an implicit dependency or shipping feature".to_owned(),
        );
    }
    let default = manifest
        .get("features")
        .and_then(|features| features.get("default"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "zephium-agentic default feature is missing".to_owned())?;
    if !default.is_empty() {
        return Err("zephium-agentic default feature set must remain empty".to_owned());
    }
    let dependencies = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "zephium-agentic dependency table is missing".to_owned())?;
    let actual = dependencies
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = [
        "base64",
        "crc32fast",
        "futures-util",
        // Pure pinned suffix data for same-site takeover checks (68a07141).
        "psl",
        "reqwest",
        // Optional provider-transport clients: structured requests (1783cf12),
        // typed decisions and their Retry-After parsing (77e1dbcf).
        "rig-core",
        "httpdate",
        "zephium-decision",
        "serde",
        "serde_json",
        "sha2",
        "thiserror",
        "tokio",
        "ulid",
        "url",
        "zeroize",
        "zephium-core",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(
            "zephium-agentic default dependency graph acquired unreviewed runtime authority"
                .to_owned(),
        );
    }
    let provider_transport = manifest
        .get("features")
        .and_then(|features| features.get("provider-transport"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "zephium-agentic provider-transport feature is missing".to_owned())?;
    let actual_provider_transport = provider_transport
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let expected_provider_transport = [
        "dep:dirs",
        "dep:futures-util",
        "dep:httpdate",
        "dep:reqwest",
        "dep:rig-core",
        "dep:security-framework",
        "dep:security-framework-sys",
        "dep:tokio",
        "dep:zeroize",
        "dep:zephium-decision",
        "dep:zephium-credentials",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if actual_provider_transport != expected_provider_transport
        || actual_provider_transport.len() != provider_transport.len()
    {
        return Err("zephium-agentic provider-transport feature graph drifted".to_owned());
    }
    if dependencies
        .get("psl")
        .and_then(toml::Value::as_str)
        .is_none_or(|version| version != "=2.1.220")
    {
        return Err("zephium-agentic suffix data must remain an exact plain pin".to_owned());
    }
    let windows = manifest
        .get("target")
        .and_then(|targets| targets.get("cfg(target_os = \"windows\")"))
        .and_then(|target| target.get("dependencies"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "Windows provider vault dependency is missing".to_owned())?;
    if windows.len() != 1
        || windows.get("zephium-credentials").is_none_or(|dependency| {
            dependency.get("workspace").and_then(toml::Value::as_bool) != Some(true)
                || dependency.get("optional").and_then(toml::Value::as_bool) != Some(true)
                || dependency.as_table().is_none_or(|table| table.len() != 2)
        })
    {
        return Err(
            "Windows provider vault must remain the exact optional credential seam".to_owned(),
        );
    }
    let macos = manifest
        .get("target")
        .and_then(|targets| targets.get("cfg(target_os = \"macos\")"))
        .and_then(|target| target.get("dependencies"))
        .and_then(toml::Value::as_table);
    if macos
        .and_then(|macos| macos.get("dirs"))
        .is_some_and(|dirs| dirs.get("optional").and_then(toml::Value::as_bool) != Some(true))
    {
        return Err("zephium-agentic runtime dependency dirs must remain optional".to_owned());
    }
    for runtime in [
        "futures-util",
        "reqwest",
        "tokio",
        "zeroize",
        "rig-core",
        "httpdate",
        "zephium-decision",
    ] {
        if dependencies
            .get(runtime)
            .and_then(toml::Value::as_table)
            .and_then(|dependency| dependency.get("optional"))
            .and_then(toml::Value::as_bool)
            != Some(true)
        {
            return Err(format!(
                "zephium-agentic runtime dependency {runtime} must remain optional"
            ));
        }
    }
    let review_binary = manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .and_then(|binaries| {
            binaries.iter().find(|binary| {
                binary.get("name").and_then(toml::Value::as_str)
                    == Some("windows-agentic-input-evidence-review")
            })
        })
        .ok_or_else(|| "Windows agentic evidence-review binary is missing".to_owned())?;
    if review_binary.get("path").and_then(toml::Value::as_str)
        != Some("src/bin/windows_agentic_input_evidence_review.rs")
        || review_binary
            .get("required-features")
            .and_then(toml::Value::as_array)
            .is_none_or(|features| {
                features.as_slice() != [toml::Value::String("probe-harness".into())]
            })
    {
        return Err("Windows agentic evidence-review binary gate drifted".to_owned());
    }
    let semantic_review_binary = manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .and_then(|binaries| {
            binaries.iter().find(|binary| {
                binary.get("name").and_then(toml::Value::as_str)
                    == Some("windows-agentic-semantic-evidence-review")
            })
        })
        .ok_or_else(|| "Windows semantic evidence-review binary is missing".to_owned())?;
    if semantic_review_binary
        .get("path")
        .and_then(toml::Value::as_str)
        != Some("src/bin/windows_agentic_semantic_evidence_review.rs")
        || semantic_review_binary
            .get("required-features")
            .and_then(toml::Value::as_array)
            .is_none_or(|features| {
                features.as_slice() != [toml::Value::String("probe-harness".into())]
            })
    {
        return Err("Windows semantic evidence-review binary gate drifted".to_owned());
    }
    Ok(())
}

fn validate_root(source: &str) -> Result<(), String> {
    if !compact(source).contains(RELEASE_REFUSAL) {
        return Err(
            "zephium-agentic must retain its exact optimized probe-harness compile refusal"
                .to_owned(),
        );
    }
    let source = compact(source);
    if !source.contains("#[cfg(feature=\"provider-transport\")]modprovider_transport;") {
        return Err(
            "zephium-agentic provider transport must remain behind its optional feature".to_owned(),
        );
    }
    if !source.contains(
        "#[cfg(all(feature=\"provider-transport\",feature=\"probe-harness\"))]pubuseprovider_transport::{exact_loopback_url,ProviderEndpoints};",
    ) {
        return Err(
            "agentic loopback endpoints must remain behind provider-transport plus probe-harness"
                .to_owned(),
        );
    }
    for module in [
        "contract",
        "control",
        "evidence",
        "fixture_server",
        "probe_evidence_path",
        "probe_recipes",
        "probe_qualification",
        "protocol",
        "semantic_probe_evidence",
    ] {
        let required_gate = format!("#[cfg(feature=\"probe-harness\")]mod{module};");
        if !source.contains(&required_gate) {
            return Err(format!(
                "agentic diagnostic module {module} must remain behind probe-harness"
            ));
        }
    }
    Ok(())
}

fn validate_probe_evidence_path(source: &str) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "constWINDOWS_FILE_ATTRIBUTE_REPARSE_POINT:u32=0x0000_0400;",
        "usestd::os::windows::fs::MetadataExtas_;",
        "windows_attributes_have_no_reparse_point(metadata.file_attributes())",
        "pubfnevidence_metadata_is_direct_directory(metadata:&Metadata)->bool",
        "metadata.file_type().is_dir()&&!metadata.file_type().is_symlink()&&metadata_has_no_windows_reparse_point(metadata)",
        "pubfnevidence_metadata_is_direct_file(metadata:&Metadata)->bool",
        "metadata.file_type().is_file()&&!metadata.file_type().is_symlink()&&metadata_has_no_windows_reparse_point(metadata)",
        "attributes&WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT==0",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "Windows evidence path validation lost required direct-path rule {required}"
            ));
        }
    }
    Ok(())
}

fn validate_windows_review_binary(source: &str, qualification: &str) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "WINDOWS_PHYSICAL_REVIEW_MODES",
        "symlink_metadata(&directory)",
        "evidence_metadata_is_direct_directory(&metadata)",
        "evidence_metadata_is_direct_file(&metadata)",
        "decode_response_line(&bytes)",
        "qualify_windows_probe_evidence(mode,&evidence)",
        "file.take((MAX_PROTOCOL_OUTPUT_BYTES+1)asu64)",
        "output.len()>MAX_PROTOCOL_OUTPUT_BYTES",
        "stdout.write_all(&output)",
        "--write-summary",
        "constREVIEW_SCHEMA_VERSION:u16=2",
        "windows-review-summary-v2.json",
        ".create_new(true)",
        "write_new_record(&directory,REVIEW_SUMMARY_FILENAME,&output)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "Windows physical evidence reviewer lost required boundary {required}"
            ));
        }
    }
    let qualification = compact(qualification);
    for required in [
        "windows-hidden-fixed-dom.jsonl",
        "windows-hidden-hwnd.jsonl",
        "windows-hidden-cdp.jsonl",
        "windows-visible-background-all.jsonl",
        "pubfnqualify_windows_probe_evidence(",
        "evidence.capabilities.as_slice()!=WINDOWS_PROBE_CAPABILITIES",
    ] {
        if !qualification.contains(required) {
            return Err(format!(
                "Windows physical evidence qualification lost required boundary {required}"
            ));
        }
    }
    for forbidden in [
        "read_to_string",
        "String::from_utf8",
        "stdout.write_all(&bytes)",
        ".display()",
        "serde_json::Value",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "Windows physical evidence reviewer can expose unreviewed input {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_windows_semantic_review_binary(source: &str, evidence: &str) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "constREVIEW_SCHEMA_VERSION:u16=5;",
        "WINDOWS_SEMANTIC_PHYSICAL_REVIEW_MODES",
        "symlink_metadata(directory)",
        "evidence_metadata_is_direct_directory(&metadata)",
        "evidence_metadata_is_direct_file(&metadata)",
        "decode_windows_semantic_probe_response(&bytes)",
        "qualify_windows_semantic_probe_evidence(mode,&evidence)",
        "file.take((MAX_WINDOWS_SEMANTIC_PROBE_OUTPUT_BYTES+1)asu64)",
        "output.len()>MAX_WINDOWS_SEMANTIC_PROBE_OUTPUT_BYTES",
        "stdout.write_all(&output)",
        "--write-summary",
        "windows-semantic-review-summary-v5.json",
        ".create_new(true)",
        "write_new_record(&directory,REVIEW_SUMMARY_FILENAME,&output)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "Windows semantic evidence reviewer lost required boundary {required}"
            ));
        }
    }
    let evidence = compact(evidence);
    for required in [
        "pubconstWINDOWS_SEMANTIC_PROBE_PROTOCOL_VERSION:u16=5;",
        "pubconstWINDOWS_SEMANTIC_PHYSICAL_REVIEW_MODES:[WindowsSemanticProbeMode;7]",
        "windows-semantic-fixed-documents.jsonl",
        "windows-semantic-redirect-lifecycle.jsonl",
        "windows-semantic-location-replacement.jsonl",
        "windows-semantic-suspend-resume.jsonl",
        "windows-semantic-event-flood.jsonl",
        "windows-semantic-renderer-loss.jsonl",
        "windows-semantic-debugger-coexistence.jsonl",
        "pubfnqualify_windows_semantic_probe_evidence(",
        "evidence.peak_pending_invocations!=1",
        "evidence.teardown.retained_native_views!=0",
        "evidence.suspend_callback_succeeded",
        "evidence.suspended_state_attested",
        "evidence.resume_state_attested",
        "evidence.post_resume_snapshot_verified",
        "evidence.redirect_chain_verified",
        "evidence.redirect_chain_hops_observed==2",
        "evidence.redirect_limit_refused",
        "evidence.redirect_limit_hops_observed==MAX_CONTEXT_NAVIGATION_REDIRECTSasu8",
        "evidence.redirect_recovery_verified",
        "evidence.same_document_replacement_observed",
        "evidence.same_document_replacement_rejoined",
        "evidence.stale_location_join_refused",
        "evidence.post_location_snapshot_verified",
        "valid_resource_sample(self.resources_before)",
        "valid_resource_sample(self.resources_after)",
        "maximum_webview2_processes",
        "maximum_resident_bytes",
        "semantic-runtime-m3-lifecycle-m2-redirect-location-resources-v4",
    ] {
        if !evidence.contains(required) {
            return Err(format!(
                "Windows semantic evidence qualification lost required boundary {required}"
            ));
        }
    }
    for forbidden in [
        "read_to_string",
        "String::from_utf8",
        "stdout.write_all(&bytes)",
        ".display()",
        "serde_json::Value",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "Windows semantic evidence reviewer can expose unreviewed input {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_agent_metrics_contract(root: &str, metrics: &str) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "modagent_metrics;",
        "AgentRunAccountingMetrics",
        "MAX_AGENT_METRIC_PRICING_SCHEDULES",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agent accounting metrics lost its default-core export {required}"
            ));
        }
    }

    let metrics = compact(metrics);
    for required in [
        "pubconstMAX_AGENT_METRIC_PRICING_SCHEDULES:usize=8;",
        "pubstructAgentRunAccountingMetrics",
        "pubfntry_new(manifest:&AgentRunManifest,supervisor:&AgentRunSupervisor",
        "pubfnrecord_model_receipt(",
        "pubfnrecord_effect_receipt(",
        "receipt.matches_manifest_revision(self.manifest,self.manifest_guard)",
        ".binary_search(&receipt.id())",
        ".binary_search(&receipt.attempt())",
        "self.validate_run_totals(next_operations,next_model)?",
        "ifnext_operations>self.operation_limit",
        "AgentMetricError::PricingScheduleLimit",
    ] {
        if !metrics.contains(required) {
            return Err(format!(
                "agent accounting metrics lost required bounded boundary {required}"
            ));
        }
    }
    for forbidden in [
        "traitAgentMetricPort",
        "implAgentAuditPort",
        "SerializeforAgentRunAccountingMetrics",
        "DeserializeforAgentRunAccountingMetrics",
    ] {
        if metrics.contains(forbidden) {
            return Err(format!(
                "run-local accounting metrics acquired forbidden telemetry/persistence seam {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_agent_action_metrics_contract(root: &str, metrics: &str) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "modagent_action_metrics;",
        "AgentRunActionPerformanceMetrics",
        "AgentRunActionPerformanceSnapshot",
        "AGENT_ACTION_DURATION_BUCKET_UPPER_BOUNDS_MILLIS",
        "MAX_AGENT_ACTION_PERFORMANCE_SNAPSHOT_BYTES",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agent action metrics lost its default-core export {required}"
            ));
        }
    }

    let metrics = compact(metrics);
    for required in [
        "pubconstAGENT_ACTION_DURATION_BUCKET_UPPER_BOUNDS_MILLIS:[u64;17]",
        "pubconstAGENT_ACTION_DURATION_BUCKET_COUNT:usize",
        "pubconstMAX_AGENT_ACTION_PERFORMANCE_SNAPSHOT_BYTES:usize=1_024;",
        "size_of::<AgentRunActionPerformanceSnapshot>()<=MAX_AGENT_ACTION_PERFORMANCE_SNAPSHOT_BYTES",
        "pubstructAgentRunActionPerformanceMetrics",
        "pubfntry_new(manifest:&AgentRunManifest,supervisor:&AgentRunSupervisor",
        "ifmanifest.plan_nodes().len()>MAX_AGENT_PLAN_NODES",
        "try_reserve_exact(manifest.plan_nodes().len())",
        "operation_limit:manifest.budget().operations()",
        "pubfnrecord_batch_result(&mutself,result:&SemanticActionBatchResult",
        "self.batches.binary_search(&result.batch())",
        "receipt.matches_manifest_revision(self.manifest,self.manifest_guard)",
        "existing_ids.binary_search(&id)",
        "existing_attempts.binary_search(&attempt)",
        "ifnext_actions>self.operation_limit",
        "ifresult.total()==0||usize::from(result.total())>MAX_SEMANTIC_ACTIONS_PER_BATCH",
        "ifadmitted!=usize::try_from(executed)",
        "[None;MAX_SEMANTIC_ACTIONS_PER_BATCH]",
        "self.batches.try_reserve(1)",
        "self.effect_receipts.try_reserve(admitted)",
        "self.effect_attempts.try_reserve(admitted)",
        "pubconstfnsnapshot(&self)->AgentRunActionPerformanceSnapshot",
    ] {
        if !metrics.contains(required) {
            return Err(format!(
                "agent action metrics lost required bounded boundary {required}"
            ));
        }
    }
    for forbidden in [
        "traitAgentActionMetricPort",
        "SerializeforAgentRunActionPerformanceMetrics",
        "DeserializeforAgentRunActionPerformanceMetrics",
        "HashMap<",
        "BTreeMap<",
    ] {
        if metrics.contains(forbidden) {
            return Err(format!(
                "run-local action metrics acquired forbidden telemetry/unbounded seam {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_agent_input_metrics_contract(
    root: &str,
    metrics: &str,
    request: &str,
    policy: &str,
    transport: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "modagent_input_metrics;",
        "AgentProviderInputMetricReceipt",
        "AgentRunProviderInputMetrics",
        "AgentRunProviderInputSnapshot",
        "MAX_AGENT_PROVIDER_INPUT_SNAPSHOT_BYTES",
        "MAX_AGENT_PROVIDER_INPUT_METRIC_RECEIPT_BYTES",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agent provider-input metrics lost its default-core export {required}"
            ));
        }
    }

    let metrics = compact(metrics);
    for required in [
        "pubconstMAX_AGENT_PROVIDER_INPUT_SNAPSHOT_BYTES:usize=1_024;",
        "pubstructAgentProviderInputKindMetrics{calls:u32,serialized_request_bytes:u64,disclosed_bytes:u64,semantic_lines:u64,semantic_payload_token_samples:u32,semantic_payload_tokens:u64,semantic_payload_qualities:[u32;4],structured_input_token_samples:u32,structured_input_tokens:u64,structured_input_qualities:[u32;4],}",
        "observation_secret_nodes:u64",
        "diff_secret_nodes:u64",
        "locate_withheld_secret_nodes:u64",
        "read_sensitive_items:u64",
        "extraction_sensitive_items:u64",
        "screenshot_dropped_ancillary_bytes:u64",
        "screenshot_layouts:[u32;2]",
        "pubstructAgentRunProviderInputSnapshot",
        "size_of::<AgentRunProviderInputSnapshot>()<=MAX_AGENT_PROVIDER_INPUT_SNAPSHOT_BYTES",
        "pubstructAgentRunProviderInputMetrics",
        // 6e9aafba adds the Decision kind for typed decision inputs.
        "kinds:[AgentProviderInputKindMetrics;7]",
        "receipts:Vec<AgentModelCallId>",
        "pubfntry_new(manifest:&AgentRunManifest,supervisor:&AgentRunSupervisor",
        "ifmanifest.plan_nodes().len()>MAX_AGENT_PLAN_NODES",
        "try_reserve_exact(manifest.plan_nodes().len())",
        "operation_limit:manifest.budget().operations()",
        "pubfnrecord(&mutself,receipt:AgentProviderInputMetricReceipt",
        "receipt.matches_manifest_revision(self.manifest,self.manifest_guard)",
        "self.receipts.binary_search(&receipt.call())",
        "binary_search_by_key(&receipt.node(),|row|row.metrics.node())",
        "ifnext_calls>self.operation_limit||next_node_calls>self.nodes[node_index].operation_limit",
        "validate_input_metrics(metrics)?",
        "self.receipts.try_reserve(1)",
        "self.receipts.insert(receipt_index,receipt.call())",
        "pubconstfnsnapshot(&self)->AgentRunProviderInputSnapshot",
        "AgentProviderSemanticInputStats::Observation(stats)",
        "AgentProviderSemanticInputStats::Diff(stats)",
        "AgentProviderSemanticInputStats::Locate(stats)",
        "AgentProviderSemanticInputStats::Read(stats)",
        "AgentProviderSemanticInputStats::Extraction(stats)",
        "AgentProviderSemanticInputStats::Screenshot(stats)",
    ] {
        if !metrics.contains(required) {
            return Err(format!(
                "agent provider-input metrics lost required bounded boundary {required}"
            ));
        }
    }
    for forbidden in [
        "traitAgentProviderInputMetricPort",
        "SerializeforAgentRunProviderInputMetrics",
        "DeserializeforAgentRunProviderInputMetrics",
        "HashMap<",
        "BTreeMap<",
        "std::thread",
        "std::time",
        "std::fs",
        "tokio::",
        "Mutex<",
        "Arc<",
    ] {
        if metrics.contains(forbidden) {
            return Err(format!(
                "run-local provider-input metrics acquired forbidden telemetry/runtime seam {forbidden}"
            ));
        }
    }

    let request = compact(request);
    for required in [
        "pubconstMAX_AGENT_PROVIDER_INPUT_METRIC_RECEIPT_BYTES:usize=192;",
        "size_of::<AgentProviderInputMetricReceipt>()<=MAX_AGENT_PROVIDER_INPUT_METRIC_RECEIPT_BYTES",
        "pubstructAgentProviderInputMetricReceipt{manifest:AgentRunManifestId,manifest_guard:[u8;32],call:crate::AgentModelCallId,lease:crate::AgentPlanLeaseId,node:crate::AgentPlanNodeId,metrics:AgentProviderInputMetrics,}",
        "manifest_guard:input.active.manifest_guard_for_metrics()",
        "pub(crate)fnmatches_manifest_revision(",
        "self.manifest==manifest&&self.manifest_guard==manifest_guard",
        "pubfnmetric_receipt(&self)->Option<AgentProviderInputMetricReceipt>",
        "self.metrics.accounting_is_final()",
        "AgentProviderInputMetricReceipt::from_committed(self)",
        "pubfninput_metric_receipt(&self)->Option<AgentProviderInputMetricReceipt>",
        "self.input.metric_receipt()",
        "pub(crate)fnsealed_input_metric_receipt(&self)->AgentProviderInputMetricReceipt",
        "self.input.sealed_metric_receipt()",
    ] {
        if !request.contains(required) {
            return Err(format!(
                "committed provider input lost exact content-free metric receipt {required}"
            ));
        }
    }
    let receipt_start = request
        .find("pubstructAgentProviderInputMetricReceipt")
        .ok_or_else(|| "provider input metric receipt boundary is missing".to_owned())?;
    let receipt_end = request[receipt_start..]
        .find("implAgentProviderInputEvidence")
        .map(|offset| receipt_start + offset)
        .ok_or_else(|| "provider input metric receipt boundary is unclosed".to_owned())?;
    let receipt = &request[receipt_start..receipt_end];
    for forbidden in [
        "pubfnnew(",
        "pubconstfnnew(",
        "Serialize",
        "Deserialize",
        "String",
        "Vec<",
        "Box<",
        "Arc<",
        "body:",
        "content:",
    ] {
        if receipt.contains(forbidden) {
            return Err(format!(
                "provider input metric receipt acquired forging/content authority {forbidden}"
            ));
        }
    }
    if request
        .matches("pubfnmetric_receipt(&self)->Option<AgentProviderInputMetricReceipt>")
        .count()
        != 1
        || request
            .matches("pubfninput_metric_receipt(&self)->Option<AgentProviderInputMetricReceipt>")
            .count()
            != 1
    {
        return Err(
            "provider input metric receipt must distinguish provisional from finalized accounting"
                .to_owned(),
        );
    }

    let policy = compact(policy);
    if !policy.contains(
        "pub(crate)constfnmanifest_guard_for_metrics(&self)->[u8;32]{self.manifest_guard}",
    ) {
        return Err(
            "provider input metric receipt lost crate-private manifest revision binding".to_owned(),
        );
    }

    let transport = compact(transport);
    for required in [
        "input_metric_receipt:AgentProviderInputMetricReceipt",
        "pubconstfninput_metric_receipt(&self)->AgentProviderInputMetricReceipt",
        "pubfninput_metric_receipt(&self)->Option<AgentProviderInputMetricReceipt>",
        "Some(AgentProviderAttemptCore::PreDispatch{committed,..})=>{committed.input_metric_receipt()}",
        "terminal.input.sealed_metric_receipt()",
        "pubfninput_metric_receipt(&self)->AgentProviderInputMetricReceipt{self.input_metric_receipt}",
    ] {
        if !transport.contains(required) {
            return Err(format!(
                "provider transport lost committed input metric receipt propagation {required}"
            ));
        }
    }
    if transport
        .matches("pubfninput_metric_receipt(&self)->Option<AgentProviderInputMetricReceipt>")
        .count()
        != 1
        || transport
            .matches("pubconstfninput_metric_receipt(&self)->AgentProviderInputMetricReceipt")
            .count()
            != 1
        || transport
            .matches("pubfninput_metric_receipt(&self)->AgentProviderInputMetricReceipt{self.input_metric_receipt}")
            .count()
            != 1
    {
        return Err(
            "provider transport must keep provisional receipts optional and expose final receipts only on counted attempts and terminals"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_agent_progress_metrics_contract(
    root: &str,
    metrics: &str,
    audit: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "modagent_progress_metrics;",
        "AgentRunProgressMetrics",
        "AgentRunProgressSnapshot",
        "AgentProgressMetricError",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agent progress metrics lost its default-core export {required}"
            ));
        }
    }

    let metrics = compact(metrics);
    for required in [
        "pubstructAgentRunProgressMetrics",
        "pubfntry_new(manifest:&AgentRunManifest,supervisor:&AgentRunSupervisor",
        "pubfnrecord_event(&mutself,event:AgentAuditEvent)->Result<(),AgentProgressMetricError>",
        "event.matches_manifest_revision(self.manifest,self.manifest_guard,self.supervisor)",
        "pubconstfnsnapshot(&self)->AgentRunProgressSnapshot",
        "try_reserve_exact(topology_nodes.len())",
        "self.active_models.try_reserve_exact(1)",
        "self.active_effects.try_reserve_exact(1)",
        "self.takeover_cancellations.try_reserve_exact(1)",
        "self.active_models.iter().any(|active|active.node==node)||self.active_effects.iter().any(|active|active.node==node)",
        "VectorAction::Insert(index,active)=>self.active_models.insert(index,active)",
        "VectorAction::Insert(index,active)=>self.active_effects.insert(index,active)",
        "MAX_AGENT_PENDING_MODEL_CALLS",
        "MAX_AGENT_PENDING_EFFECTS",
        "MAX_AGENT_PLAN_NODES",
        "pubconstfnqueue_wait(self)->Option<AgentDurationMetrics>",
        "pubconstfnmodel(self)->Option<AgentDurationMetrics>",
        "pubconstfneffect(self)->Option<AgentDurationMetrics>",
        "pubconstfnhuman_wait(self)->Option<AgentDurationMetrics>",
        "pubconstfntotal_elapsed_millis(self)->Option<u64>",
    ] {
        if !metrics.contains(required) {
            return Err(format!(
                "agent progress metrics lost required bounded boundary {required}"
            ));
        }
    }
    for forbidden in [
        "traitAgentProgressMetricPort",
        "implAgentAuditPort",
        "SerializeforAgentRunProgressMetrics",
        "DeserializeforAgentRunProgressMetrics",
        "std::time::Instant",
        "std::time::SystemTime",
        "unreachable!",
    ] {
        if metrics.contains(forbidden) {
            return Err(format!(
                "run-local progress metrics acquired forbidden runtime/telemetry seam {forbidden}"
            ));
        }
    }

    let audit = compact(audit);
    for required in [
        "pub(crate)fnmatches_manifest_revision(self,manifest:AgentRunManifestId,manifest_guard:[u8;32],supervisor:AgentSupervisorId,)->bool",
        "self.progress.manifest()==manifest&&self.progress.supervisor()==supervisor&&self.guard==event_guard(manifest_guard,self.record)",
    ] {
        if !audit.contains(required) {
            return Err(format!(
                "canonical audit events lost their private revision seal {required}"
            ));
        }
    }
    Ok(())
}

fn validate_agent_supervisor_cancellation_finalization(source: &str) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let production = compact(production);
    for required in [
        "self.nodes.iter().enumerate().find_map(|(index,row)|",
        "letNodeState::Cancelling(cancelling)=row.stateelse{returnNone;}",
        ".then_some((index,cancelling))",
        "letSome((index,cancelling))=candidateelse{return;};",
    ] {
        if !production.contains(required) {
            return Err(format!(
                "agent supervisor cancellation finalization lost its proven-state carry {required}"
            ));
        }
    }
    if production.contains("unreachable!") {
        return Err(
            "agent supervisor cancellation finalization regained a process-terminating invariant branch"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_agentic_default_core_panic_boundary(
    semantic: &str,
    wire: &str,
    execution: &str,
    continuation: &str,
    request: &str,
) -> Result<(), String> {
    let semantic = compact(semantic);
    if !semantic.contains("pub(crate)constfninto_nonzero(self)->NonZeroU64{self.0}") {
        return Err("semantic node identity lost its proven nonzero carry".to_owned());
    }

    let execution = compact(execution);
    if !execution.contains("Self(key.into_nonzero())")
        || execution.contains("semanticnodekeysarenonzero")
    {
        return Err("native target identity regained a process-aborting nonzero rewrap".to_owned());
    }

    let wire = compact(wire);
    for required in [
        "letallowed=allowed_operations(role)?;",
        "fnallowed_operations(role:SemanticRole)->Result<SemanticOperations,SemanticDecodeError>",
        "SemanticOperations::try_new(operations).map_err(|_|SemanticDecodeError::NodeContract)",
    ] {
        if !wire.contains(required) {
            return Err(format!(
                "semantic wire operation validation lost fail-closed decoding {required}"
            ));
        }
    }
    if wire.contains("staticoperationsareduplicate-free") {
        return Err("semantic wire operation validation regained an invariant abort".to_owned());
    }

    let continuation = continuation
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(continuation, |(production, _)| production);
    let continuation = compact(continuation);
    for required in [
        "pub(super)structAgentProviderBoundTranscript{prior:AgentProviderTranscript,latest:AgentProviderTranscriptTurn,retained_bytes:usize,}",
        "fntry_bind(mutself,correlation:AgentProviderToolCallCorrelation,tool_result:String,)->Result<AgentProviderBoundTranscript,AgentProviderContinuationError>",
        "self.turns.try_reserve(1).map_err(|_|AgentProviderContinuationError::TranscriptLimit)?;",
        "latest:AgentProviderTranscriptTurn{correlation,tool_result,}",
        "fninto_transcript(self)->AgentProviderTranscript",
        "prior.turns.push(latest);prior.retained_bytes=retained_bytes;prior",
    ] {
        if !continuation.contains(required) {
            return Err(format!(
                "provider continuation lost allocation-free proven latest-turn carry {required}"
            ));
        }
    }
    for (required, expected) in [
        (
            "lettranscript=transcript.try_bind(correlation,tool_result)?;",
            4,
        ),
        ("self.transcript.latest().correlation.id()", 3),
        ("self.transcript.into_transcript()", 4),
    ] {
        if continuation.matches(required).count() != expected {
            return Err(format!(
                "provider continuation does not carry every bound turn structurally {required}"
            ));
        }
    }
    for forbidden in [
        "latest_turn",
        "expect(",
        "unwrap(",
        "unreachable!",
        "panic!",
    ] {
        if continuation.contains(forbidden) {
            return Err(format!(
                "provider continuation regained a process-aborting invariant branch {forbidden}"
            ));
        }
    }

    // Counted in production only; e351769d added a test encoder with the same signature.
    let request = compact(
        request
            .split_once("\n#[cfg(test)]\nmod tests")
            .map_or(request, |(production, _)| production),
    );
    if request
        .matches("transcript:&AgentProviderBoundTranscript")
        .count()
        != 4
        || request
            .matches("transcript.latest().correlation().kind()!=AgentBrowserToolKind::Extract")
            .count()
            != 2
    {
        return Err(
            "provider encoders stopped consuming the structurally nonempty transcript".to_owned(),
        );
    }
    Ok(())
}

fn validate_agent_metric_closure_contract(
    root: &str,
    closure: &str,
    accounting: &str,
    progress: &str,
    actions: &str,
    inputs: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "modagent_metric_closure;",
        "AgentRunMetricClosure",
        "AgentRunMetricClosureError",
        "MAX_AGENT_RUN_METRIC_CLOSURE_BYTES",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agent metric closure lost its default-core export {required}"
            ));
        }
    }

    let closure = compact(closure);
    for required in [
        "pubconstMAX_AGENT_RUN_METRIC_CLOSURE_BYTES:usize=192;",
        "size_of::<AgentRunMetricClosure>()<=MAX_AGENT_RUN_METRIC_CLOSURE_BYTES",
        "pubstructAgentRunMetricClosure{manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "manifest_guard:manifest.guard()",
        "pubfntry_close(",
        "!supervisor.topology().matches_manifest(manifest)",
        "!accounting.matches_metric_scope(manifest,supervisor_id)",
        "!progress.matches_metric_scope(manifest,supervisor_id)",
        "!actions.matches_metric_scope(manifest,supervisor_id)",
        "!inputs.matches_metric_scope(manifest,supervisor_id)",
        "status.is_sealed()",
        "status.terminal()!=status.activated()",
        "progress_snapshot.activated_nodes()!=activated_nodes",
        "progress_snapshot.terminal_nodes()!=terminal_nodes",
        "accounting.model_receipt_ids()!=inputs.receipt_ids()",
        "accounting.effect_receipt_ids()!=actions.effect_receipt_ids()",
        "accounting.effect_attempt_ids()!=actions.effect_attempt_ids()",
        "model_duration_samples!=model.calls()",
        "effect_duration_samples!=effects.attempts()",
        "AgentRunMetricClosureError::ModelCoverage",
        "AgentRunMetricClosureError::EffectCoverage",
        "field(\"authority\",&\"[none]\")",
        "field(\"content\",&\"[redacted]\")",
    ] {
        if !closure.contains(required) {
            return Err(format!(
                "agent metric closure lost required terminal coverage {required}"
            ));
        }
    }
    for forbidden in [
        "pubfnnew(",
        "pubconstfnnew(",
        "pubfnmanifest_guard(",
        "pubconstfnmanifest_guard(",
        "traitAgentRunMetricClosurePort",
        "SerializeforAgentRunMetricClosure",
        "DeserializeforAgentRunMetricClosure",
        "HashMap<",
        "BTreeMap<",
        "std::thread",
        "std::time",
        "std::fs",
        "tokio::",
        "Mutex<",
        "Arc<",
        "nativehandle",
    ] {
        if closure.contains(forbidden) {
            return Err(format!(
                "agent metric closure acquired forbidden authority/runtime seam {forbidden}"
            ));
        }
    }

    for (name, source, required) in [
        (
            "accounting",
            accounting,
            [
                "pub(crate)fnmatches_metric_scope(",
                "pub(crate)fnmodel_receipt_ids(&self)->&[AgentModelCallId]",
                "pub(crate)fneffect_receipt_ids(&self)->&[AgentEffectId]",
                "pub(crate)fneffect_attempt_ids(&self)->&[crate::SemanticActionAttemptId]",
            ],
        ),
        (
            "action",
            actions,
            [
                "pub(crate)fnmatches_metric_scope(",
                "pub(crate)fneffect_receipt_ids(&self)->&[AgentEffectId]",
                "pub(crate)fneffect_attempt_ids(&self)->&[SemanticActionAttemptId]",
                "self.manifest_guard==manifest.guard()",
            ],
        ),
        (
            "provider-input",
            inputs,
            [
                "pub(crate)fnmatches_metric_scope(",
                "pub(crate)fnreceipt_ids(&self)->&[AgentModelCallId]",
                "self.manifest_guard==manifest.guard()",
                "self.supervisor==supervisor",
            ],
        ),
        (
            "progress",
            progress,
            [
                "pub(crate)fnmatches_metric_scope(",
                "self.manifest==manifest.id()",
                "self.manifest_guard==manifest.guard()",
                "self.supervisor==supervisor",
            ],
        ),
    ] {
        let source = compact(source);
        for required in required {
            if !source.contains(required) {
                return Err(format!(
                    "agent metric closure lost {name} private join {required}"
                ));
            }
        }
    }
    Ok(())
}

fn validate_agent_policy_settlement_contract(
    root: &str,
    closure: &str,
    accounting: &str,
    audit: &str,
    policy: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "AgentRunPolicySettlement",
        "AgentRunPolicySettlementError",
        "AgentRunPolicySettlementRefusal",
        "MAX_AGENT_RUN_POLICY_SETTLEMENT_BYTES",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "clean agent policy settlement lost its default-core export {required}"
            ));
        }
    }

    let closure = compact(closure);
    for required in [
        "pub(crate)fnmatches_manifest_revision(&self,manifest:&AgentRunManifest)->bool",
        "self.manifest==manifest.id()&&self.manifest_guard==manifest.guard()",
    ] {
        if !closure.contains(required) {
            return Err(format!(
                "clean agent policy settlement lost its private closure join {required}"
            ));
        }
    }
    for forbidden in [
        "pubfnmatches_manifest_revision(",
        "pubconstfnmatches_manifest_revision(",
    ] {
        if closure.contains(forbidden) {
            return Err(format!(
                "metric closure exposed its private revision join {forbidden}"
            ));
        }
    }

    let accounting = compact(accounting);
    for required in [
        "pub(crate)fnmatches_metric_scope(",
        "self.manifest==manifest.id()",
        "self.manifest_guard==manifest.guard()",
        "self.supervisor==supervisor",
        "pubfnnodes(&self)->implExactSizeIterator<Item=AgentNodeAccountingMetrics>+'_",
    ] {
        if !accounting.contains(required) {
            return Err(format!(
                "clean agent policy settlement lost its accounting join {required}"
            ));
        }
    }

    let audit = compact(audit);
    for required in [
        "pubstructAgentAuditLedger{manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "pub(crate)fnmatches_run_scope(&self,manifest:&AgentRunManifest,supervisor:AgentSupervisorId,)->bool",
        "self.manifest==manifest.id()&&self.manifest_guard==manifest.guard()&&self.supervisor==supervisor",
        "pubfnis_quiescent(&self)->bool",
        "self.shutdown_sealed&&self.events.is_empty()&&self.in_flight.is_none()",
    ] {
        if !audit.contains(required) {
            return Err(format!(
                "clean agent policy settlement lost its durable-audit join {required}"
            ));
        }
    }
    for forbidden in [
        "pubfnmatches_run_scope(",
        "pubconstfnmatches_run_scope(",
        "implCloneforAgentAuditLedger",
    ] {
        if audit.contains(forbidden) {
            return Err(format!(
                "durable audit ledger exposed or duplicated terminal authority {forbidden}"
            ));
        }
    }

    let policy = compact(policy);
    for required in [
        "pubconstMAX_AGENT_RUN_POLICY_SETTLEMENT_BYTES:usize=256;",
        "size_of::<AgentRunPolicySettlement>()<=MAX_AGENT_RUN_POLICY_SETTLEMENT_BYTES",
        // 1783cf12 adds one derived fact: every settled model call had exact usage.
        "pubstructAgentRunPolicySettlement{closure:AgentRunMetricClosure,accounting:AgentPolicyAccounting,model_usage_exact:bool,}",
        "model_usage_exact:metrics.snapshot().model().exact()==metrics.snapshot().model().calls(),",
        "pubstructAgentRunPolicySettlementRefusal{error:AgentRunPolicySettlementError,policy:AgentRunPolicy,audit:AgentAuditLedger,}",
        "pubconstfnaudit(&self)->&AgentAuditLedger{&self.audit}",
        "pubfninto_parts(self)->(AgentRunPolicy,AgentAuditLedger){(self.policy,self.audit)}",
        "pubfnsettle_metric_closure(self,closure:AgentRunMetricClosure,metrics:&AgentRunAccountingMetrics,audit:AgentAuditLedger,)->Result<AgentRunPolicySettlement,Box<AgentRunPolicySettlementRefusal>>",
        "matchvalidate_metric_settlement(&self,closure,metrics,&audit)",
        "policy:self",
        "audit,",
        "!closure.matches_manifest_revision(policy.manifest())",
        "!metrics.matches_metric_scope(policy.manifest(),closure.supervisor())",
        "snapshot.operations()!=closure.operations()",
        "snapshot.model().calls()!=closure.model_calls()",
        "snapshot.effects().attempts()!=closure.effects()",
        "ifpolicy.is_sealed()",
        "policy.pending_model_calls()!=0",
        "policy.pending_effects()!=0",
        "policy.pending_origin_writes()!=0",
        "accounting.reserved_operations()!=0",
        "accounting.reserved_model_tokens()!=0",
        "accounting.reserved_cost_micro_usd()!=0",
        ".input_tokens().checked_add(snapshot.model().output_tokens())",
        "accounting.consumed_operations()!=snapshot.operations()",
        "accounting.consumed_model_tokens()!=model_tokens",
        "accounting.consumed_cost_micro_usd()!=snapshot.model().cost_micro_usd()",
        "policy.leases.len()!=metrics.nodes().len()",
        "for(lease,node)inpolicy.leases.iter().zip(metrics.nodes())",
        "lease.binding.node()!=node.node()",
        "lease_accounting.reserved_operations()!=0",
        "lease_accounting.reserved_model_tokens()!=0",
        "lease_accounting.reserved_cost_micro_usd()!=0",
        "lease_accounting.consumed_operations()!=node.operations()",
        "lease_accounting.consumed_model_tokens()!=node.model_tokens()",
        "lease_accounting.consumed_cost_micro_usd()!=node.cost_micro_usd()",
        "AgentRunPolicySettlementError::Authority",
        "AgentRunPolicySettlementError::Sealed",
        "AgentRunPolicySettlementError::Pending",
        "AgentRunPolicySettlementError::Accounting",
        "AgentRunPolicySettlementError::Overflow",
        "!audit.matches_run_scope(policy.manifest(),closure.supervisor())",
        "AgentRunPolicySettlementError::AuditAuthority",
        "ifaudit_status.fail_stopped()",
        "AgentRunPolicySettlementError::AuditFailStopped",
        "if!audit_status.shutdown_sealed()",
        "AgentRunPolicySettlementError::AuditUnsealed",
        "!audit.is_quiescent()",
        "audit_status.pending()!=0",
        "audit_status.in_flight()!=0",
        "AgentRunPolicySettlementError::AuditPending",
        "audit_status.committed()!=closure.events()",
        "AgentRunPolicySettlementError::AuditCoverage",
        "field(\"audit\",&self.audit.status())",
        "field(\"authority\",&\"[none]\")",
        "field(\"authority\",&\"[retained]\")",
        "field(\"content\",&\"[redacted]\")",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "clean agent policy settlement lost required invariant {required}"
            ));
        }
    }
    for forbidden in [
        "implAgentRunPolicySettlement{pubfnnew(",
        "implAgentRunPolicySettlement{pubconstfnnew(",
        "traitAgentRunPolicySettlementPort",
        "SerializeforAgentRunPolicySettlement",
        "DeserializeforAgentRunPolicySettlement",
        "SerializeforAgentRunPolicySettlementRefusal",
        "DeserializeforAgentRunPolicySettlementRefusal",
    ] {
        if policy.contains(forbidden) {
            return Err(format!(
                "clean agent policy settlement acquired forbidden authority/runtime seam {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_provider_transport_manifest(source: &str) -> Result<(), String> {
    let manifest: toml::Value = toml::from_str(source)
        .map_err(|error| format!("cannot parse {PROVIDER_TRANSPORT_MANIFEST}: {error}"))?;
    if manifest
        .get("package")
        .and_then(|package| package.get("publish"))
        .and_then(toml::Value::as_bool)
        != Some(false)
    {
        return Err(
            "agent provider transport must remain an unpublished internal crate".to_owned(),
        );
    }
    let reqwest = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("reqwest"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "agent provider transport reqwest dependency is missing".to_owned())?;
    if reqwest.get("version").and_then(toml::Value::as_str) != Some("=0.13.4")
        || reqwest
            .get("default-features")
            .and_then(toml::Value::as_bool)
            != Some(false)
        || reqwest.get("optional").and_then(toml::Value::as_bool) != Some(true)
    {
        return Err("agent provider transport reqwest pin or default features drifted".to_owned());
    }
    let features = reqwest
        .get("features")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "agent provider transport reqwest features are missing".to_owned())?;
    let actual = features
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let expected = ["http2", "rustls", "stream", "system-proxy"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    if actual != expected || actual.len() != features.len() {
        return Err("agent provider transport reqwest feature graph drifted".to_owned());
    }
    let tokio = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("tokio"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "agent provider transport Tokio dependency is missing".to_owned())?;
    if tokio.get("workspace").and_then(toml::Value::as_bool) != Some(true) {
        return Err("agent provider transport Tokio workspace pin drifted".to_owned());
    }
    let features = tokio
        .get("features")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "agent provider transport Tokio features are missing".to_owned())?;
    let actual = features
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let expected = ["net", "sync", "time"].into_iter().collect::<BTreeSet<_>>();
    if actual != expected || actual.len() != features.len() {
        return Err("agent provider transport Tokio feature graph drifted".to_owned());
    }
    if tokio.get("optional").and_then(toml::Value::as_bool) != Some(true) {
        return Err("agent provider transport Tokio dependency must remain optional".to_owned());
    }
    let macos_dependencies = manifest
        .get("target")
        .and_then(|targets| targets.get("cfg(target_os = \"macos\")"))
        .and_then(|target| target.get("dependencies"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "agent provider transport macOS dependency table is missing".to_owned())?;
    for (name, version) in [
        ("security-framework", "=3.7.0"),
        ("security-framework-sys", "=2.17.0"),
    ] {
        let dependency = macos_dependencies
            .get(name)
            .and_then(toml::Value::as_table)
            .ok_or_else(|| format!("agent provider transport dependency {name} is missing"))?;
        if dependency.get("version").and_then(toml::Value::as_str) != Some(version)
            || dependency.get("optional").and_then(toml::Value::as_bool) != Some(true)
        {
            return Err(format!(
                "agent provider transport dependency {name} must remain pinned and optional"
            ));
        }
    }
    Ok(())
}

fn validate_provider_transport_facade(manifest: &str, root: &str) -> Result<(), String> {
    let manifest: toml::Value = toml::from_str(manifest)
        .map_err(|error| format!("cannot parse {PROVIDER_TRANSPORT_FACADE_MANIFEST}: {error}"))?;
    if manifest
        .get("package")
        .and_then(|package| package.get("publish"))
        .and_then(toml::Value::as_bool)
        != Some(false)
    {
        return Err("agent provider transport façade must remain unpublished".to_owned());
    }
    let features = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "agent provider transport façade features are missing".to_owned())?;
    if features
        .get("default")
        .and_then(toml::Value::as_array)
        .is_none_or(|default| !default.is_empty())
        || features
            .get("provider-transport")
            .and_then(toml::Value::as_array)
            .is_none_or(|feature| {
                feature.as_slice()
                    != [toml::Value::String(
                        "zephium-agentic/provider-transport".to_owned(),
                    )]
            })
        || features
            .get("probe-harness")
            .and_then(toml::Value::as_array)
            .is_none_or(|feature| {
                feature.as_slice()
                    != [
                        toml::Value::String("provider-transport".to_owned()),
                        toml::Value::String("zephium-agentic/probe-harness".to_owned()),
                    ]
            })
        || features.len() != 3
    {
        return Err(
            "agent provider transport façade must preserve its opt-in feature graph".to_owned(),
        );
    }
    let dependency = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("zephium-agentic"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "agent provider transport façade dependency is missing".to_owned())?;
    if dependency.get("workspace").and_then(toml::Value::as_bool) != Some(true)
        || dependency.len() != 1
    {
        return Err(
            "agent provider transport façade must not activate runtime authority implicitly"
                .to_owned(),
        );
    }

    let root = compact(root);
    for required in [
        "#[cfg(all(feature=\"probe-harness\",not(debug_assertions)))]compile_error!(\"theprovidertransportprobeharnessisforbiddeninoptimizedbuilds\");",
        "#[cfg(feature=\"provider-transport\")]pubusezephium_agentic::{",
        "#[cfg(feature=\"probe-harness\")]pubusezephium_agentic::{exact_loopback_url,ProviderEndpoints};",
        "AgentProviderAbortReason,",
        "AgentProviderAttemptStateError,",
        "AgentProviderTransport,",
        "AgentProviderPolicySettlement,",
        "AgentProviderImmediateSettlementError,",
        "AgentProviderSettledTerminal,",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agent provider transport façade lost safe re-export {required}"
            ));
        }
    }
    for forbidden in [
        "reqwest::",
        "AgentCommittedProviderRequest",
        "AgentProviderFinishedStream",
        "AgentProviderStreamDecoder",
        "AgentProviderContinuationSeed",
        "fnfinish(",
        "fncommit(",
    ] {
        if root.contains(forbidden) {
            return Err(format!(
                "agent provider transport façade acquired raw authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_provider_billing_contract(
    root: &str,
    request: &str,
    openai: &str,
    anthropic: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "constOPENAI_STANDARD_SERVICE_TIER:&str=\"default\";",
        "constANTHROPIC_STANDARD_SERVICE_TIER_REQUEST:&str=\"standard_only\";",
        "constANTHROPIC_STANDARD_SERVICE_TIER_RESPONSE:&str=\"standard\";",
        "constANTHROPIC_GLOBAL_INFERENCE_GEO:&str=\"global\";",
        "pubenumAgentProviderBillingClass",
        "pubenumAgentProviderResponseRoute",
        "catalog:Arc<AgentProviderCatalogBinding>",
        "pub(incrate::agent_provider)fnfrom_catalog(",
        "pubfnbilling_class(&self)->AgentProviderBillingClass",
        "pubfnpricing_profile(&self)->AgentProviderPricingProfile",
        "pubfnresponse_route(&self)->AgentProviderResponseRoute",
        "pubfnaccounting_guard(&self)->[u8;32]",
        "!self.pricing_profile().contains(total_input_tokens)",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agent provider billing identity lost required boundary {required}"
            ));
        }
    }

    let request = compact(request);
    for required in [
        "service_tier:config.response_route().request_service_tier()",
        "inference_geo:config.response_route().response_inference_geo()",
    ] {
        if !request.contains(required) {
            return Err(format!(
                "agent provider request lost fixed billing control {required}"
            ));
        }
    }
    if request.contains("speed:&'staticstr") {
        return Err(
            "stable Anthropic Messages request must not acquire beta-only speed control".to_owned(),
        );
    }

    let openai = compact(openai);
    for required in [
        "event.response.service_tier!=self.config.response_route().response_service_tier()",
        "AgentProviderResponseIdentity::try_attested(&self.config,event.response.model,event.response.service_tier,None,)",
    ] {
        if !openai.contains(required) {
            return Err(format!(
                "OpenAI decoder lost catalog-bound response attestation {required}"
            ));
        }
    }

    let anthropic = compact(anthropic);
    for required in [
        "AgentProviderResponseIdentity::try_attested(&self.config,event.message.model,event.message.usage.service_tier,Some(event.message.usage.inference_geo),)",
    ] {
        if !anthropic.contains(required) {
            return Err(format!(
                "Anthropic decoder lost catalog-bound response attestation {required}"
            ));
        }
    }
    Ok(())
}

fn validate_provider_pricing_contract(
    pricing: &str,
    policy: &str,
    settlement: &str,
) -> Result<(), String> {
    let pricing = compact(pricing);
    for required in [
        "pubstructAgentProviderPricingSchedule{binding:Arc<AgentProviderCatalogBinding>",
        "fnexact_binding_matches(&self,config:&AgentProviderCallConfig)->bool",
        "config.accounting_guard()==self.binding.guard",
        "config.catalog.as_ref()==self.binding.as_ref()",
        "!self.binding.matches_response_identity(identity)",
        "!self.binding.profile.contains(usage.input_tokens())",
        ".checked_add(usage.cache_write_input_tokens())",
        ".checked_sub(priced_input_subsets)",
        "u64::try_from(rounded)",
        "pub(crate)structAgentProviderPricedUsage",
        "pubstructAgentProviderPricingAttribution",
        "schedule_guard:[u8;32]",
        "response_identity_guard:[u8;32]",
    ] {
        if !pricing.contains(required) {
            return Err(format!(
                "agent provider pricing lost required checked boundary {required}"
            ));
        }
    }

    let policy = compact(policy);
    for required in [
        "PricedCeiling",
        "pub(crate)fnsettle_model_call_priced(",
        "pub(crate)fnprevalidate_active_model_call(",
        "AgentModelUsageAccounting::PricedCeiling",
        "pricing_attribution:Option<AgentProviderPricingAttribution>",
        "pricing_attribution:Some(pricing_attribution)",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "agent policy lost priced-ceiling settlement boundary {required}"
            ));
        }
    }

    let settlement = compact(settlement);
    for required in [
        "schedule:&AgentProviderPricingSchedule",
        "policy.prevalidate_active_model_call(&self.active)",
        "!schedule.exact_binding_matches(&self.config)",
        "schedule.try_price(&self.config,self.identity,self.usage)",
        ".settle_model_call_priced(active,settlement,priced)",
        "pubfnsettle_at_reservation_ceiling(",
        "AgentModelCallUnaccountedSettlement::ProviderFailed",
        "unsettled:Box<AgentProviderPricingSettlement>",
        "pubfninto_unsettled(self)->Option<AgentProviderPricingSettlement>",
    ] {
        if !settlement.contains(required) {
            return Err(format!(
                "agent provider settlement lost price-authority join {required}"
            ));
        }
    }
    if settlement.contains("pubfnsettle(self,policy:&mutAgentRunPolicy,cost_micro_usd:u64)") {
        return Err("agent provider settlement regained an unbound raw-cost handoff".to_owned());
    }
    Ok(())
}

fn validate_provider_transport_root(source: &str) -> Result<(), String> {
    let source = compact(source);
    for required in [
        "constOPENAI_RESPONSES_URL:&str=\"https://api.openai.com/v1/responses\";",
        "constANTHROPIC_MESSAGES_URL:&str=\"https://api.anthropic.com/v1/messages\";",
        "fnexact_production_url(url:&Url,host:&str,path:&str)->bool",
        "pubstructProviderEndpoints{",
        "#[cfg(any(test,feature=\"probe-harness\"))]pubfnloopback(",
        "openai:&str,anthropic:&str",
        "openai_input_tokens.path_segments_mut().map_err(|_|AgentProviderTransportConfigError::Endpoint)?.pop_if_empty().push(\"input_tokens\");",
        "#[cfg(any(test,feature=\"probe-harness\"))]pubfnexact_loopback_url(url:&str)->bool",
        "fnparse_exact_loopback_url(source:&str)->Option<Url>",
        "url.as_str()==source",
        "fnexact_loopback_path(path:&str)->bool",
        "path.split('/').all(|segment|",
        "url.scheme()==\"http\"&&url.host_str()==Some(\"127.0.0.1\")&&url.port().is_some_and(|port|port!=0)",
        "byte.is_ascii_alphanumeric()||matches!(byte,b'-'|b'.'|b'_'|b'~')",
        "#[cfg(any(test,feature=\"probe-harness\"))]pubfntry_new_loopback(",
        "Self::try_new_with_endpoints(config,ProviderEndpoints::loopback(openai,anthropic)?)",
        "#[cfg(any(test,feature=\"probe-harness\"))]letbuilder=ifendpoints.https_only{builder}else{builder.no_proxy()};",
        ".https_only(endpoints.https_only)",
        ".redirect(Policy::none())",
        ".referer(false)",
        ".retry(reqwest::retry::never())",
        ".pool_max_idle_per_host(0)",
        ".header(ACCEPT_ENCODING,HeaderValue::from_static(\"identity\"))",
        "value.set_sensitive(true)",
        "pubenumAgentProviderUsageKnowledge",
        "ExactZeroBeforeModelDispatch",
        "UnknownAfterDispatch",
        "pubfninto_policy_settlement(self)->AgentProviderPolicySettlement",
        "ifself.cancellation.is_cancelled()||self.shutdown.is_cancelled()",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent provider transport lost required fixed boundary {required}"
            ));
        }
    }
    for forbidden in [
        "danger_accept_invalid_certs",
        "danger_accept_invalid_hostnames",
        "pubfntry_new_with_endpoints",
        "pubfninto_parts",
        "pubfnexact_loopback_url(url:&Url)",
        "pubfnloopback(openai:Url,anthropic:Url)",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "agent provider transport exposes forbidden authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_provider_transport_commit_boundary(source: &str) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let source = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "commit_gate:Mutex<()>,",
        "commit_gate:Mutex::new(()),",
        "let_commit_gate=self.inner.commit_gate.lock().unwrap_or_else(std::sync::PoisonError::into_inner);self.publish_cancellation()",
        "fnlock_commit_gate(&self)->Option<MutexGuard<'_,()>>",
        "Err(poisoned)=>{letguard=poisoned.into_inner();self.publish_cancellation();drop(guard);None}",
        "fnshares_authority(&self,other:&Self)->bool{Arc::ptr_eq(&self.inner,&other.inner)}",
        "letcancellation_gate=matchcancellation.lock_commit_gate()",
        "letshutdown_gate=ifcancellation.shares_authority(&self.shared.shutdown)",
        "matchself.shared.shutdown.lock_commit_gate()",
        "ifcancellation.is_cancelled()||self.shared.shutdown.is_cancelled()",
        "letcommitted=ifletSome(fence)=&image_fence{",
        "letcurrent=fence.while_current(&mut||{ifletSome(input)=owned.take(){committed=Some(input.commit(policy));}});",
        "}else{input.commit(policy)};letcommitted=matchcommitted{",
        "slot.mark_committed();drop(shutdown_gate);drop(cancellation_gate);",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent provider transport lost linearized disclosure commit boundary {required}"
            ));
        }
    }
    if source.contains("fntry_claim_commit") {
        return Err(
            "agent provider transport regained a racy cancellation load before disclosure commit"
                .to_owned(),
        );
    }

    let run_gate = source
        .find("letcancellation_gate=matchcancellation.lock_commit_gate()")
        .ok_or_else(|| "agent provider transport run commit gate is missing".to_owned())?;
    let shutdown_gate = source
        .find("letshutdown_gate=ifcancellation.shares_authority(&self.shared.shutdown)")
        .ok_or_else(|| "agent provider transport shutdown commit gate is missing".to_owned())?;
    let final_check = source
        .find("ifcancellation.is_cancelled()||self.shared.shutdown.is_cancelled()")
        .ok_or_else(|| "agent provider transport final cancellation check is missing".to_owned())?;
    let commit = source
        .find("letcommitted=ifletSome(fence)=&image_fence{")
        .ok_or_else(|| "agent provider transport disclosure commit is missing".to_owned())?;
    let marked = source
        .find("slot.mark_committed();drop(shutdown_gate);drop(cancellation_gate);")
        .ok_or_else(|| "agent provider transport committed slot ordering is missing".to_owned())?;
    if !(run_gate < shutdown_gate
        && shutdown_gate < final_check
        && final_check < commit
        && commit < marked)
    {
        return Err(
            "agent provider transport must hold both cancellation gates through disclosure and slot commitment"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_abortable_provider_operation_contract(source: &str) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let source = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "pubstructAgentProviderAttempt{",
        "core:Option<AgentProviderAttemptCore>,",
        "phase:AgentProviderAttemptPhase,",
        "credential:Option<AgentProviderAttemptCredential>,",
        "enumAgentProviderAttemptCore{PreDispatch{committed:Box<AgentCommittedProviderRequest>,slot:Option<AgentProviderSlot>,},Generation(Box<AgentProviderTerminalCore>),}",
        "enumAgentProviderAttemptPhase{Ready,CountDriving,Counted(AgentProviderExactInputCount),GenerationDriving,Terminal,}",
        "pubasyncfncount_openai_input_tokens(&mutself)->AgentProviderExactCountOutcome<'_>",
        "self.phase=AgentProviderAttemptPhase::CountDriving;",
        "self.mark_input_count_disclosed();",
        "pubasyncfnexecute_anthropic<F>(&mutself,",
        "pubasyncfnexecute_openai_exact_local<F>(&mutself,",
        "self.phase=AgentProviderAttemptPhase::GenerationDriving;",
        "self.mark_model_may_have_dispatched();",
        "pubfnabort(mutself,reason:AgentProviderAbortReason,)->Result<AgentProviderTransportResult,AgentProviderAttemptStateError>",
        "self.finish_terminal(outcome)",
        "implDropforAgentProviderAttempt{fndrop(&mutself){ifself.core.is_some(){self.fail_stop();self.phase=AgentProviderAttemptPhase::Terminal;}}}",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent provider abortable operation lost required ownership rule {required}"
            ));
        }
    }
    for forbidden in [
        "pubasyncfncount_openai_input_tokens(self",
        "pubasyncfnexecute_anthropic<F>(self",
        "pubasyncfnexecute_openai_exact_local<F>(self",
        "#[cfg(any())]",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "agent provider abortable operation regained consuming or dormant drive {forbidden}"
            ));
        }
    }

    let count_start = source
        .find("pubasyncfncount_openai_input_tokens")
        .ok_or_else(|| "agent provider abortable count drive is missing".to_owned())?;
    let abort_start = source[count_start..]
        .find("pubfnabort(mutself")
        .map(|offset| count_start + offset)
        .ok_or_else(|| "agent provider abortable terminal recovery is missing".to_owned())?;
    let count = &source[count_start..abort_start];
    let count_driving = count
        .find("self.mark_count_drive_started()")
        .ok_or_else(|| "count drive no longer publishes its exclusive phase".to_owned())?;
    let count_disclosure = count
        .find("self.mark_input_count_disclosed()")
        .ok_or_else(|| "count drive no longer publishes disclosure before send".to_owned())?;
    // 697c56d0: the free count call retries once on overload with a cloned request.
    let count_send = count
        .find("response=lead::image_fence::fenced(image_fence.as_ref(),send.send())=>matchresponse{Ok(response)=>response,Err(())=>returnself.counted_failure(cancelled_failure())}")
        .ok_or_else(|| "count drive send boundary is missing".to_owned())?;
    if !count.contains("letSome(send)=request.try_clone()else{")
        || !count
            .contains("letretry=retries==0&&failure.class()==AgentProviderFailureClass::Overloaded")
    {
        return Err("count drive retry is no longer single and overload-only".to_owned());
    }
    if !(count_driving < count_disclosure && count_disclosure < count_send) {
        return Err(
            "count drive must publish phase then disclosure before its first send poll".to_owned(),
        );
    }

    let generation_start = source
        .find("asyncfnexecute_model<F>")
        .ok_or_else(|| "agent provider abortable generation drive is missing".to_owned())?;
    let generation_end = source[generation_start..]
        .find("implDropforAgentProviderAttempt")
        .map(|offset| generation_start + offset)
        .ok_or_else(|| "agent provider abortable generation drive is unclosed".to_owned())?;
    let generation = &source[generation_start..generation_end];
    let generation_begin = generation
        .find("=self.begin_generation_drive()?")
        .ok_or_else(|| "generation drive no longer moves its body before polling".to_owned())?;
    let generation_disclosure = generation
        .find("self.mark_model_may_have_dispatched()")
        .ok_or_else(|| "generation drive no longer publishes dispatch before send".to_owned())?;
    let generation_send = generation
        .find("response=lead::image_fence::fenced(image_fence.as_ref(),request.send())=>matchresponse{Ok(response)=>response,Err(())=>returnself.finish_terminal(cancelled_failure())}")
        .ok_or_else(|| "generation drive send boundary is missing".to_owned())?;
    if !(generation_begin < generation_disclosure && generation_disclosure < generation_send) {
        return Err(
            "generation drive must move authority then publish dispatch before its first send poll"
                .to_owned(),
        );
    }

    let abort_end = source[abort_start..]
        .find("pubfncancel_without_dispatch")
        .map(|offset| abort_start + offset)
        .ok_or_else(|| "agent provider abortable recovery is unclosed".to_owned())?;
    let abort = &source[abort_start..abort_end];
    if abort.contains("prior_disclosure_stage=")
        || abort.contains("mark_input_count_disclosed()")
        || abort.contains("mark_model_may_have_dispatched()")
    {
        return Err(
            "abort must preserve private disclosure evidence rather than rewrite accounting"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_immediate_provider_settlement_recovery_contract(source: &str) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let source = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "pubstructAgentProviderImmediateSettlement{",
        "pubfnsettle(self,policy:&mutAgentRunPolicy,)->Result<AgentModelCallReceipt,AgentProviderImmediateSettlementError>{",
        "ifletErr(error)=policy.prevalidate_active_model_call(&self.active){returnErr(AgentProviderImmediateSettlementError::PolicyPrecondition{error,unsettled:Box::new(self),});}",
        "letSelf{active,accounting,..}=self;",
        ".map_err(AgentProviderImmediateSettlementError::Policy)",
        "pubenumAgentProviderImmediateSettlementError{",
        "PolicyPrecondition{error:AgentPolicyError,unsettled:Box<AgentProviderImmediateSettlement>,}",
        "Policy(#[source]AgentPolicyError),",
        "pubconstfnpolicy_precondition_error(&self)->Option<AgentPolicyError>",
        "pubfninto_unsettled(self)->Option<AgentProviderImmediateSettlement>",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent provider immediate settlement lost recoverable policy boundary {required}"
            ));
        }
    }
    let settle_start = source
        .find("pubfnsettle(self,policy:&mutAgentRunPolicy,)->Result<AgentModelCallReceipt,AgentProviderImmediateSettlementError>")
        .ok_or_else(|| "immediate settlement entry point is missing".to_owned())?;
    let error_start = source
        .find("pubenumAgentProviderImmediateSettlementError")
        .ok_or_else(|| "immediate settlement error is missing".to_owned())?;
    let settle = &source[settle_start..error_start];
    let prevalidate = settle
        .find("policy.prevalidate_active_model_call(&self.active)")
        .ok_or_else(|| "immediate settlement prevalidation is missing".to_owned())?;
    let destructure = settle
        .find("letSelf{active,accounting,..}=self;")
        .ok_or_else(|| "immediate settlement destructures before validation".to_owned())?;
    let policy = settle
        .find(".map_err(AgentProviderImmediateSettlementError::Policy)")
        .ok_or_else(|| "immediate settlement lost terminal policy mapping".to_owned())?;
    if !(prevalidate < destructure && destructure < policy) {
        return Err(
            "immediate settlement must retain the owner until policy prevalidation succeeds"
                .to_owned(),
        );
    }
    let error_derive_start = source[..error_start]
        .rfind("#[derive(")
        .ok_or_else(|| "immediate settlement error derive is missing".to_owned())?;
    let error_end = source[error_start..]
        .find("implAgentProviderImmediateSettlementError")
        .map(|offset| error_start + offset)
        .ok_or_else(|| "immediate settlement error implementation is missing".to_owned())?;
    if source[error_derive_start..error_end].contains("Clone") {
        return Err("immediate settlement recovery error must remain move-only".to_owned());
    }
    Ok(())
}

fn validate_provider_consumer_panic_boundary(
    transport: &str,
    provider_contract: &str,
) -> Result<(), String> {
    let production = transport
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(transport, |(production, _)| production);
    let transport = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "fnfail_stop(&self){self.shared.shutdown.cancel();matchself.shared.state.lock(){Ok(mutstate)=>state.sealed=true,Err(poisoned)=>poisoned.into_inner().sealed=true,}}",
        "matchstd::panic::catch_unwind(std::panic::AssertUnwindSafe(||consume(batch)))",
        "fnfail_stop(&self){ifletSome(slot)=self.slot(){slot.fail_stop();}else{self.cancellation.cancel();self.shutdown.cancel();}}",
        "Err(_)=>{self.fail_stop();returnself.finish_terminal(integration_failure());}",
        "fnintegration_failure()->AgentProviderTransportOutcome",
        "AgentProviderFailureClass::Integration",
    ] {
        if !transport.contains(required) {
            return Err(format!(
                "agent provider consumer panic boundary lost required fail-stop rule {required}"
            ));
        }
    }
    if transport.matches("std::panic::catch_unwind").count() != 1
        || transport.contains("resume_unwind")
    {
        return Err(
            "agent provider transport must contain exactly the normalized batch consumer panic"
                .to_owned(),
        );
    }

    let provider_contract = compact(provider_contract);
    for required in ["Integration,", "|Self::Integration"] {
        if !provider_contract.contains(required) {
            return Err(format!(
                "agent provider integration failure lost non-retryable contract {required}"
            ));
        }
    }
    Ok(())
}

fn validate_provider_transport_response_header_boundary(source: &str) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let source = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "pubconstMAX_AGENT_PROVIDER_RESPONSE_HEADER_BYTES:u32=64*1_024;",
        "constHTTP_HEADER_FIELD_OVERHEAD_BYTES:usize=32;",
        ".http2_max_header_list_size(MAX_AGENT_PROVIDER_RESPONSE_HEADER_BYTES)",
        "fnresponse_headers_admitted(headers:&HeaderMap)->bool",
        ".checked_add(name.as_str().len())?",
        ".checked_add(value.as_bytes().len())?",
        ".checked_add(HTTP_HEADER_FIELD_OVERHEAD_BYTES)",
        "usize::try_from(MAX_AGENT_PROVIDER_RESPONSE_HEADER_BYTES)",
        "!response_headers_admitted(response.headers())",
        "fnresponse_content_length_admitted(headers:&HeaderMap,max_wire_bytes:u32)->bool",
        "headers.get_all(CONTENT_LENGTH).iter()",
        "declared<=u64::from(max_wire_bytes)",
        "if!response_content_length_admitted(response.headers(),config.stream_budget().max_wire_bytes(),)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent provider transport lost decoded response-header boundary {required}"
            ));
        }
    }
    let count_start = source
        .find("pubasyncfncount_openai_input_tokens")
        .ok_or_else(|| "provider input-token count phase is missing".to_owned())?;
    let model_start = source
        .find("asyncfnexecute_model")
        .ok_or_else(|| "provider model execution phase is missing".to_owned())?;
    if count_start >= model_start {
        return Err("provider input counting must precede model dispatch".to_owned());
    }
    let count = &source[count_start..model_start];
    let model = &source[model_start..];
    let count_header_check = count
        .find("!response_headers_admitted(response.headers())")
        .ok_or_else(|| "provider count response-header admission is missing".to_owned())?;
    let count_content_length = count
        .find("!response_content_length_admitted(response.headers(),MAX_OPENAI_INPUT_TOKEN_RESPONSE_BYTESasu32,)")
        .ok_or_else(|| "provider count response content-length admission is missing".to_owned())?;
    // 697c56d0: only an admitted OK leaves the bounded retry loop.
    let count_status = count
        .find("ifresponse.status()==StatusCode::OK{breakresponse;}")
        .ok_or_else(|| "provider count response status handling is missing".to_owned())?;
    let count_body = count
        .find("letmutstream=response.bytes_stream()")
        .ok_or_else(|| "provider count response body stream is missing".to_owned())?;
    if !(count_header_check < count_content_length
        && count_content_length < count_status
        && count_status < count_body)
    {
        return Err(
            "provider count headers and declared body must be bounded before body processing"
                .to_owned(),
        );
    }
    let header_check = model
        .find("!response_headers_admitted(response.headers())")
        .ok_or_else(|| "provider response-header admission is missing".to_owned())?;
    let status = model
        .find("ifresponse.status()!=StatusCode::OK")
        .ok_or_else(|| "provider response status handling is missing".to_owned())?;
    let body = model
        .find("letmutstream=response.bytes_stream()")
        .ok_or_else(|| "provider response body stream is missing".to_owned())?;
    let content_length = model
        .find("if!response_content_length_admitted(response.headers(),config.stream_budget().max_wire_bytes(),)")
        .ok_or_else(|| "provider response content-length admission is missing".to_owned())?;
    if !(header_check < status && status < content_length && content_length < body) {
        return Err(
            "provider response headers and declared body must be bounded before body processing"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_provider_transport_http2_ingress_boundary(source: &str) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let source = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "pubconstAGENT_PROVIDER_HTTP2_INITIAL_RECEIVE_WINDOW_BYTES:u32=65_535;",
        "pubconstMAX_AGENT_PROVIDER_HTTP2_FRAME_BYTES:u32=16*1_024;",
        ".http2_initial_stream_window_size(AGENT_PROVIDER_HTTP2_INITIAL_RECEIVE_WINDOW_BYTES)",
        ".http2_initial_connection_window_size(AGENT_PROVIDER_HTTP2_INITIAL_RECEIVE_WINDOW_BYTES)",
        ".http2_adaptive_window(false)",
        ".http2_max_frame_size(MAX_AGENT_PROVIDER_HTTP2_FRAME_BYTES)",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent provider transport lost bounded HTTP/2 ingress rule {required}"
            ));
        }
    }
    if source.contains(".http2_adaptive_window(true)") {
        return Err("agent provider transport enabled adaptive HTTP/2 receive windows".to_owned());
    }
    Ok(())
}

// These fixed platform vault loaders each retry Inaccessible once, under the
// process-wide vault turn, whose wait for the call ahead is itself bounded so
// one unanswered OS prompt cannot queue every later load. No other thread or
// sleep is exempted.
const KEYCHAIN_RETRY_SLEEP: &str = "std::thread::sleep(std::time::Duration::from_millis(40));";
const KEYCHAIN_LOGIN_LOADER: &str = r#"fn load_keychain_login_credential<P: AgentCredentialBinding>(
    provider: P,
    service: &'static str,
    account: &'static str,
) -> Result<AgentProviderCredential<P>, MacosAgentProviderCredentialError> {
    let _turn = keychain_turn().map_err(vault_error)?;
    match read_keychain_login_credential(provider, service, account) {
        Err(MacosAgentProviderCredentialError::Inaccessible) => {
            std::thread::sleep(std::time::Duration::from_millis(40));
            read_keychain_login_credential(provider, service, account)
        }
        loaded => loaded,
    }
}"#;
const LEAD_KEYCHAIN_LOADER: &str = r#"pub(super) fn read(service: &str) -> Result<LeadSecret, LeadKeyError> {
        let _turn = turn()?;
        match read_once(service) {
            Err(LeadKeyError::Inaccessible) => {
                std::thread::sleep(std::time::Duration::from_millis(40));
                read_once(service)
            }
            read => read,
        }
    }"#;

const WINDOWS_VAULT_RETRY_SLEEP: &str = "std::thread::sleep(Duration::from_millis(40));";
const WINDOWS_VAULT_LOADER: &str = r#"fn load_windows_credential<P: AgentCredentialBinding>(
    provider: P,
    target: &str,
) -> Result<AgentProviderCredential<P>, AgentProviderVaultError> {
    let _turn = keychain_turn().map_err(vault_error)?;
    let read = || zephium_credentials::read(target);
    let loaded = match read() {
        Err(zephium_credentials::VaultError::Inaccessible) => {
            std::thread::sleep(Duration::from_millis(40));
            read()
        }
        loaded => loaded,
    };
    let secret = loaded.map_err(vault_error)?;
    AgentProviderCredential::try_from_zeroizing(provider, secret).map_err(|error| match error {
        AgentProviderCredentialError::Content => AgentProviderVaultError::Invalid,
        AgentProviderCredentialError::Capacity => AgentProviderVaultError::Capacity,
    })
}"#;
const LEAD_WINDOWS_VAULT_LOADER: &str = r#"pub(super) fn read(service: &str) -> Result<LeadSecret, LeadKeyError> {
        let _turn = crate::provider_transport::keychain_turn().map_err(map)?;
        let loaded = match zephium_credentials::read(service) {
            Err(VaultError::Inaccessible) => {
                std::thread::sleep(std::time::Duration::from_millis(40));
                zephium_credentials::read(service)
            }
            loaded => loaded,
        };
        let bytes = loaded.map_err(map)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| LeadKeyError::Invalid)?;
        LeadSecret::new(text.to_owned()).map_err(|_| LeadKeyError::Invalid)
    }"#;

fn without_exact_keychain_retry(source: &str, loader: &str) -> Result<String, String> {
    let loader = compact(loader);
    if source.matches(&loader).count() != 1 {
        return Err("Keychain loader changed its exact single 40 ms retry".to_owned());
    }
    Ok(source.replacen(
        &loader,
        &loader
            .replace(KEYCHAIN_RETRY_SLEEP, "")
            .replace(WINDOWS_VAULT_RETRY_SLEEP, ""),
        1,
    ))
}

fn validate_provider_transport_shutdown_contract(
    source: &str,
    lead_keys: &str,
) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let proof_declaration = production
        .find("pub struct AgentProviderTransportShutdownProof")
        .ok_or_else(|| "agent provider transport shutdown proof is missing".to_owned())?;
    let proof_attribute_start = production[..proof_declaration]
        .rfind("\n\n")
        .map_or(0, |start| start + 2);
    let proof_attributes = &production[proof_attribute_start..proof_declaration];
    for forbidden in ["Clone", "Copy", "Default", "Serialize", "Deserialize"] {
        if proof_attributes.contains(forbidden) {
            return Err(format!(
                "agent provider transport shutdown proof acquired forgeable derive {forbidden}"
            ));
        }
    }

    let source = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "pubconstMAX_AGENT_PROVIDER_TRANSPORT_SHUTDOWN_PROOF_BYTES:usize=32;",
        "#[cfg(feature=\"probe-harness\")]pubfnload_probe_openai_credential()->Result<AgentProviderCredential,AgentProviderVaultError>{#[cfg(target_os=\"macos\")]letloaded=load_macos_probe_openai_credential();#[cfg(not(target_os=\"macos\"))]letloaded=load_development_openai_credential();loaded}",
        "pubconstfnis_idle(self)->bool{self.active==0}",
        "pubconstfnis_quiescent(self)->bool{self.sealed&&self.active==0}",
        "drained:Notify,",
        "shutdown_waiting:AtomicBool,",
        "structAgentProviderTransportShutdownWaitGuard<'a>{waiting:&'aAtomicBool,}",
        "implDropforAgentProviderTransportShutdownWaitGuard<'_>{fndrop(&mutself){self.waiting.store(false,Ordering::Release);}}",
        "shutdown:AgentProviderCancellation::new(),drained:Notify::new(),",
        "shutdown_waiting:AtomicBool::new(false),",
        "pubstructAgentProviderTransportShutdownProof{snapshot:AgentProviderTransportSnapshot,}",
        "pubconstfnsnapshot(&self)->AgentProviderTransportSnapshot{self.snapshot}",
        "size_of::<AgentProviderTransportShutdownProof>()<=MAX_AGENT_PROVIDER_TRANSPORT_SHUTDOWN_PROOF_BYTES",
        "pubenumAgentProviderTransportShutdownError{",
        "pubfntry_prove_shutdown(&self,)->Result<AgentProviderTransportShutdownProof,AgentProviderTransportShutdownError>",
        ".snapshot().map_err(|_|AgentProviderTransportShutdownError::State)?;",
        "if!snapshot.is_sealed(){returnErr(AgentProviderTransportShutdownError::Unsealed);}",
        "if!snapshot.is_idle(){returnErr(AgentProviderTransportShutdownError::Pending);}",
        "Ok(AgentProviderTransportShutdownProof{snapshot})",
        "Deadline,",
        "WaiterActive,",
        "pubfnseal_and_prove_shutdown_until(&self,deadline:Instant,)->Result<implstd::future::Future<Output=Result<AgentProviderTransportShutdownProof,AgentProviderTransportShutdownError,>,>+Send+'_,AgentProviderTransportShutdownError,>{self.seal();",
        "self.shared.shutdown_waiting.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).map_err(|_|AgentProviderTransportShutdownError::WaiterActive)?;",
        "letwait_guard=AgentProviderTransportShutdownWaitGuard{waiting:&self.shared.shutdown_waiting,};Ok(asyncmove{let_wait_guard=wait_guard;",
        "letdeadline_sleep=tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));tokio::pin!(deadline_sleep);",
        "letdrained=self.shared.drained.notified();tokio::pin!(drained);drained.as_mut().enable();matchself.try_prove_shutdown()",
        "tokio::select!{()=&mutdrained=>{}()=&mutdeadline_sleep=>{returnErr(AgentProviderTransportShutdownError::Deadline);}}",
        "letidle=state.active.is_empty();drop(state);ifidle{self.shared.drained.notify_waiters();}",
        "pubfnseal(&self){self.shared.shutdown.cancel();matchself.shared.state.lock(){Ok(mutstate)=>state.sealed=true,Err(poisoned)=>poisoned.into_inner().sealed=true,}}",
        "ifstate.sealed{returnErr(ReserveError::Sealed);}",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent provider transport lost exact shutdown proof rule {required}"
            ));
        }
    }
    let source = without_exact_keychain_retry(&source, KEYCHAIN_LOGIN_LOADER)?;
    let source = without_exact_keychain_retry(&source, WINDOWS_VAULT_LOADER)?;
    let lead_keys = without_exact_keychain_retry(&compact(lead_keys), LEAD_KEYCHAIN_LOADER)?;
    let lead_keys = without_exact_keychain_retry(&lead_keys, LEAD_WINDOWS_VAULT_LOADER)?;
    for forbidden in ["std::thread", "thread::sleep("] {
        if lead_keys.contains(forbidden) {
            return Err(format!(
                "lead Keychain loader acquired forbidden surface {forbidden}"
            ));
        }
    }
    for forbidden in [
        "implAgentProviderTransportShutdownProof{pubfnnew(",
        "implAgentProviderTransportShutdownProof{pubconstfnnew(",
        "implCloneforAgentProviderTransportShutdownProof",
        "implCopyforAgentProviderTransportShutdownProof",
        "DefaultforAgentProviderTransportShutdownProof",
        "SerializeforAgentProviderTransportShutdownProof",
        "DeserializeforAgentProviderTransportShutdownProof",
        "state.sealed=false",
        "tokio::spawn(",
        "std::thread",
        "thread::sleep(",
        "tokio::time::interval(",
        "tokio::sync::mpsc",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "agent provider transport shutdown proof acquired forbidden surface {forbidden}"
            ));
        }
    }
    if source
        .matches("Ok(AgentProviderTransportShutdownProof{snapshot})")
        .count()
        != 1
    {
        return Err(
            "agent provider transport shutdown proof must have one checked construction".to_owned(),
        );
    }
    Ok(())
}

fn validate_provider_secret_diagnostic_contract(source: &str) -> Result<(), String> {
    for declaration in [
        "pub struct AgentProviderCredential",
        "struct AgentProviderAttemptCredential",
    ] {
        let declaration_start = source
            .find(declaration)
            .ok_or_else(|| format!("agent provider credential owner is missing {declaration}"))?;
        let attribute_block_start = source[..declaration_start]
            .rfind("\n\n")
            .map_or(0, |start| start + 2);
        if source[attribute_block_start..declaration_start].contains("#[derive(") {
            return Err(format!(
                "agent provider credential owner must remain move-only and manually redacted {declaration}"
            ));
        }
    }

    let source = compact(source);
    for owner in [
        // 77e1dbcf: one owner, generic over a sealed closed set of two identities.
        "pubstructAgentProviderCredential<P:AgentCredentialBinding=AgentProviderKind>{provider:P,secret:Zeroizing<Vec<u8>>,}",
        "modcredential_binding{pubtraitSealed{}implSealedforcrate::AgentProviderKind{}implSealedforsuper::DecisionCredentialProvider{}}",
        "pubtraitAgentCredentialBinding:credential_binding::Sealed+Copy+fmt::Debug{}",
        "structAgentProviderAttemptCredential{secret:Zeroizing<Vec<u8>>,}",
    ] {
        if !source.contains(owner) {
            return Err(format!(
                "retained provider credential owner lost zeroizing byte storage {owner}"
            ));
        }
    }
    for required in [
        "letmutencoded=Zeroizing::new(Vec::new());",
        "secret.try_reserve_exact(credential.secret.len())",
        "secret.extend_from_slice(&credential.secret);",
        "sensitive_header(provider,&self.secret)",
        "value.set_sensitive(true);",
        ".field(\"secret\",&\"[redacted]\")",
        ".field(\"credential\",&\"[redacted]\")",
        "fnnetwork_failure(error:&reqwest::Error)->AgentProviderTransportOutcome",
        "iferror.is_timeout()",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "agent provider secret/diagnostic boundary lost required protection {required}"
            ));
        }
    }

    let attempt_copy_start = source
        .find("fntry_from_credential(")
        .ok_or_else(|| "agent provider bounded attempt credential copy is missing".to_owned())?;
    let attempt_copy_end = source[attempt_copy_start..]
        .find("fnsensitive_header(")
        .map(|offset| attempt_copy_start + offset)
        .ok_or_else(|| "agent provider dispatch-time header conversion is missing".to_owned())?;
    let attempt_copy = &source[attempt_copy_start..attempt_copy_end];
    for forbidden in ["sensitive_header(", "HeaderValue"] {
        if attempt_copy.contains(forbidden) {
            return Err(format!(
                "agent provider admission creates an unnecessary non-zeroizing header copy {forbidden}"
            ));
        }
    }
    for forbidden in [
        ".field(\"secret\",&self.secret)",
        ".field(\"credential\",&self.credential)",
        "error.to_string()",
        "format!(\"{error",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "agent provider secret/diagnostic boundary exposes forbidden value {forbidden}"
            ));
        }
    }

    let network_start = source
        .find("fnnetwork_failure(error:&reqwest::Error)->AgentProviderTransportOutcome")
        .ok_or_else(|| "agent provider network failure classifier is missing".to_owned())?;
    let network = &source[network_start..];
    let mut depth = 0_u32;
    let mut opened = false;
    let mut network_end = None;
    for (offset, character) in network.char_indices() {
        match character {
            '{' => {
                opened = true;
                depth = depth.checked_add(1).ok_or_else(|| {
                    "agent provider network classifier nesting overflow".to_owned()
                })?;
            }
            '}' if opened => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "agent provider network classifier is malformed".to_owned())?;
                if depth == 0 {
                    network_end = Some(offset + character.len_utf8());
                    break;
                }
            }
            _ => {}
        }
    }
    let network_end = network_end
        .ok_or_else(|| "agent provider network failure classifier is unclosed".to_owned())?;
    let network = &network[..network_end];
    if network.matches("error").count() != 2 {
        return Err(
            "agent provider network failure must consume reqwest diagnostics only via is_timeout"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_provider_revocation_before_secret_materialization(source: &str) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let source = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    let execute_start = source
        .find("asyncfnexecute_model<F>")
        .ok_or_else(|| "agent provider attempt execution boundary is missing".to_owned())?;
    let execute_end = source[execute_start..]
        .find("implfmt::DebugforAgentProviderAttempt")
        .map(|offset| execute_start + offset)
        .ok_or_else(|| "agent provider attempt execution boundary is unclosed".to_owned())?;
    let execute = &source[execute_start..execute_end];
    let cancellation = "ifself.cancellation.is_cancelled()||self.shutdown.is_cancelled(){returnself.finish_terminal(";
    let checks = execute
        .match_indices(cancellation)
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();
    if checks.len() != 2 {
        return Err(
            "agent provider dispatch requires exactly two terminal sticky-cancellation checks"
                .to_owned(),
        );
    }
    let decoder = execute
        .find("letmutdecoder=matchAgentProviderStreamDecoder::try_new")
        .ok_or_else(|| "agent provider stream decoder construction is missing".to_owned())?;
    let header = execute[decoder..]
        .find("letcredential=matchself.credential.as_ref()")
        .map(|offset| decoder + offset)
        .ok_or_else(|| {
            "agent provider dispatch-time authentication header is missing".to_owned()
        })?;
    let request = execute[header..]
        .find("letSome(request)=provider_request(")
        .map(|offset| header + offset)
        .ok_or_else(|| "agent provider fixed request construction is missing".to_owned())?;
    if !(checks[0] < decoder && decoder < header && header < request && request < checks[1]) {
        return Err(
            "agent provider cancellation must precede secret materialization and be rechecked before dispatch"
                .to_owned(),
        );
    }
    Ok(())
}

/// A chosen model's drive keeps the fixed-endpoint ordering: it moves the
/// body out of the retained owner, checks sticky cancellation twice, then
/// publishes possible dispatch before the client can first be polled. It
/// holds no key and reaches no network of its own. A reply with no usable
/// browser call may be asked again a fixed number of times, every ask
/// summed into the one call's usage and held to its one reservation.
fn validate_chosen_model_drive_contract(source: &str) -> Result<(), String> {
    let production = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let source = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for forbidden in ["reqwest", "credential", "sensitive_header", "count_openai"] {
        if source.contains(forbidden) {
            return Err(format!(
                "chosen-model drive acquired fixed-endpoint authority {forbidden}"
            ));
        }
    }
    let drive_start = source
        .find("pubasyncfnexecute_model_client<F>(&mutself,")
        .ok_or_else(|| "chosen-model drive is missing or consuming".to_owned())?;
    let drive = &source[drive_start..];
    let begin = drive
        .find("=self.begin_generation_drive()?")
        .ok_or_else(|| "chosen-model drive no longer moves its body first".to_owned())?;
    let cancellation =
        "ifself.cancellation.is_cancelled()||self.shutdown.is_cancelled(){returnself.finish_terminal(cancelled_failure());}";
    let checks = drive
        .match_indices(cancellation)
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();
    let dispatch = drive
        .find("self.mark_model_may_have_dispatched();")
        .ok_or_else(|| "chosen-model drive no longer publishes dispatch".to_owned())?;
    let call = drive
        .find("outcome=model.client.call(request.clone(),&events)=>outcome")
        .ok_or_else(|| "chosen-model drive call boundary is missing".to_owned())?;
    if checks.len() != 2 || !(begin < checks[0] && checks[1] < dispatch && dispatch < call) {
        return Err(
            "chosen-model drive must move authority, recheck cancellation and publish dispatch before its call"
                .to_owned(),
        );
    }
    for required in [
        "spent.input_tokens>reserved||spent.output_tokens>max_output",
        "outcome.usage=spent;",
        "reasks<MAX_REASKS",
        "contract.finish(call,&config,outcome)",
        "Err(_)=>{self.fail_stop();returnself.finish_terminal(integration_failure());}",
    ] {
        if !drive.contains(required) {
            return Err(format!(
                "chosen-model drive lost required settlement rule {required}"
            ));
        }
    }
    if !source.contains("constMAX_REASKS:u8=2;") {
        return Err("chosen-model drive re-asks a malformed call more than twice".to_owned());
    }
    Ok(())
}

fn validate_semantic_diff_policy_contract(
    root: &str,
    diff: &str,
    model: &str,
    policy: &str,
) -> Result<(), String> {
    if !compact(root).contains("SemanticDiffDeliveryReceipt") {
        return Err("agentic root stopped exporting exact semantic-diff delivery proof".to_owned());
    }

    let diff = compact(diff);
    for required in [
        "baseline_guard:[u8;32]",
        "current_fingerprint:SemanticObservationFingerprint",
        "guard:[u8;32]",
        "letbaseline_guard=acknowledgement.guard()",
        "letguard=semantic_diff_guard(baseline_guard,current_fingerprint.digest())",
        "fnsemantic_diff_guard(baseline_guard:[u8;32],current_guard:[u8;32])->[u8;32]",
        "hasher.update(b\"ZEPHIUM-SEMANTIC-DIFF-1\\0\")",
    ] {
        if !diff.contains(required) {
            return Err(format!(
                "semantic diff lost exact baseline/current binding {required}"
            ));
        }
    }

    let model = compact(model);
    for required in [
        "pubstructSemanticDiffDeliveryReceipt",
        "diff_guard:[u8;32]",
        "pub(crate)fnmatches_diff(&self,diff:&SemanticDiff)->bool",
        "self.diff_guard==diff.guard()&&self.current_fingerprint==*diff.current_fingerprint()",
        "pubfnsettle_delivery_receipt(",
        "diff_guard:self.diff_guard",
    ] {
        if !model.contains(required) {
            return Err(format!(
                "semantic diff model seam lost exact delivery proof {required}"
            ));
        }
    }

    let policy = compact(policy);
    for required in [
        "source_guard:[u8;32]",
        "ModelInputKind::Diff",
        "pubfnprepare_diff_input(",
        "if!payload.matches_diff(diff)",
        "cohort.source_guard==diff.baseline_guard()",
        "source_guard:diff.current_guard()",
        "fnretire_taint_reference(",
        "fninsert_taint_reference(",
        "source_guard:read.guard()",
        "pubfncommit_diff_input(",
        "self.commit_model_input(admission,ModelInputKind::Diff,receipt.guard())",
        "hasher.update(candidate.source_guard)",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "agent policy lost exact semantic-diff authority {required}"
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_semantic_locate_contract(
    root: &str,
    locate: &str,
    locate_model: &str,
    provider_root: &str,
    provider_tool: &str,
    continuation: &str,
    provider_request: &str,
    policy: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "modsemantic_locate;",
        "modsemantic_locate_model;",
        "locate_semantic_observation",
        "encode_semantic_locate_result",
        "MAX_SEMANTIC_LOCATE_MATCHES",
        "MAX_SEMANTIC_LOCATE_QUERY_BYTES",
        "MAX_SEMANTIC_LOCATE_QUERY_TERMS",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost bounded semantic-locate contract {required}"
            ));
        }
    }

    let locate = compact(locate);
    for required in [
        "pubconstMAX_SEMANTIC_LOCATE_QUERY_BYTES:usize=1_024;",
        // Capacity now derives from the unchanged public byte ceiling, with
        // fixed bit storage and bounded per-frame ancestry caching. Match the
        // current implementation rather than reinstating its old hidden limit.
        "pubconstMAX_SEMANTIC_LOCATE_QUERY_TERMS:usize=MAX_SEMANTIC_LOCATE_QUERY_BYTES.div_ceil(2);",
        "structQueryTermBits([u64;MAX_SEMANTIC_LOCATE_QUERY_TERMS.div_ceil(64)]);",
        "pubconstMAX_SEMANTIC_LOCATE_MATCHES:u8=32;",
        "||looks_like_secret_value(&source)",
        "ifterms.iter().all(|term|is_query_stopword(term))",
        "if!acknowledgement.matches(observation)",
        "validate_current_frames(observation,current_frames)?;",
        "fingerprint:SemanticObservationFingerprint",
        "ifrequest.fingerprint!=SemanticObservationFingerprint::from_observation(observation)",
        "SemanticLocateScope::Frame(reference)",
        "SemanticFrameBoundaryStatus::Observed{frame,..}=>Some(frame)",
        "Vec::<RankedMatch>::with_capacity(usize::from(request.budget.max_matches))",
        "ifnode.sensitivity()==SemanticSensitivity::Secret",
        "letcandidate=match_node(node,&request.query,&mutscratch,inherited,&mutancestor_contributions[node_index],);",
        "ancestor_term_bits(frame.nodes(),node_index,&ancestor_contributions)",
        "letmutancestor_contributions=vec![QueryTermBits::default();frame.nodes().len()];",
        "elseifcontent_anchor{",
        "contains_normalized_phrase(&query.normalized,anchor)",
        "for_in0..MAX_SEMANTIC_DEPTH",
        "truncated:usize::from(matched_nodes)>matches.len()",
    ] {
        if !locate.contains(required) {
            return Err(format!(
                "semantic locate lost acknowledgement, scope, secret, or resource bound {required}"
            ));
        }
    }
    let secret_check = locate
        .find("ifnode.sensitivity()==SemanticSensitivity::Secret")
        .ok_or_else(|| "semantic locate secret exclusion is missing".to_owned())?;
    let matching = locate
        .find("letcandidate=match_node(")
        .ok_or_else(|| "semantic locate matcher is missing".to_owned())?;
    if secret_check >= matching {
        return Err("semantic locate must exclude secret nodes before matching".to_owned());
    }
    for forbidden in [
        "regex::",
        "scraper::",
        "fantoccini",
        "query_selector(",
        "querySelector(",
        "evaluate_javascript(",
        "execute_script(",
        "Runtime.evaluate",
        "std::net::",
        "reqwest::",
    ] {
        if locate.contains(forbidden) {
            return Err(format!(
                "semantic locate acquired forbidden document/runtime surface {forbidden}"
            ));
        }
    }

    let provider_tool = compact(provider_tool);
    for required in [
        "pubconstMAX_AGENT_BROWSER_SEMANTIC_QUERY_BYTES:usize=MAX_SEMANTIC_LOCATE_QUERY_BYTES;",
        "SemanticLocateQuery::try_new(value)",
        "pubfninto_locate_query(self)->SemanticLocateQuery",
        "pubfntry_into_locate_scope(self,)->Result<SemanticLocateScope,AgentBrowserToolContractError>",
        "Self::SurroundingText{..}|Self::TextSearch{..}=>{Err(AgentBrowserToolContractError::Scope)}",
    ] {
        if !provider_tool.contains(required) {
            return Err(format!(
                "provider locate proposal drifted from bounded semantic core {required}"
            ));
        }
    }

    let locate_model = compact(locate_model);
    for required in [
        "pubconstSEMANTIC_LOCATE_MODEL_SCHEMA_VERSION:u16=1;",
        "SemanticModelEncodingBudget::LOCATE_RESULT_EXACT",
        "validate_semantic_token_measurement(&self.budget,&measurement,expected_revision)?;",
        "ZLOC{}content=untrusted",
        "Lref={}role={}match={}sensitivity={}source={}actionable={}",
        "matched.sensitivity()==SemanticSensitivity::Secret",
        "result.matches_acknowledgement(&self.acknowledgement)",
        "locate_guard==result.guard()",
        "pubstructSemanticLocateDeliveryReceipt",
    ] {
        if !locate_model.contains(required) {
            return Err(format!(
                "semantic locate model delivery lost exact bounded contract {required}"
            ));
        }
    }
    for forbidden in [
        "write_quoted",
        "matched.name",
        "matched.text",
        "matched.value",
    ] {
        if locate_model.contains(forbidden) {
            return Err(format!(
                "semantic locate model acquired page-content output surface {forbidden}"
            ));
        }
    }

    let continuation = compact(continuation);
    for required in [
        "ifself.correlation.kind()!=AgentBrowserToolKind::Locate",
        "if!result.matches_acknowledgement(&self.baseline)",
        "if!payload.matches_result(result)",
        "lettranscript=transcript.try_bind(correlation,tool_result)?;",
        "AgentBrowserToolKind::Locate|AgentBrowserToolKind::Read|AgentBrowserToolKind::Extract|AgentBrowserToolKind::Screenshot",
        "pubstructAgentProviderBoundLocateContinuation",
    ] {
        if !continuation.contains(required) {
            return Err(format!(
                "provider locate continuation lost exact tool/baseline join {required}"
            ));
        }
    }

    let provider_request = compact(provider_request);
    for required in [
        "pubstructAgentProviderLocateRequestDraft",
        "counter.count_openai_responses_input(",
        "counter.count_anthropic_messages_input(",
        "policy.prepare_provider_locate_input(",
        "commitment:AgentProviderInputCommitment::Locate",
        "continuation_transcript:Some(self.continuation_transcript)",
        "AgentProviderInputEvidence::Locate(receipt)",
    ] {
        if !provider_request.contains(required) {
            return Err(format!(
                "provider locate result lost local counting or atomic disclosure {required}"
            ));
        }
    }

    let provider_root = compact(provider_root);
    for required in [
        "fnvalidate_locate_request(",
        "iflocate.quality()!=crate::SemanticTokenCountQuality::ExactLocal",
        "self.validate_diff_request(request,locate,structured_input)",
    ] {
        if !provider_root.contains(required) {
            return Err(format!(
                "provider locate result lost exact local count bound {required}"
            ));
        }
    }

    let policy = compact(policy);
    for required in [
        "pub(crate)fnprepare_provider_locate_input(",
        "if!delivery.matches_result(result)",
        "letcandidates=locate_taints(result,request.account(),&self.taints)?;",
        "ModelInputKind::Locate",
        "cohort.source_guard==result.observation_guard()",
        "cohort.contains_reference(matched.reference())",
        "pubfncommit_locate_input(",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "semantic locate policy lost baseline-only taint admission {required}"
            ));
        }
    }
    Ok(())
}

fn validate_semantic_read_continuation_contract(
    root: &str,
    read: &str,
    read_model: &str,
    provider_root: &str,
    continuation: &str,
    provider_request: &str,
    policy: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "AgentProviderBoundReadContinuation",
        "AgentProviderReadContinuationRequestDraft",
        "AgentPreparedReadContinuationRequest",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost semantic read continuation export {required}"
            ));
        }
    }

    let read = compact(read);
    for required in [
        "observation_fingerprint:SemanticObservationFingerprint",
        "pub(crate)constfnobservation_guard(&self)->[u8;32]",
        "pub(crate)fnmatches_acknowledgement(",
        "acknowledgement.guard()==self.observation_fingerprint.digest()",
        "node.sensitivity()==SemanticSensitivity::Secret",
        "pubconstMAX_SEMANTIC_READ_ITEMS:u16=768",
        "pubconstMAX_SEMANTIC_READ_BYTES:u32=128*1024",
    ] {
        if !read.contains(required) {
            return Err(format!(
                "semantic read lost exact observation, secret, or resource binding {required}"
            ));
        }
    }

    let read_model = compact(read_model);
    for required in [
        "measurement:SemanticTokenMeasurement",
        "observation_guard:[u8;32]",
        "self.observation_guard==read.observation_guard()",
        "observation_guard:read.observation_guard()",
        "pubfnmatches_read(&self,read:&SemanticReadResult<'_>)->bool",
    ] {
        if !read_model.contains(required) {
            return Err(format!(
                "semantic read delivery lost exact source and token binding {required}"
            ));
        }
    }

    let continuation = compact(continuation);
    for required in [
        "pubfnbind_read_request(",
        "self.correlation.kind()!=AgentBrowserToolKind::Read",
        "match(self.correlation.read_scope.as_ref(),read.selected_target()){",
        "(Some(super::AgentBrowserScopeProposal::Initial),None)=>{}",
        "(Some(super::AgentBrowserScopeProposal::Subtree(wanted)),Some(actual))if*wanted==actual&&!read.has_retained_evidence()=>{}",
        "_=>returnErr(AgentProviderContinuationError::Scope),",
        "if!read.matches_acknowledgement(&self.baseline)",
        "if!payload.matches_read(read)",
        "pubstructAgentProviderBoundReadContinuation",
        "baseline.guard()!=receipt.observation_guard()",
        "AgentBrowserToolKind::Locate|AgentBrowserToolKind::Read|AgentBrowserToolKind::Extract|AgentBrowserToolKind::Screenshot",
    ] {
        if !continuation.contains(required) {
            return Err(format!(
                "provider read continuation lost exact tool/baseline join {required}"
            ));
        }
    }

    let provider_request = compact(provider_request);
    for required in [
        "pubstructAgentProviderReadContinuationRequestDraft",
        "counter.count_openai_responses_input(",
        "counter.count_anthropic_messages_input(",
        "config().validate_read_continuation_request(",
        "policy.prepare_provider_read_input(",
        "pubstructAgentPreparedReadContinuationRequest",
        "commitment:AgentProviderInputCommitment::Read",
        "continuation_transcript:Some(self.continuation_transcript)",
        "continuation_baseline:Some(self.baseline)",
        "Self::Read(_)|Self::Extraction(_)=>None",
    ] {
        if !provider_request.contains(required) {
            return Err(format!(
                "provider read result lost local counting, non-promotion, or atomic disclosure {required}"
            ));
        }
    }

    let provider_root = compact(provider_root);
    for required in [
        "fnvalidate_read_continuation_request(",
        "ifread.quality()!=crate::SemanticTokenCountQuality::ExactLocal",
        "self.validate_diff_request(request,read,structured_input)",
    ] {
        if !provider_root.contains(required) {
            return Err(format!(
                "provider read result lost exact local count bound {required}"
            ));
        }
    }

    let policy = compact(policy);
    for required in [
        "pub(crate)fnprepare_provider_read_input(",
        "!delivery.matches_read(read)||!read.matches_acknowledgement(baseline)",
        "provider_read_taints(read,baseline,request.account(),&self.taints)?",
        "ModelInputKind::Read",
        "cohort.source_guard==baseline.guard()",
        "cohort.contains_reference(provenance.reference())",
        "ifcandidates.is_empty()",
        "pubfncommit_read_input(",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "semantic read policy lost baseline-only taint admission {required}"
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_semantic_extraction_provider_contract(
    root: &str,
    extract: &str,
    extract_model: &str,
    provider_root: &str,
    provider_tool: &str,
    continuation: &str,
    provider_request: &str,
    provider_extraction: &str,
    policy: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "modsemantic_extract;",
        "modsemantic_extract_model;",
        "encode_semantic_extraction_request",
        "AgentProviderExtractionRequestDraft",
        "AgentProviderExtractionOutputBinding",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost constrained extraction contract {required}"
            ));
        }
    }

    let extract = compact(extract);
    for required in [
        "pubconstMAX_SEMANTIC_EXTRACTION_INPUT_BYTES:usize=64*1024;",
        "pubconstMAX_SEMANTIC_EXTRACTION_FIELDS:usize=64;",
        "pubfnextract_delivered_semantic_read<'a>(",
        "if!delivery.matches(schema,read)",
        "extract_semantic_read_inner(schema,read,sensitivity_limit,model_output)",
        "looks_like_secret_value",
        "SemanticExtractionTrust::ModelMapped",
    ] {
        if !extract.contains(required) {
            return Err(format!(
                "semantic extraction lost bounded Rust admission {required}"
            ));
        }
    }

    let extract_model = compact(extract_model);
    for required in [
        "pubconstSEMANTIC_EXTRACTION_MODEL_SCHEMA_VERSION:u16=1;",
        "ZEXTRACT{}schema_content=trustedevidence_content=untrustedschema={}fields={}",
        "letread=encode_semantic_read(read,budget)?.into_extraction_parts();",
        "fnextraction_schema_guard(schema:&SemanticExtractionSchema)->[u8;32]",
        "field.max_text_bytes().unwrap_or_default()",
        "field.maximum_unsigned().unwrap_or_default()",
        "field.max_list_items().unwrap_or_default()",
        "field.max_list_item_bytes().unwrap_or_default()",
        "pubstructSemanticExtractionDeliveryReceipt",
        "self.schema_guard==extraction_schema_guard(schema)",
        "self.read_guard==read.guard()",
        "self.request_guard==extraction_request_guard(",
    ] {
        if !extract_model.contains(required) {
            return Err(format!(
                "extraction model input lost exact schema/read guard {required}"
            ));
        }
    }

    let provider_tool = compact(provider_tool);
    for required in [
        "Some(AgentBrowserToolProposal::Extract{schema,..})=>Some(*schema)",
        "extraction_schema:Option<SemanticExtractionSchemaId>",
        "extraction_schema,extraction_scope,read_scope,snapshot_scope,navigation_target,provider_item_id:self.provider_item_id",
        "Some(AgentBrowserToolProposal::Read(scope))=>Some(scope.clone())",
    ] {
        if !provider_tool.contains(required) {
            return Err(format!(
                "provider extraction tool lost exact schema correlation {required}"
            ));
        }
    }

    let continuation = compact(continuation);
    for required in [
        "pubfnbind_extraction_request(",
        "self.correlation.kind()!=AgentBrowserToolKind::Extract||self.correlation.extraction_schema!=Some(schema.id())",
        "if!read.matches_acknowledgement(&self.baseline)",
        "if!payload.matches(schema,read)",
        // 697c56d0 narrows the provider schema with the already-joined read.
        "output_schema:super::request::bound_extraction_output_schema(schema,Some(read))",
        "pubstructAgentProviderBoundExtractionContinuation",
        "AgentBrowserToolKind::Locate|AgentBrowserToolKind::Read|AgentBrowserToolKind::Extract|AgentBrowserToolKind::Screenshot",
        "AgentProviderInputEvidence::Screenshot(receipt)=>{letbaseline=continuation_baseline?;if!receipt.matches_baseline(&baseline){returnNone;}baseline}",
        "AgentProviderInputEvidence::Extraction(_)=>{returnNone;}",
    ] {
        if !continuation.contains(required) {
            return Err(format!(
                "provider extraction continuation lost terminal schema/read join {required}"
            ));
        }
    }

    let provider_request = compact(provider_request);
    for required in [
        "constAGENT_EXTRACTION_INSTRUCTIONS_V1:&str=concat!(",
        "pubstructAgentProviderExtractionRequestDraft",
        "encode_openai_extraction_body(",
        "encode_anthropic_extraction_body(",
        "text:OpenAiExtractionTextWire",
        "output_config:AnthropicExtractionOutputConfigWire",
        "policy.prepare_provider_extraction_input(",
        "commitment:AgentProviderInputCommitment::Extraction",
        "continuation_transcript:None",
        "staticEXTRACTION_OUTPUT_SCHEMA:LazyLock<Value>",
        "project_anthropic_schema(output_schema)",
        "pub(super)fnbound_extraction_output_schema(schema:&SemanticExtractionSchema,read:Option<&crate::SemanticReadResult<'_>>,)->Value",
        "AgentProviderInputEvidence::Extraction(receipt)",
    ] {
        if !provider_request.contains(required) {
            return Err(format!(
                "provider extraction request lost constrained atomic path {required}"
            ));
        }
    }
    for (start, end, label) in [
        (
            "structOpenAiExtractionRequestWire<'a>{",
            "structOpenAiExtractionTextWire<'a>{",
            "OpenAI",
        ),
        (
            "structAnthropicExtractionRequestWire<'a>{",
            "structAnthropicExtractionOutputConfigWire<'a>{",
            "Anthropic",
        ),
    ] {
        let start = provider_request
            .find(start)
            .ok_or_else(|| format!("missing {label} extraction request wire"))?;
        let end = provider_request[start..]
            .find(end)
            .map(|offset| start + offset)
            .ok_or_else(|| format!("unterminated {label} extraction request wire"))?;
        let wire = &provider_request[start..end];
        if wire.contains("tools:") || wire.contains("tool_choice:") {
            return Err(format!(
                "{label} extraction mapping reacquired browser tools"
            ));
        }
    }

    let provider_extraction = compact(provider_extraction);
    for required in [
        "try_reserve_exact(MAX_SEMANTIC_EXTRACTION_INPUT_BYTES)",
        "ifself.failed||batch.call()!=self.call",
        ".checked_add(delta.len()).filter(|bytes|*bytes<=MAX_SEMANTIC_EXTRACTION_INPUT_BYTES)",
        "terminal:&AgentProviderSettledTerminal",
        "terminal.receipt().settlement()!=AgentModelCallSettlement::Completed",
        "terminal.receipt().pricing_attribution().is_none()",
        "terminal.has_tool_turn()",
        "completion.stop()!=AgentProviderStopReason::Completed",
        "completion.stats().tool_calls()!=0",
        "completion.stats().tool_argument_bytes()!=0",
        "Some(self.output.len())",
        "extract_delivered_semantic_read(",
        "self.output.clear();",
    ] {
        if !provider_extraction.contains(required) {
            return Err(format!(
                "provider extraction collector lost bounded exact terminal join {required}"
            ));
        }
    }

    let provider_root = compact(provider_root);
    for required in [
        "fnvalidate_extraction_request(",
        "ifextraction.quality()!=crate::SemanticTokenCountQuality::ExactLocal",
        "self.validate_diff_request(request,extraction,structured_input)",
    ] {
        if !provider_root.contains(required) {
            return Err(format!(
                "provider extraction lost exact local count bound {required}"
            ));
        }
    }

    let policy = compact(policy);
    for required in [
        "pub(crate)structAgentProviderExtractionInput<'a,'read>",
        "pub(crate)fnprepare_provider_extraction_input(",
        "!input.delivery.matches(input.schema,input.read)",
        "matchinput.subtree_target",
        "None=>provider_extraction_read_taints(",
        "Some(target)=>provider_subtree_read_taints(",
        "provider_extraction_read_taints(input.read,input.baseline,request.account(),&self.taints,)?",
        "ModelInputKind::Extraction",
        "pubfncommit_extraction_input(",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "extraction policy lost baseline-only atomic admission {required}"
            ));
        }
    }
    Ok(())
}

fn validate_semantic_execution_contract(
    root: &str,
    effect: &str,
    action: &str,
    execution: &str,
    coordinator: &str,
    context_port: &str,
    native: (&str, &str),
) -> Result<(), String> {
    let (engine_port, macos_action) = native;
    let root = compact(root);
    for required in [
        "modsemantic_execute;",
        "modsemantic_execute_coordinator;",
        "pub(crate)usesemantic_execute::{",
        "begin_semantic_action_settlement",
        "prepare_semantic_action_execution",
        "SemanticActionExecutionPending",
        "SemanticActionSettlementStart",
        "SemanticActionExecutionCoordinator",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost semantic execution handoff {required}"
            ));
        }
    }

    let effect = compact(effect);
    for required in [
        "pubfnmatches_action(&self,action:&SemanticPreparedAction)->bool",
        "self.effect==action.effect()&&self.action_guard==action.verification_guard()",
    ] {
        if !effect.contains(required) {
            return Err(format!(
                "agent effect policy lost exact native action join {required}"
            ));
        }
    }

    if !compact(action).contains("target_geometry:Option<SemanticRect>") {
        return Err(
            "prepared semantic action stopped retaining fresh pre-execution geometry".to_owned(),
        );
    }

    let execution = compact(execution);
    for required in [
        "MAX_SEMANTIC_ACTION_NATIVE_EXECUTION_MILLIS",
        "active:AgentActiveEffect",
        "active.matches_action(action)",
        "action.target_geometry().filter",
        "action.verification_guard()",
        "NativeRecipe::Fill(value)",
        "ExactVisibleUnoccludedTarget",
        "ExactConnectedScrollTarget",
        "SemanticActionExecutionDisposition::ContractViolation",
        "current_frame!=&self.correlation.frame",
        "applied.completed_at>deadline",
        "SemanticActionFailure::BackendRefused",
        "SemanticSettleInstant::from_millis",
        "pubstructSemanticActionSettlementStart{",
        "execution:SemanticActionExecutionApplied",
        "pub(crate)constfntracker_mut(",
        "pub(crate)fninto_parts(",
        "pubstructSemanticActionSettlementRefusal{",
        "active:Box<AgentActiveEffect>",
        "pubfnbegin_semantic_action_settlement(",
        "let(active,disposition)=outcome.into_parts();",
        "if!active.matches_action(action)",
        "SemanticSettleTracker::begin(",
        "applied.settle_started_at()",
        "SemanticActionSettlementStartError::ExecutionFailed",
        "SemanticActionSettlementStartError::ExecutionContract",
        "SemanticActionSettlementStartError::Settlement",
        "pub(crate)structSemanticActionExecutionPending{",
        "pub(crate)fninto_parts(self",
        "(AgentActiveEffect,SemanticActionExecutionDisposition)",
        "pubconstfnaction_failure(&self)->SemanticActionFailure",
        "self.error.action_failure()",
        "Self::ExecutionFailed(failure)=>failure",
        "pub(crate)fnprepare_semantic_action_execution(",
    ] {
        if !execution.contains(required) {
            return Err(format!(
                "semantic execution seam lost required one-shot bound {required}"
            ));
        }
    }
    for (return_type, label) in [
        (
            "(AgentActiveEffect,SemanticActionExecutionDisposition)",
            "native execution outcome",
        ),
        (
            "(AgentActiveEffect,SemanticActionSettlementStartError)",
            "settlement-start refusal",
        ),
        (
            "(AgentActiveEffect,SemanticActionExecutionPreparationError)",
            "raw preparation refusal",
        ),
    ] {
        if !has_crate_private_into_parts(&execution, return_type) {
            return Err(format!(
                "semantic execution exposed or lost {label} ownership"
            ));
        }
    }
    for forbidden in [
        "evaluateJavaScript",
        "callAsyncJavaScript",
        "querySelector",
        "CGEvent",
        "NSEvent",
        "Input.dispatch",
        "WKWebView",
        "WebView2",
        "std::thread",
        "std::fs",
        "pubfnprepare_semantic_action_execution(",
        "pubfninto_parts(self",
    ] {
        if execution.contains(forbidden) {
            return Err(format!(
                "semantic execution core acquired forbidden native/program surface {forbidden}"
            ));
        }
    }

    let coordinator = compact(coordinator);
    for required in [
        "MAX_PENDING_SEMANTIC_ACTION_EXECUTIONS:usize=MAX_AGENT_PENDING_EFFECTS",
        "pending:Vec<SemanticActionExecutionPending>",
        "deadline:SemanticActionExecutionInstant",
        "letdeadline=pending.deadline()",
        "self.pending.len()>=MAX_PENDING_SEMANTIC_ACTION_EXECUTIONS",
        "entry.coordinator_key().context()==action.frame().context().identity()",
        "!self.pending[index].matches_native_settlement(&settlement)",
        "now<self.pending[index].deadline()",
        "self.sealed=true",
        "semantic_action_dispatch_failure(dispatch)",
        "pub(crate)constfnsemantic_action_dispatch_failure(",
        "ContextDispatch::Scheduled=>None",
        "ContextDispatch::Unsupported|ContextDispatch::Rejected(ContextPortFailure::Unsupported)",
        "ContextPortFailure::ResourceExhausted)=>{Some(SemanticActionFailure::ResourceExhausted)",
        "SemanticActionExecutionCoordinatorError::PrematureTimeout",
        "pubstructSemanticActionExecutionCoordinatorRefusal{",
        "pubconstfnaction_failure(&self)->SemanticActionFailure",
        "SemanticActionExecutionCoordinatorError::Preparation(error)=>error.action_failure()",
        "SemanticActionExecutionCoordinatorError::ContextBusy|SemanticActionExecutionCoordinatorError::Capacity=>{SemanticActionFailure::ResourceExhausted}",
        "pub(crate)fninto_parts(self",
        "(AgentActiveEffect,SemanticActionExecutionCoordinatorError)",
        "SemanticEffectClass::LocalWrite",
        "SemanticWaitCondition::TargetState{state:crate::SemanticState::Expanded,present:true,}",
        "AgentActiveEffect::for_execution_qualification(&action,attempt)",
        "pubfnsettle_and_verify(",
        "crate::begin_semantic_action_settlement(outcome,&self.action)",
        "crate::SemanticActionSettlementCoordinator::new()",
        ".observe_snapshot(reservation,observed_at,snapshot)",
        "crate::verify_semantic_action_terminal(*terminal,&self.action,evidence)",
    ] {
        if !coordinator.contains(required) {
            return Err(format!(
                "semantic execution coordinator lost required bound {required}"
            ));
        }
    }
    if !has_crate_private_into_parts(
        &coordinator,
        "(AgentActiveEffect,SemanticActionExecutionCoordinatorError)",
    ) {
        return Err("semantic execution coordinator exposed returned policy authority".to_owned());
    }
    for forbidden in [
        "evaluateJavaScript",
        "callAsyncJavaScript",
        "querySelector",
        "CGEvent",
        "NSEvent",
        "Input.dispatch",
        "std::thread",
        "std::fs",
        "Mutex<",
        "Arc<",
        "pubconstfnsemantic_action_dispatch_failure(",
        "pubfninto_parts(self",
    ] {
        if coordinator.contains(forbidden) {
            return Err(format!(
                "semantic execution coordinator acquired forbidden work/program surface {forbidden}"
            ));
        }
    }

    let context_port = compact(context_port);
    for required in [
        "pubtypeSemanticActionNativeCompletion=Box<dynFnOnce(SemanticActionNativeSettlement)+Send+'static>;",
        "fnexecute_semantic_action(&self,request:SemanticActionNativeRequest,completion:SemanticActionNativeCompletion,)->ContextDispatch;",
    ] {
        if !context_port.contains(required) {
            return Err(format!(
                "agent browser port lost move-only semantic action contract {required}"
            ));
        }
    }

    let engine_port = compact(engine_port);
    let start = engine_port
        .find("fnexecute_semantic_action(")
        .ok_or_else(|| "engine action port lost fail-closed method".to_owned())?;
    let end = engine_port[start..]
        .find("fncapture_semantic_screenshot(")
        .map(|offset| start + offset)
        .ok_or_else(|| "engine action port lost bounded method boundary".to_owned())?;
    let action_method = &engine_port[start..end];
    for required in [
        "#[cfg(target_os=\"macos\")]",
        "!matches!(request.kind(),zephium_agentic::SemanticActionKind::Click|zephium_agentic::SemanticActionKind::Fill|zephium_agentic::SemanticActionKind::Select)",
        "request.frame().frame()!=zephium_agentic::FrameId::MAIN",
        "request.frame().trust()!=zephium_agentic::SemanticFrameTrust::SameOrigin",
        "request.frame().context().identity().kind()!=zephium_agentic::ContextKind::Owned",
        "self.schedule_action(request,completion)",
        "#[cfg(not(target_os=\"macos\"))]",
        "let_=(request,completion);ContextDispatch::Unsupported",
    ] {
        if !action_method.contains(required) {
            return Err(format!(
                "engine action port lost its exact macOS closed-action boundary {required}"
            ));
        }
    }
    for forbidden in [
        "ContextDispatch::Scheduled",
        "ContextDispatch::Rejected",
        "dispatch_to_host",
        "platform::",
    ] {
        if action_method.contains(forbidden) {
            return Err(format!(
                "engine action port bypassed its fixed action scheduler {forbidden}"
            ));
        }
    }

    let macos_action = compact(macos_action);
    for required in [
        "dispatch_guarded(_view,semantic,request,admitted_at,None,completion);",
        "pub(super)fndispatch_guarded(",
        "authority:Option<Box<dynFn()->bool>>",
        // a246faf4: the adapter admits the shared closed set, pinned below.
        "if!zephium_agentic::AGENT_BROWSER_SNAPSHOT_ACTION_KINDS.contains(&request.kind())",
        "encode_semantic_action_runtime_invocation(&request)",
        "semantic.dispatch_action_guarded(invocation,authority,move|outcome|",
        "Ok(evidence)=>complete_runtime_recipe(request,evidence,admitted_at)",
        "request.complete(evidence.backend(),evidence.readiness(),evidence.viewport(),evidence.geometry(),completed_at,completed_at,)",
    ] {
        if !macos_action.contains(required) {
            return Err(format!(
                "macOS semantic action adapter lost a required closed-action invariant {required}"
            ));
        }
    }
    for forbidden in [
        "querySelector",
        "evaluateJavaScript",
        "callAsyncJavaScript",
        "CGEvent",
        "NSEvent",
        "NSResponder",
        "EngineNativeInput",
        "InProcessAccessibility",
        "setHidden",
        "set_visible",
        "orderFront",
        "postEvent",
        "sendEvent",
        "activateIgnoringOtherApps",
        "makeKeyAndOrderFront",
    ] {
        if macos_action.contains(forbidden) {
            return Err(format!(
                "macOS semantic action adapter acquired forbidden authority {forbidden}"
            ));
        }
    }
    Ok(())
}

// The macOS adapter's closed action set: Click, Fill, Select, Press and Scroll.
fn validate_snapshot_action_kinds(provider: &str) -> Result<(), String> {
    if !compact(provider).contains(concat!(
        "pubconstAGENT_BROWSER_SNAPSHOT_ACTION_KINDS:[crate::SemanticActionKind;5]=[",
        "crate::SemanticActionKind::Click,crate::SemanticActionKind::Fill,",
        "crate::SemanticActionKind::Select,crate::SemanticActionKind::Press,",
        "crate::SemanticActionKind::Scroll,];",
    )) {
        return Err("snapshot action kinds widened beyond the closed native set".to_owned());
    }
    Ok(())
}

fn validate_semantic_settle_wake(settle: &str) -> Result<(), String> {
    let settle = compact(settle);
    for required in [
        "pub(crate)fnbegin(",
        "pubconstfnnext_wake(&self)->Option<SemanticSettleInstant>",
        "self.status.is_terminal()",
        "self.last_mutation_at.checked_add(quiet.millis())",
        "Some(candidate)ifcandidate.millis()<self.deadline.millis()=>Some(candidate)",
        "Some(_)|None=>Some(self.deadline)",
    ] {
        if !settle.contains(required) {
            return Err(format!(
                "semantic settlement lost exact no-poll wake contract {required}"
            ));
        }
    }
    for forbidden in ["std::thread", "std::time::Instant", "sleep(", "interval("] {
        if settle.contains(forbidden) {
            return Err(format!(
                "semantic settlement core acquired imperative timer surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_semantic_settlement_coordinator(root: &str, coordinator: &str) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "modsemantic_settle_coordinator;",
        "SemanticActionSettlementCoordinator",
        "SemanticActionSettlementReservation",
        "SemanticActionSettlementTerminal",
        "MAX_PENDING_SEMANTIC_ACTION_SETTLEMENTS",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost bounded semantic settlement owner {required}"
            ));
        }
    }

    let coordinator = compact(coordinator);
    for required in [
        "MAX_PENDING_SEMANTIC_ACTION_SETTLEMENTS:usize=MAX_AGENT_PENDING_EFFECTS",
        "pending:Vec<SettlementEntry>",
        "start:SemanticActionSettlementStart",
        "pubstructSemanticActionSettlementTerminal{",
        "Terminal(Box<SemanticActionSettlementTerminal>)",
        "SemanticActionSettlementTerminal::new(",
        "pub(crate)fninto_parts(self",
        "(AgentActiveEffect,SemanticActionExecutionApplied,SemanticSettleTracker",
        "pubstructSemanticActionSettlementAdmissionRefusal{",
        "pubconstfnaction_failure(&self)->SemanticActionFailure",
        "SemanticActionSettlementCoordinatorError::ContextBusy|SemanticActionSettlementCoordinatorError::Capacity=>{SemanticActionFailure::ResourceExhausted}",
        "(SemanticActionSettlementStart,SemanticActionSettlementCoordinatorError",
        "next_wake:SemanticSettleInstant",
        "start.tracker().status().is_terminal()",
        "entry.key.context()==key.context()",
        "self.pending.len()>=MAX_PENDING_SEMANTIC_ACTION_SETTLEMENTS",
        "self.pending.push(SettlementEntry{key,start})",
        "self.pending.remove(index).start",
        "self.pending[index].start.tracker().next_wake()!=Some(reservation.next_wake)",
        "SemanticActionSettlementCoordinatorError::ScheduleMismatch",
        "SemanticSettleFact::Tick",
        "now<reservation.next_wake",
        "pubfnobserve_snapshot(",
        "self.sealed=true",
    ] {
        if !coordinator.contains(required) {
            return Err(format!(
                "semantic settlement coordinator lost required bound {required}"
            ));
        }
    }
    for (return_type, label) in [
        (
            "(AgentActiveEffect,SemanticActionExecutionApplied,SemanticSettleTracker)",
            "settlement terminal",
        ),
        (
            "(SemanticActionSettlementStart,SemanticActionSettlementCoordinatorError)",
            "settlement admission refusal",
        ),
    ] {
        if !has_crate_private_into_parts(&coordinator, return_type) {
            return Err(format!(
                "semantic settlement coordinator exposed or lost {label} ownership"
            ));
        }
    }
    for forbidden in [
        "evaluateJavaScript",
        "callAsyncJavaScript",
        "querySelector",
        "CGEvent",
        "NSEvent",
        "Input.dispatch",
        "WKWebView",
        "WebView2",
        "std::thread",
        "std::time",
        "std::fs",
        "tokio::",
        "Mutex<",
        "Arc<",
        "sleep(",
        "interval(",
        "pubfninto_authority(",
    ] {
        if coordinator.contains(forbidden) {
            return Err(format!(
                "semantic settlement coordinator acquired forbidden work/program surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_semantic_terminal_verification(
    root: &str,
    verification: &str,
    policy: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "verify_semantic_action_terminal",
        "SemanticActionVerificationRefusal",
        "SemanticActionVerifiedTerminal",
        "SemanticSnapshotEvidenceError",
        "prepare_semantic_action_snapshot_evidence",
        "AgentFailedSemanticEffect",
        "AgentVerifiedSemanticEffect",
        "#[cfg(test)]pub(crate)usesemantic_verify::verify_semantic_action;",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost consuming terminal verification {required}"
            ));
        }
    }

    let verification = compact(verification);
    for required in [
        "pubstructSemanticActionVerifiedTerminal{",
        "active:AgentActiveEffect",
        "execution:SemanticActionExecutionApplied",
        "settlement:SemanticSettleTracker",
        "verified:SemanticVerifiedAction",
        "pubstructSemanticActionVerificationRefusal{",
        "terminal:Box<SemanticActionSettlementTerminal>",
        "observed_at:SemanticSettleInstant",
        "pubfnverify_semantic_action_terminal(",
        "terminal:SemanticActionSettlementTerminal",
        "letobserved_at=evidence.observed_at();",
        "letverified=matchverify_semantic_action(terminal.tracker(),action,evidence)",
        "let(active,execution,settlement)=terminal.into_parts();",
        "pub(crate)fnverify_semantic_action(",
        "pub(crate)constfnexact_target_value(",
        "pubenumSemanticSnapshotEvidenceError{",
        "NonSnapshotEvidenceRequired",
        "pubfnprepare_semantic_action_snapshot_evidence",
    ] {
        if !verification.contains(required) {
            return Err(format!(
                "semantic verification lost exact terminal join {required}"
            ));
        }
    }
    for forbidden in [
        "pubfnverify_semantic_action(",
        "pubconstfnexact_target_value(",
        "pubfninto_parts(",
        "into_terminal(",
        "evaluateJavaScript",
        "callAsyncJavaScript",
        "querySelector",
        "CGEvent",
        "NSEvent",
        "Input.dispatch",
        "std::thread",
        "std::time",
        "std::fs",
        "tokio::",
        "Mutex<",
        "Arc<",
        "retry(",
    ] {
        if verification.contains(forbidden) {
            return Err(format!(
                "semantic terminal verification acquired forbidden escape/work surface {forbidden}"
            ));
        }
    }

    if verification.matches("pub(crate)fninto_parts(").count() < 2 {
        return Err(
            "verified and refused terminals must remain opaque until policy settlement".to_owned(),
        );
    }

    let policy = compact(policy);
    for required in [
        "pubstructAgentVerifiedSemanticEffect{",
        "pubstructAgentFailedSemanticEffect{",
        "pubfnsettle_verified_semantic_terminal(",
        "terminal:SemanticActionVerifiedTerminal",
        "let(active,execution,settlement,verified)=terminal.into_parts();",
        "self.settle_verified_semantic_effect(active,action,&verified)?",
        "pubfnsettle_refused_semantic_terminal(",
        "refusal:SemanticActionVerificationRefusal",
        "let(active,execution,settlement,verification_observed_at,verification_error)=refusal.into_parts();",
        "verification_observed_at:SemanticSettleInstant",
        "verification_error.action_failure()",
        "pub(crate)fnsettle_verified_semantic_effect(",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "effect policy lost consuming verification settlement {required}"
            ));
        }
    }
    for forbidden in [
        "pubfnsettle_verified_semantic_effect(",
        "std::thread",
        "std::time",
        "std::fs",
        "tokio::",
        "retry(",
    ] {
        if policy.contains(forbidden) {
            return Err(format!(
                "effect policy acquired forbidden raw-verification/work surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_accounted_action_result(
    root: &str,
    policy: &str,
    result: &str,
    batch: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "finalize_accounted_semantic_action_result",
        "AgentAccountedSemanticActionResult",
        "AgentAccountedSemanticActionResultRefusal",
        "SemanticActionBatchAdmissionRefusal",
        "SemanticActionBatchFailureAdmissionRefusal",
        "SemanticActionBatchFailureStage",
        "MAX_SEMANTIC_ACTION_BATCH_COMPLETION_BYTES",
        "MAX_SEMANTIC_ACTION_BATCH_FAILURE_BYTES",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost accounted action-result finalization {required}"
            ));
        }
    }

    let policy = compact(policy);
    for required in [
        "pubstructAgentEffectReceipt{",
        "action_guard:[u8;32]",
        "pubstructAgentVerifiedSemanticEffect{",
        "pubstructAgentFailedSemanticEffect{",
        "evidence:Option<Box<AgentFailedSemanticEffectEvidence>>",
        "pubfnsettle_execution_admission_refusal(",
        "refusal:SemanticActionExecutionCoordinatorRefusal",
        "pubfnsettle_settlement_start_refusal(",
        "refusal:SemanticActionSettlementRefusal",
        "pubfnsettle_settlement_admission_refusal(",
        "refusal:SemanticActionSettlementAdmissionRefusal",
        "letfailure=refusal.action_failure();",
        "pub(crate)fnsettle_failed_semantic_effect(",
        "action:&SemanticPreparedAction",
        "Result<AgentFailedSemanticEffect,AgentPolicyError>",
        "if!active.matches_action(action)",
        "pub(crate)fninto_parts(",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "verified effect escaped exact result finalization {required}"
            ));
        }
    }
    if policy
        .matches("letfailure=refusal.action_failure();")
        .count()
        < 3
    {
        return Err(
            "every pre-verification refusal must derive its own closed action failure".to_owned(),
        );
    }
    if policy.contains("pubfnsettle_failed_semantic_effect(") {
        return Err("effect policy exposed raw caller-selected failure settlement".to_owned());
    }

    let result = compact(result);
    for required in [
        "pubstructAgentAccountedSemanticActionResult{",
        "receipt:AgentEffectReceipt",
        "result:SemanticActionResult",
        "pubstructAgentAccountedSemanticActionResultRefusal{",
        "accounted:Box<AgentVerifiedSemanticEffect>",
        "current:Box<SemanticPostActionObservation>",
        "pubfnfinalize_accounted_semantic_action_result(",
        "accounted:AgentVerifiedSemanticEffect",
        "ifletErr(error)=validate_semantic_action_result(",
        "let(receipt,execution,settlement,verified)=accounted.into_parts();",
        "letresult=finish_semantic_action_result(",
        "pub(crate)fninto_parts(",
        "#[cfg(test)]pub(crate)fnfinalize_semantic_action_result(",
        "current:&SemanticPostActionObservation",
    ] {
        if !result.contains(required) {
            return Err(format!(
                "accounted action result lost exact proof/current-state join {required}"
            ));
        }
    }
    for forbidden in [
        "pubfnfinalize_semantic_action_result(",
        "std::thread",
        "std::time",
        "std::fs",
        "tokio::",
        "Mutex<",
        "Arc<",
        "retry(",
    ] {
        if result.contains(forbidden) {
            return Err(format!(
                "accounted action result acquired forbidden loose/work surface {forbidden}"
            ));
        }
    }

    let batch = compact(batch);
    for required in [
        "pubstructSemanticActionBatchAdmissionRefusal{",
        "accounted:Box<AgentAccountedSemanticActionResult>",
        "pubfnrecord_success(",
        "accounted:AgentAccountedSemanticActionResult",
        "ifletErr(error)=self.validate_success(action,accounted.result())",
        "receipt:AgentEffectReceipt",
        "execution:SemanticActionExecutionApplied",
        "settlement_event_count:u16",
        "settlement_elapsed_millis:u64",
        "settlement_terminal_at:SemanticSettleInstant",
        "verification_observed_at:Option<SemanticSettleInstant>",
        "pubconstfnverification_observed_at(self)->Option<SemanticSettleInstant>",
        "pubconstMAX_SEMANTIC_ACTION_BATCH_COMPLETION_BYTES:usize=512;",
        "size_of::<SemanticActionBatchCompletion>()<=MAX_SEMANTIC_ACTION_BATCH_COMPLETION_BYTES",
        "pubconstMAX_SEMANTIC_ACTION_BATCH_FAILURE_BYTES:usize=512;",
        "pubstructSemanticActionBatchFailure{",
        "failure:Option<SemanticActionBatchFailure>",
        "size_of::<SemanticActionBatchFailure>()<=MAX_SEMANTIC_ACTION_BATCH_FAILURE_BYTES",
        "AgentEffectSettlement::Verified(verified_proof)",
        "SemanticSettleStatus::ReadyForVerification",
        "let(_,_,_,result)=accounted.into_parts();",
        "pubstructSemanticActionBatchFailureAdmissionRefusal{",
        "execution:Box<SemanticActionBatchExecution>",
        "failed:Box<AgentFailedSemanticEffect>",
        "pubfnfail(self,action:&SemanticPreparedAction,failed:AgentFailedSemanticEffect,)",
        "ifletErr(error)=self.validate_failure(action,&failed)",
        "let(receipt,failure,evidence)=failed.into_parts();",
        "AccountingMismatch",
        "pubfninto_parts(",
        "AgentAccountedSemanticActionResult,SemanticActionBatchExecutionError",
    ] {
        if !batch.contains(required) {
            return Err(format!(
                "accounted batch aggregation lost exact receipt/state join {required}"
            ));
        }
    }
    let validate = batch
        .find("ifletErr(error)=self.validate_success(action,accounted.result())")
        .ok_or_else(|| "accounted batch aggregation lost validation".to_owned())?;
    let consume = batch
        .find("let(_,_,_,result)=accounted.into_parts();")
        .ok_or_else(|| "accounted batch aggregation lost consuming transition".to_owned())?;
    if validate >= consume {
        return Err("accounted batch aggregation consumed state before validation".to_owned());
    }
    let failure_validate = batch
        .find("ifletErr(error)=self.validate_failure(action,&failed)")
        .ok_or_else(|| "accounted batch failure lost validation".to_owned())?;
    let failure_consume = batch
        .find("let(receipt,failure,evidence)=failed.into_parts();")
        .ok_or_else(|| "accounted batch failure lost consuming transition".to_owned())?;
    if failure_validate >= failure_consume {
        return Err("accounted batch failure consumed evidence before validation".to_owned());
    }
    for forbidden in [
        "result:SemanticActionResult)->Result<SemanticActionBatchContinuation",
        "pubfnfail(self,failure:SemanticActionFailure)",
        "std::thread",
        "std::time",
        "std::fs",
        "tokio::",
        "Mutex<",
        "Arc<",
        "retry(",
    ] {
        if batch.contains(forbidden) {
            return Err(format!(
                "accounted batch aggregation acquired forbidden loose/work surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_provider_input_evidence_contract(
    root: &str,
    request: &str,
    provider: &str,
    policy: &str,
    diff_model: &str,
    transport: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "AgentCommittedProviderInput",
        "AgentPreparedDiffRequest",
        "AgentProviderInputEvidence",
        "AgentProviderInputMetrics",
        "AgentProviderInputTokenCount",
        "AgentProviderSemanticInputStats",
        "AgentProviderLocalInputTokenCounter",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost provider input continuation export {required}"
            ));
        }
    }

    let request = compact(request);
    for required in [
        "pubenumAgentProviderInputEvidence",
        "Observation(SemanticObservationAcknowledgement)",
        "Diff(SemanticDiffDeliveryReceipt)",
        "Read(SemanticReadDeliveryReceipt)",
        "pubstructAgentCommittedProviderInput",
        "evidence:AgentProviderInputEvidence::Observation(acknowledgement)",
        "evidence:AgentProviderInputEvidence::Diff(receipt)",
        "evidence:AgentProviderInputEvidence::Read(receipt)",
        "pubenumAgentProviderSemanticInputStats",
        "Observation(SemanticEncodingStats)",
        "Diff(SemanticDiffEncodingStats)",
        "Locate(SemanticLocateEncodingStats)",
        "Read(SemanticReadEncodingStats)",
        "Extraction(SemanticExtractionEncodingStats)",
        "Screenshot(SemanticScreenshotStats)",
        "pubstructAgentProviderInputTokenCount{tokens:u32,quality:SemanticTokenCountQuality",
        "pubstructAgentProviderInputMetrics{serialized_request_bytes:u32,semantic:AgentProviderSemanticInputStats,semantic_payload_tokens:Option<AgentProviderInputTokenCount>,structured_input_tokens:Option<AgentProviderInputTokenCount>",
        "assert!(MAX_AGENT_PROVIDER_REQUEST_BYTES<=u32::MAXasusize)",
        "assert!(std::mem::size_of::<AgentProviderInputMetrics>()<=64)",
        "Committed(Box<AgentCommittedProviderInput>)",
        "pubconstfnmetrics(&self)->AgentProviderInputMetrics",
        "pubconstfninput_evidence(&self)->&AgentProviderInputEvidence",
        "pubconstfninput_metrics(&self)->AgentProviderInputMetrics",
        "commitment.commit(policy,input_metrics)",
        "commitment.settle(policy,settlement,input_metrics)",
        "AgentProviderRequestSettlement::Committed=>Ok(AgentProviderInputOutcome::Committed(",
        "Box::new(self.commit(policy,metrics)?)",
        "AgentProviderSemanticInputStats::Observation(self.semantic_stats)",
        "AgentProviderSemanticInputStats::Diff(self.semantic_stats)",
        "AgentProviderSemanticInputStats::Locate(self.semantic_stats)",
        "AgentProviderSemanticInputStats::Read(self.semantic_stats)",
        "AgentProviderSemanticInputStats::Extraction(self.semantic_stats)",
        "AgentProviderSemanticInputStats::Screenshot(self.screenshot_stats)",
        "pubtraitAgentProviderLocalInputTokenCounter",
        "fncount_openai_responses_input(",
        "fncount_anthropic_messages_input(",
        "pubstructAgentProviderDiffRequestDraft",
        "counter:&dynAgentProviderLocalInputTokenCounter",
        "config.validate_diff_request(",
        "policy.prepare_provider_diff_input(",
        "pubstructAgentPreparedDiffRequest",
        "commitment:AgentProviderInputCommitment::Diff",
        "continuation_transcript:Some(self.continuation_transcript)",
    ] {
        if !request.contains(required) {
            return Err(format!(
                "provider request lost exact admitted input continuation seam {required}"
            ));
        }
    }
    if request
        .matches("pubconstfninput_metrics(&self)->AgentProviderInputMetrics")
        .count()
        != 1
    {
        return Err(
            "provider request metrics must surface only after exact disclosure commit".to_owned(),
        );
    }
    let metrics_start = request
        .find("pubenumAgentProviderSemanticInputStats")
        .ok_or_else(|| "provider request lost semantic metrics boundary".to_owned())?;
    let metrics_end = request[metrics_start..]
        .find("implAgentProviderInputEvidence")
        .map(|offset| metrics_start + offset)
        .ok_or_else(|| "provider request metrics boundary is unclosed".to_owned())?;
    let metrics = &request[metrics_start..metrics_end];
    for forbidden in [
        "String,",
        "Vec<",
        "Arc<",
        "Box<",
        "body:",
        "content:",
        "Serialize",
        "Deserialize",
    ] {
        if metrics.contains(forbidden) {
            return Err(format!(
                "provider input metrics acquired content or serialization storage {forbidden}"
            ));
        }
    }
    for forbidden in ["ZDIFF1"] {
        if request.contains(forbidden) {
            return Err(format!(
                "provider diff request regained standalone or remote-count authority {forbidden}"
            ));
        }
    }

    let provider = compact(provider);
    for required in [
        "fnvalidate_diff_request(",
        "structured_input.quality()!=crate::SemanticTokenCountQuality::ExactLocal",
        "u64::from(structured_input.tokens())>allowed_input_tokens",
        "!self.pricing_profile().contains(u64::from(structured_input.tokens()))",
    ] {
        if !provider.contains(required) {
            return Err(format!(
                "provider diff request lost exact local whole-input bound {required}"
            ));
        }
    }

    let policy = compact(policy);
    for required in [
        "fnprepare_provider_diff_input(",
        "request.id()!=expected.call",
        "request.lease()!=expected.lease",
        "self.manifest.id()!=expected.manifest",
        "delivery.matches_diff(diff)",
        "measured:structured_input_tokens,additional:0",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "provider diff policy lost exact authority or reservation join {required}"
            ));
        }
    }

    let diff_model = compact(diff_model);
    for required in [
        "structSemanticDiffDeliveryAuthority{measurement:SemanticTokenMeasurement",
        "fnmatches_diff(&self,diff:&SemanticDiff)->bool",
        "measurement:self.measurement",
    ] {
        if !diff_model.contains(required) {
            return Err(format!(
                "semantic diff delivery lost its exact payload/count binding {required}"
            ));
        }
    }

    let transport = compact(transport);
    for required in [
        "pubfninput_evidence(&self)->Option<&AgentProviderInputEvidence>",
        "Some(AgentProviderAttemptCore::PreDispatch{committed,..})=>{Some(committed.input_evidence())}",
        "Some(AgentProviderAttemptCore::Generation(terminal))=>Some(terminal.input.evidence())",
        "pubfninput_metrics(&self)->Option<AgentProviderInputMetrics>",
        "Some(AgentProviderAttemptCore::PreDispatch{committed,..})=>{Some(committed.input_metrics())}",
        "Some(AgentProviderAttemptCore::Generation(terminal))=>Some(terminal.input.metrics())",
    ] {
        if !transport.contains(required) {
            return Err(format!(
                "provider transport lost content-free input continuation seam {required}"
            ));
        }
    }
    Ok(())
}

fn validate_progress_manifest_revision_contract(
    policy: &str,
    effect_policy: &str,
    supervisor: &str,
    progress: &str,
    audit: &str,
) -> Result<(), String> {
    let policy = compact(policy);
    for required in [
        "pubstructAgentActiveModelCall{manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "pubstructAgentModelCallReceipt{manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "Ok(AgentActiveModelCall{manifest:admission.manifest,manifest_guard:self.manifest.guard()",
        "Ok(AgentModelCallReceipt{manifest:self.manifest.id(),manifest_guard:self.manifest.guard()",
        "!active.matches_manifest_revision(self.manifest.id(),self.manifest.guard())",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "agent model progress value lost exact manifest revision binding {required}"
            ));
        }
    }
    if policy
        .matches("pub(crate)fnmatches_manifest_revision(")
        .count()
        != 2
        || policy
            .matches("self.manifest==manifest&&self.manifest_guard==manifest_guard")
            .count()
            != 2
    {
        return Err(
            "agent model progress values lost canonical manifest revision matching".to_owned(),
        );
    }

    let effect_policy = compact(effect_policy);
    for required in [
        "pubstructAgentNeedsHumanTransition{manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "pubstructAgentEffectPermit{approval_binding:Option<crate::SemanticApprovalBinding>,manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "pubstructAgentActiveEffect{approval_binding:Option<crate::SemanticApprovalBinding>,manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "pubstructAgentEffectReceipt{manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "AgentEffectAuthorization::Permit(AgentEffectPermit{approval_binding:assessment.approval_binding,manifest:self.manifest.id(),manifest_guard:self.manifest.guard()",
        "Ok(AgentActiveEffect{approval_binding:permit.approval_binding,manifest:permit.manifest,manifest_guard:permit.manifest_guard",
        "Ok(AgentEffectReceipt{manifest:self.manifest.id(),manifest_guard:active.manifest_guard",
        "AgentEffectAuthorization::NeedsHuman(AgentNeedsHumanTransition{manifest:self.manifest.id(),manifest_guard:self.manifest.guard()",
    ] {
        if !effect_policy.contains(required) {
            return Err(format!(
                "agent effect progress value lost exact manifest revision binding {required}"
            ));
        }
    }
    if effect_policy
        .matches("pub(crate)fnmatches_manifest_revision(")
        .count()
        != 4
        || effect_policy
            .matches("self.manifest==manifest&&self.manifest_guard==manifest_guard")
            .count()
            != 4
    {
        return Err(
            "agent effect progress values lost canonical manifest revision matching".to_owned(),
        );
    }
    if effect_policy
        .matches("matches_manifest_revision(self.manifest.id(),self.manifest.guard())")
        .count()
        != 3
    {
        return Err(
            "agent effect lifecycle stopped rejoining the canonical manifest revision".to_owned(),
        );
    }

    let supervisor = compact(supervisor);
    if !supervisor.contains("pub(super)constfnmanifest_guard(&self)->[u8;32]{self.manifest_guard}")
    {
        return Err(
            "agent supervisor stopped retaining its private manifest revision guard".to_owned(),
        );
    }
    if !supervisor.contains(
        "pub(super)fnmatches_manifest_revision(&self,manifest:AgentRunManifestId,manifest_guard:[u8;32],)->bool{self.manifest==manifest&&self.manifest_guard==manifest_guard}",
    ) {
        return Err(
            "agent supervisor lost its exact private manifest revision comparison".to_owned(),
        );
    }

    let progress = compact(progress);
    let join = "matches_manifest_revision(self.topology.manifest(),self.topology.manifest_guard())";
    if progress.matches(join).count() != 9 {
        return Err(
            "all nine supervisor progress admissions must join the exact manifest revision"
                .to_owned(),
        );
    }
    if !compact(audit).contains(
        "!supervisor.topology().matches_manifest_revision(self.manifest,self.manifest_guard)",
    ) {
        return Err(
            "agent audit admission must rejoin the exact supervisor manifest revision".to_owned(),
        );
    }
    Ok(())
}

fn validate_provider_call_identity_progress_boundary(
    policy: &str,
    provider: &str,
    continuation: &str,
    progress: &str,
) -> Result<(), String> {
    let policy = compact(policy);
    for required in [
        "pubstructAgentModelCallAdmission{manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "manifest_guard:self.manifest.guard(),",
        "pub(crate)constfnmanifest_guard_for_provider(&self)->[u8;32]{self.manifest_guard}",
    ] {
        if !policy.contains(required) {
            return Err(format!(
                "model admission lost its private manifest guard for provider correlation {required}"
            ));
        }
    }

    let provider = compact(provider);
    for required in [
        "pubstructAgentProviderCallIdentity{manifest:AgentRunManifestId,manifest_guard:[u8;32]",
        "pub(crate)fnfrom_admission(admission:&AgentModelCallAdmission)->Self",
        "manifest_guard:admission.manifest_guard_for_provider()",
        "pub(crate)fnfrom_active(active:&AgentActiveModelCall)->Self",
        "manifest_guard:active.manifest_guard_for_metrics()",
        "pub(crate)fnmatches_manifest_revision(self,manifest:AgentRunManifestId,manifest_guard:[u8;32],)->bool{self.manifest==manifest&&self.manifest_guard==manifest_guard}",
        "pub(crate)constfnmanifest_guard_for_continuation(self)->[u8;32]{self.manifest_guard}",
        "field(\"manifest_guard\",&\"[redacted]\")",
    ] {
        if !provider.contains(required) {
            return Err(format!(
                "provider call identity lost its private guarded projection {required}"
            ));
        }
    }
    for forbidden in [
        "pubfnfrom_admission(",
        "pubfnfrom_active(",
        "pubfnmatches_active(",
        "pubfnmanifest_guard(",
        "pubconstfnmanifest_guard(",
        "pubfnmanifest_guard_for_continuation(",
        "pubconstfnmanifest_guard_for_continuation(",
    ] {
        if provider.contains(forbidden) {
            return Err(format!(
                "provider call identity exposed policy or manifest authority {forbidden}"
            ));
        }
    }

    let continuation = compact(continuation);
    if continuation
        .matches("next_call.matches_manifest_revision(self.prior_call.manifest(),self.prior_call.manifest_guard_for_continuation(),)")
        .count()
        != 5
    {
        return Err(
            "all provider continuation lineages must retain the private manifest guard".to_owned(),
        );
    }

    let progress = compact(progress);
    for required in [
        "pubfnrecord_active_model_call(&mutself,execution:&AgentNodeExecution,call:crate::AgentProviderCallIdentity,)->Result<AgentSemanticProgress,AgentSupervisorRuntimeError>",
        "call.matches_manifest_revision(self.topology.manifest(),self.topology.manifest_guard())",
        "Some(AgentProgressResource::ModelCall(call.call()))",
    ] {
        if !progress.contains(required) {
            return Err(format!(
                "supervisor model progress lost guarded call projection {required}"
            ));
        }
    }
    Ok(())
}

fn validate_profile_lease_release_contract(
    root: &str,
    profile_lease: &str,
    supervisor_context: &str,
) -> Result<(), String> {
    let root = compact(root);
    for required in [
        "AgentSupervisorContextRelease,",
        "AgentSupervisorContextReleaseOutcome,",
        "ContextProfileLeaseRegistry,",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost profile cleanup proof export {required}"
            ));
        }
    }

    let profile_lease = profile_lease
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(profile_lease, |(production, _)| production);
    let profile_lease = compact(profile_lease);
    for required in [
        "proof:AgentSupervisorContextRelease",
        "proof.assignment().identity()!=lease.identity",
        "profile_release_outcome_matches(lease.purpose,proof.outcome())",
        "ContextProfileLeaseError::ReleaseProof",
        "(_,AgentSupervisorContextReleaseOutcome::QueuedCancelled)",
        "terminal:ContextTerminal::Closed,resource:ContextResourceDisposition::Destroyed",
        "terminal:ContextTerminal::Adopted,resource:ContextResourceDisposition::TransferredToBrowse",
        "terminal:ContextTerminal::Released,resource:ContextResourceDisposition::ExistingBrowseRetained",
        "ContextProfileLeasePurpose::HumanSignInHandoff,AgentSupervisorContextReleaseOutcome::Retired{terminal:ContextTerminal::Released,resource:ContextResourceDisposition::Destroyed",
    ] {
        if !profile_lease.contains(required) {
            return Err(format!(
                "profile lease release lost exact cleanup proof boundary {required}"
            ));
        }
    }
    if profile_lease.contains(
        "pubfnrelease(&mutself,lease:ContextProfileLease)->Result<ContextIdentity,ContextProfileLeaseError>",
    ) || profile_lease.matches("pubfnrelease(").count() != 1
    {
        return Err("profile leases regained a bare or ambiguous release path".to_owned());
    }
    let proof_check = profile_lease
        .find("ifproof.assignment().identity()!=lease.identity")
        .ok_or_else(|| "profile lease cleanup proof check is missing".to_owned())?;
    let removal = profile_lease
        .find("letremoved=self.leases.remove(&lease.identity.id())")
        .ok_or_else(|| "profile lease exact removal is missing".to_owned())?;
    if proof_check >= removal {
        return Err("profile lease removal must follow exact cleanup proof".to_owned());
    }

    let release_declaration = supervisor_context
        .find("pub struct AgentSupervisorContextRelease")
        .ok_or_else(|| "supervisor profile cleanup receipt is missing".to_owned())?;
    let release_attribute_start = supervisor_context[..release_declaration]
        .rfind("\n\n")
        .map_or(0, |start| start + 2);
    let release_attributes = &supervisor_context[release_attribute_start..release_declaration];
    for forbidden in ["Serialize", "Deserialize", "Default"] {
        if release_attributes.contains(forbidden) {
            return Err(format!(
                "supervisor profile cleanup receipt acquired forgeable derive {forbidden}"
            ));
        }
    }

    let supervisor_context = compact(supervisor_context);
    for required in [
        "pubstructAgentSupervisorContextRelease{assignment:AgentSupervisorContextAssignment,outcome:AgentSupervisorContextReleaseOutcome,}",
        "letidentity=registry.cancel_queued(context)?;",
        "letretired=registry.reap_terminal(context)?;",
        "#[cfg(test)]pub(crate)fnfor_profile_lease_test(",
    ] {
        if !supervisor_context.contains(required) {
            return Err(format!(
                "supervisor profile cleanup receipt lost constructor-closed registry proof {required}"
            ));
        }
    }
    if supervisor_context
        .matches("Ok(AgentSupervisorContextRelease{")
        .count()
        != 2
    {
        return Err(
            "supervisor must emit profile cleanup receipts only from queued cancel and terminal reap"
                .to_owned(),
        );
    }
    let release_impl_start = supervisor_context
        .find("implAgentSupervisorContextRelease{")
        .ok_or_else(|| "supervisor profile cleanup receipt impl is missing".to_owned())?;
    let release_impl_end = supervisor_context[release_impl_start..]
        .find("pub(super)structSupervisorContextRow")
        .map(|offset| release_impl_start + offset)
        .ok_or_else(|| "supervisor profile cleanup receipt impl boundary is missing".to_owned())?;
    let release_impl = &supervisor_context[release_impl_start..release_impl_end];
    if release_impl.matches("pubfn").count() != 0
        || release_impl.matches("pub(crate)fn").count() != 1
        || release_impl.matches("Self{assignment:").count() != 1
    {
        return Err(
            "supervisor profile cleanup receipt acquired an additional constructor surface"
                .to_owned(),
        );
    }
    for forbidden in [
        "implAgentSupervisorContextRelease{pubfnnew(",
        "implAgentSupervisorContextRelease{pubconstfnnew(",
        "SerializeforAgentSupervisorContextRelease",
        "DeserializeforAgentSupervisorContextRelease",
        "DefaultforAgentSupervisorContextRelease",
    ] {
        if supervisor_context.contains(forbidden) {
            return Err(format!(
                "supervisor profile cleanup receipt acquired forging surface {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_context_shutdown_retention_contract(source: &str) -> Result<(), String> {
    let source = source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production);
    let source = compact(source);

    let seal_start = source
        .find("pubfnseal_for_shutdown(")
        .ok_or_else(|| "context shutdown seal is missing".to_owned())?;
    let seal_end = source[seal_start..]
        .find("pubfnshutdown_targets(")
        .map(|offset| seal_start + offset)
        .ok_or_else(|| "context shutdown seal boundary is missing".to_owned())?;
    let seal = &source[seal_start..seal_end];
    for required in [
        "self.sealed=true;",
        "RegistryRow::Queued(queued)=>Some(queued.identity)",
        "RegistryRow::Active(_)=>None",
        "self.validate()?;Ok(queued)",
    ] {
        if !seal.contains(required) {
            return Err(format!(
                "context shutdown seal lost retained queued cleanup authority {required}"
            ));
        }
    }
    for forbidden in ["self.rows.remove(", "Vec::with_capacity("] {
        if seal.contains(forbidden) {
            return Err(format!(
                "context shutdown seal discards queued cleanup authority {forbidden}"
            ));
        }
    }

    let cancel_start = source
        .find("pubfncancel_queued(")
        .ok_or_else(|| "queued context cancellation is missing".to_owned())?;
    let cancel_end = source[cancel_start..]
        .find("pubfnbegin_context(")
        .map(|offset| cancel_start + offset)
        .ok_or_else(|| "queued context cancellation boundary is missing".to_owned())?;
    let cancel = &source[cancel_start..cancel_end];
    for required in [
        "letrow=self.rows.remove(&context).ok_or(ContextRegistryError::NotFound)?;",
        "RegistryRow::Queued(queued)=>{self.validate()?;Ok(queued.identity)}",
        "active@RegistryRow::Active(_)=>{self.rows.insert(context,active);Err(ContextRegistryError::NotQueued)}",
    ] {
        if !cancel.contains(required) {
            return Err(format!(
                "sealed queued context cancellation lost exact cleanup behavior {required}"
            ));
        }
    }
    if cancel.contains("self.sealed") {
        return Err(
            "queued context cleanup must remain available behind the shutdown admission seal"
                .to_owned(),
        );
    }
    if source.contains(
        "RegistryRow::Queued(queued)=>{ifself.sealed||queued.capabilities.kind()!=identity.kind()",
    ) || !source
        .contains("RegistryRow::Queued(queued)=>{ifqueued.capabilities.kind()!=identity.kind()")
    {
        return Err(
            "context registry validation must admit retained queued rows after shutdown seal"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_agent_native_shutdown_coordinator(
    root: &str,
    lifecycle: &str,
    shutdown: &str,
    screenshot: &str,
) -> Result<(), String> {
    fn production(source: &str) -> &str {
        source
            .split_once("\n#[cfg(test)]\nmod tests")
            .map_or(source, |(production, _)| production)
    }

    let root = compact(root);
    for required in [
        "modagent_lifecycle;",
        "modagent_native_shutdown;",
        "AgentBrowserLifecycle,",
        "AgentBrowserShutdownOutcome",
        "AgentNativeShutdownCoordinator,",
        "AgentNativeShutdownProof,",
        "AgentNativeShutdownResources,",
        "MAX_AGENT_NATIVE_SHUTDOWN_AUDITS,",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost native shutdown coordinator export {required}"
            ));
        }
    }

    let lifecycle_production = production(lifecycle);
    let outcome_declaration = lifecycle_production
        .find("pub enum AgentBrowserShutdownOutcome")
        .ok_or_else(|| "agent browser shutdown outcome is missing".to_owned())?;
    let outcome_attribute_start = lifecycle_production[..outcome_declaration]
        .rfind("\n\n")
        .map_or(0, |start| start + 2);
    let outcome_attributes = &lifecycle_production[outcome_attribute_start..outcome_declaration];
    for forbidden in ["Clone", "Copy", "Default", "Serialize", "Deserialize"] {
        if outcome_attributes.contains(forbidden) {
            return Err(format!(
                "agent browser shutdown outcome acquired forgeable derive {forbidden}"
            ));
        }
    }
    let lifecycle_without_comments = lifecycle_production
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<String>();
    let lifecycle = compact(&lifecycle_without_comments);
    for required in [
        "pubenumAgentBrowserShutdownOutcome{Clean(AgentNativeShutdownProof),Unclean,}",
        "pubfninto_native_proof(self)->Option<AgentNativeShutdownProof>{matchself{Self::Clean(proof)=>Some(proof),Self::Unclean=>None,}}",
        "pubtraitAgentBrowserLifecycle:Send{fnshutdown_until(self:Box<Self>,deadline:Instant)->AgentBrowserShutdownOutcome;}",
    ] {
        if !lifecycle.contains(required) {
            return Err(format!(
                "agent browser lifecycle lost consuming proof-bound shutdown rule {required}"
            ));
        }
    }

    let screenshot = compact(production(screenshot));
    for required in [
        "pubstructSemanticScreenshotCoordinatorStatus{pending:u8,shutdown_sealed:bool,}",
        "pubstructSemanticScreenshotCoordinator{pending:Vec<PendingScreenshotEntry>,shutdown_sealed:bool,}",
        "ifself.shutdown_sealed{returnErr(SemanticScreenshotCoordinatorError::Shutdown);}",
        "pubfnseal_for_shutdown(&mutself)->SemanticScreenshotCoordinatorStatus{self.shutdown_sealed=true;self.status()}",
        "pubfnis_quiescent(&self)->bool{self.shutdown_sealed&&self.pending.is_empty()}",
        "Shutdown,",
    ] {
        if !screenshot.contains(required) {
            return Err(format!(
                "semantic screenshot shutdown seal lost exact behavior {required}"
            ));
        }
    }
    let screenshot_cancel = screenshot
        .split_once("pubfncancel(")
        .and_then(|(_, suffix)| {
            suffix
                .split_once("pubfnseal_for_shutdown(")
                .map(|(body, _)| body)
        })
        .ok_or_else(|| "semantic screenshot cancellation boundary is missing".to_owned())?;
    if screenshot_cancel.contains("shutdown_sealed") {
        return Err(
            "semantic screenshot exact terminal cleanup must remain available after sealing"
                .to_owned(),
        );
    }

    let shutdown_production = production(shutdown);
    let proof_declaration = shutdown_production
        .find("pub struct AgentNativeShutdownProof")
        .ok_or_else(|| "agent native shutdown proof is missing".to_owned())?;
    let proof_attribute_start = shutdown_production[..proof_declaration]
        .rfind("\n\n")
        .map_or(0, |start| start + 2);
    let proof_attributes = &shutdown_production[proof_attribute_start..proof_declaration];
    for forbidden in ["Clone", "Copy", "Default", "Serialize", "Deserialize"] {
        if proof_attributes.contains(forbidden) {
            return Err(format!(
                "agent native shutdown proof acquired forgeable derive {forbidden}"
            ));
        }
    }

    let shutdown = compact(shutdown_production);
    for required in [
        "pubconstMAX_AGENT_NATIVE_SHUTDOWN_AUDITS:u8=8;",
        "pubstructAgentNativeShutdownResources{contexts:ContextRegistry,profile_leases:ContextProfileLeaseRegistry,cookie_transfers:ContextCookieTransferRegistry,action_executions:SemanticActionExecutionCoordinator,action_settlements:SemanticActionSettlementCoordinator,screenshots:SemanticScreenshotCoordinator,}",
        "if!self.contexts.is_quiescent(){returnSome(AgentNativeShutdownAdmissionError::Contexts);}",
        "if!self.profile_leases.is_quiescent(){returnSome(AgentNativeShutdownAdmissionError::ProfileLeases);}",
        "if!self.cookie_transfers.is_quiescent(){returnSome(AgentNativeShutdownAdmissionError::CookieTransfers);}",
        "if!action_executions.sealed()||action_executions.pending()!=0",
        "if!action_settlements.sealed()||action_settlements.pending()!=0",
        "if!self.screenshots.is_quiescent()",
        "pubfntry_new(resources:AgentNativeShutdownResources,)->Result<Self,Box<AgentNativeShutdownAdmissionRefusal>>",
        "ifletSome(error)=resources.readiness_error(){returnErr(Box::new(AgentNativeShutdownAdmissionRefusal{error,resources,}));}",
        "pubfnbegin_port_seal(&mutself,audit:ContextResourceAuditId,)->Result<(),AgentNativeShutdownError>",
        "pubfnaccount_port_seal(&mutself,audit:ContextResourceAuditId,dispatch:ContextShutdownDispatch,)",
        "ContextShutdownDispatch::AuditScheduled=>{NativeShutdownState::ShutdownAuditPending(audit)}",
        "ContextShutdownDispatch::SealedWithoutAudit(_)=>self.retry_or_exhausted()",
        "pubfnsettle_shutdown_audit(&mutself,settlement:ContextShutdownAuditSettlement,)",
        "pubfnbegin_resource_audit(&mutself,audit:ContextResourceAuditId,)",
        "pubfnaccount_resource_audit(&mutself,audit:ContextResourceAuditId,dispatch:ContextDispatch,)",
        "pubfnsettle_resource_audit(&mutself,settlement:ContextResourceAuditSettlement,)",
        "ifself.last_audit.is_some_and(|last_audit|audit<=last_audit){returnErr(AgentNativeShutdownError::AuditReplay);}",
        "ifattempts>MAX_AGENT_NATIVE_SHUTDOWN_AUDITS{returnErr(AgentNativeShutdownError::Stage);}",
        "ifself.attempts>=MAX_AGENT_NATIVE_SHUTDOWN_AUDITS{NativeShutdownState::Exhausted}else{NativeShutdownState::ResourceAuditRequired}",
        "Ok(snapshot)ifnative_snapshot_is_zero(snapshot)=>{NativeShutdownState::ZeroProven{audit,snapshot}}",
        "Ok(_)|Err(_)=>self.retry_or_exhausted()",
        "pubfnfinish(self)->Result<AgentNativeShutdownProof,Box<AgentNativeShutdownFinishRefusal>>",
        "letNativeShutdownState::ZeroProven{audit,snapshot}=self.stateelse",
        "Ok(AgentNativeShutdownProof{audit,attempts:self.attempts,snapshot,})",
    ] {
        if !shutdown.contains(required) {
            return Err(format!(
                "agent native shutdown coordinator lost exact drain rule {required}"
            ));
        }
    }

    for field in [
        "counts.known_bindings==0",
        "counts.resident_views==0",
        "counts.owned_reservations==0",
        "counts.borrowed_leases==0",
        "counts.visible_surfaces==0",
        "counts.suspended_views==0",
        "counts.pending_operations==0",
        "counts.pending_captures==0",
        "counts.queued_tasks==0",
    ] {
        if !shutdown.contains(field) {
            return Err(format!(
                "agent native shutdown zero proof omitted resource field {field}"
            ));
        }
    }

    for forbidden in [
        "implAgentNativeShutdownProof{pubfnnew(",
        "implAgentNativeShutdownProof{pubconstfnnew(",
        "DefaultforAgentNativeShutdownProof",
        "SerializeforAgentNativeShutdownProof",
        "DeserializeforAgentNativeShutdownProof",
    ] {
        if shutdown.contains(forbidden) {
            return Err(format!(
                "agent native shutdown proof acquired forging surface {forbidden}"
            ));
        }
    }
    if shutdown.matches("Ok(AgentNativeShutdownProof{").count() != 1 {
        return Err("agent native shutdown proof must have one checked construction".to_owned());
    }
    Ok(())
}

fn validate_agent_native_shutdown_driver(root: &str, driver: &str) -> Result<(), String> {
    let production = driver
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(driver, |(production, _)| production);
    let root = compact(root);
    for required in [
        "modagent_native_shutdown_driver;",
        "drive_agent_native_shutdown_until,",
        "AgentNativeShutdownDriveError,",
        "AgentNativeShutdownEventSource,",
        "AgentNativeShutdownWait,",
        "AGENT_NATIVE_SHUTDOWN_RETRY_BASE_MILLIS,",
        "AGENT_NATIVE_SHUTDOWN_RETRY_MAX_MILLIS,",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "agentic root lost native shutdown driver export {required}"
            ));
        }
    }

    let wait_declaration = production
        .find("pub enum AgentNativeShutdownWait")
        .ok_or_else(|| "agent native shutdown wait result is missing".to_owned())?;
    let wait_attribute_start = production[..wait_declaration]
        .rfind("\n\n")
        .map_or(0, |start| start + 2);
    let wait_attributes = &production[wait_attribute_start..wait_declaration];
    for forbidden in ["Clone", "Copy", "Serialize", "Deserialize", "Debug"] {
        if wait_attributes.contains(forbidden) {
            return Err(format!(
                "agent native shutdown wait acquired payload-copying derive {forbidden}"
            ));
        }
    }

    let driver = compact(
        &production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "pubconstAGENT_NATIVE_SHUTDOWN_RETRY_BASE_MILLIS:u64=100;",
        "pubconstAGENT_NATIVE_SHUTDOWN_RETRY_MAX_MILLIS:u64=1_000;",
        "pubenumAgentNativeShutdownWait{Event(Box<ContextNativeEvent>),Elapsed,Closed,}",
        "pubtraitAgentNativeShutdownEventSource:Send{fnwait_until(&mutself,wake:Instant)->AgentNativeShutdownWait;}",
        "pubfndrive_agent_native_shutdown_until(mutcoordinator:AgentNativeShutdownCoordinator,port:&dynAgentBrowserPort,first_audit:ContextResourceAuditId,events:&mutdynAgentNativeShutdownEventSource,deadline:Instant,)->Result<AgentNativeShutdownProof,AgentNativeShutdownDriveError>",
        "coordinator.begin_port_seal(first_audit)",
        "letdispatch=port.seal_for_shutdown(first_audit);",
        "coordinator.account_port_seal(first_audit,dispatch)",
        "letContextNativeEvent::ShutdownAuditSettled(settlement)=eventelse{returnErr(AgentNativeShutdownDriveError::UnexpectedEvent);};",
        "letContextNativeEvent::ResourceAuditSettled(settlement)=eventelse{returnErr(AgentNativeShutdownDriveError::UnexpectedEvent);};",
        "wait_for_retry(events,coordinator.status().attempts(),deadline)?;",
        ".checked_add(1).and_then(ContextResourceAuditId::new)",
        "coordinator.begin_resource_audit(next)",
        "letdispatch=port.audit_resources(next);",
        "coordinator.account_resource_audit(next,dispatch)",
        "AgentNativeShutdownStage::ZeroProven=>{returncoordinator.finish()",
        "AgentNativeShutdownStage::Exhausted=>{returnErr(AgentNativeShutdownDriveError::AttemptsExhausted);}",
        "Self::Event(_)=>formatter.write_str(\"AgentNativeShutdownWait::Event([redacted])\")",
        "AGENT_NATIVE_SHUTDOWN_RETRY_BASE_MILLIS.saturating_mul(1_u64<<shift).min(AGENT_NATIVE_SHUTDOWN_RETRY_MAX_MILLIS)",
    ] {
        if !driver.contains(required) {
            return Err(format!(
                "agent native shutdown driver lost bounded drain rule {required}"
            ));
        }
    }
    if driver.matches("ifInstant::now()>=deadline").count() < 4 {
        return Err(
            "agent native shutdown driver must recheck its absolute deadline around dispatch"
                .to_owned(),
        );
    }
    for forbidden in [
        "std::thread",
        "thread::",
        "sleep(",
        "spawn(",
        "channel(",
        "mpsc::",
        "Sender<",
        "Receiver<",
        "Timer",
        "interval(",
        "tokio::",
        "async_std::",
        "std::net",
        "std::fs",
        "std::process",
        "wry::",
        "tauri_runtime_wry",
        "WebView",
        "ICoreWebView",
        "WKWebView",
        "windows_sys::",
        "objc2::",
        "raw_window_handle",
        "evaluate_script",
        "Selector",
        "println!",
        "eprintln!",
        "tracing::",
        "log::",
        "Self::Event(event)=>formatter",
        "AgentNativeShutdownResources",
    ] {
        if driver.contains(forbidden) {
            return Err(format!(
                "agent native shutdown driver acquired forbidden authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_agent_app_lifecycle(
    manifest: &str,
    root: &str,
    api: &str,
    actor: &str,
    shell: &str,
    desktop_manifest: &str,
) -> Result<(), String> {
    fn compact_without_line_comments(source: &str) -> String {
        compact(
            &source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<String>(),
        )
    }

    let manifest = compact_without_line_comments(manifest);
    for required in [
        "agentic-browser=[\"dep:zephium-agentic\"]",
        "zephium-agentic={workspace=true,optional=true}",
    ] {
        if !manifest.contains(required) {
            return Err(format!(
                "application agent lifecycle lost dormant feature boundary {required}"
            ));
        }
    }
    let desktop_manifest = compact_without_line_comments(desktop_manifest);
    // Admits only the debug QA inspection lane (17dc25ff); that engine feature
    // refuses optimized builds and stays out of the ordinary graph.
    const QA_LANE: &str = "work-integration-qa=[\"work-development-traces\",\"tauri/custom-protocol\",\"zephium-engine/agentic-browser-qa\"]";
    let desktop_manifest = if desktop_manifest.matches("agentic-browser-qa").count() == 1 {
        desktop_manifest.replacen(QA_LANE, "", 1)
    } else {
        desktop_manifest
    };
    for forbidden in [
        "zephium-app/agentic-browser",
        "zephium-engine/agentic-browser",
    ] {
        if desktop_manifest.contains(forbidden) {
            return Err(format!(
                "ordinary desktop graph prematurely enabled dormant agent lifecycle {forbidden}"
            ));
        }
    }

    let root = compact_without_line_comments(root);
    for required in [
        "#[cfg(feature=\"agentic-browser\")]pubuseactor::{spawn_agentic,spawn_agentic_suspended,AgenticSpawnFailure};",
        "#[cfg(feature=\"agentic-browser\")]pubuseapi::AgentLifecycle;",
    ] {
        if !root.contains(required) {
            return Err(format!(
                "application root lost feature-gated agent lifecycle export {required}"
            ));
        }
    }

    let api = compact_without_line_comments(api);
    if !api.contains(
        "#[cfg(feature=\"agentic-browser\")]pubtypeAgentLifecycle=Box<dynAgentBrowserLifecycle>;",
    ) {
        return Err(
            "application agent lifecycle must remain a feature-gated move-only trait object"
                .to_owned(),
        );
    }

    let actor = compact_without_line_comments(actor);
    for required in [
        "traitPendingAgentLifecycle:Send+'static{typeFailure;",
        "structPendingAgentBrowserLifecycle(AgentLifecycle);",
        "typeFailure=AgenticSpawnFailure;",
        "structShellHandoff<Agent=NoAgentLifecycle>",
        "agent_lifecycle:Agent,",
        "pubstructAgenticSpawnFailure{error:SpawnError,agent_lifecycle:AgentLifecycle,worker_cleanup_proven:bool,}",
        "pubfninto_parts(self)->(SpawnError,AgentLifecycle)",
        "pubfnspawn_agentic_suspended(",
        "agent_lifecycle:PendingAgentBrowserLifecycle(agent_lifecycle),",
        "#[cfg_attr(not(test),deny(clippy::panic,clippy::unreachable,clippy::unwrap_used))]fnspawn_suspended_with_worker_spawner",
        "lethandoff=ShellHandoff{engine,store,blocker,agent_lifecycle,terminal_failure,chrome,emit,};",
        "ShellHandoff<Agent>",
        "ports.with_agent_lifecycle(agent_lifecycle.into_shell_lifecycle())",
    ] {
        if !actor.contains(required) {
            return Err(format!(
                "application actor lost lossless agent lifecycle ownership rule {required}"
            ));
        }
    }
    if actor.matches("agent_lifecycle.into_spawn_failure(").count() != 4 {
        return Err(
            "every app worker and handoff refusal must return the agent lifecycle owner".to_owned(),
        );
    }
    for forbidden in [
        "letmutagent_lifecycle=Some(agent_lifecycle)",
        "expect(\"pendingagentlifecycleownerisunique\")",
    ] {
        if actor.contains(forbidden) {
            return Err(format!(
                "application actor must carry unique lifecycle ownership structurally, not through {forbidden}"
            ));
        }
    }

    let shell = compact_without_line_comments(shell);
    for required in [
        "enumAgentLifecycleOwner{Absent,Owned(AgentLifecycle),Consumed,}",
        "agent_lifecycle:AgentLifecycleOwner,",
        "agent_lifecycle:AgentLifecycleOwner::new(agent_lifecycle),",
        "std::mem::replace(&mutself.agent_lifecycle,AgentLifecycleOwner::Consumed)",
        "AgentLifecycleOwner::Absent=>returntrue",
        "AgentLifecycleOwner::Owned(lifecycle)=>lifecycle",
        "AgentLifecycleOwner::Consumed=>",
        "lifecycle.shutdown_until(deadline)",
        "Ok(AgentBrowserShutdownOutcome::Clean(native_zero_proof))=>{drop(native_zero_proof);true}",
        "Ok(AgentBrowserShutdownOutcome::Unclean)=>",
        "&&agent_lifecycle_clean",
    ] {
        if !shell.contains(required) {
            return Err(format!(
                "application Shell lost proof-gated agent shutdown rule {required}"
            ));
        }
    }

    let shutdown_start = shell
        .find("fnshutdown_until(&mutself,")
        .ok_or_else(|| "application ordered shutdown function is missing".to_owned())?;
    let shutdown_end = shell[shutdown_start..]
        .find("fnshutdown_native_and_blocker_until(")
        .map(|offset| shutdown_start + offset)
        .ok_or_else(|| "application native shutdown boundary is missing".to_owned())?;
    let ordered = &shell[shutdown_start..shutdown_end];
    let flush = ordered
        .find("self.store.flush_until(deadline)")
        .ok_or_else(|| "application Store preflight is missing".to_owned())?;
    let agent = ordered
        .find("self.shutdown_agent_lifecycle_until(deadline)")
        .ok_or_else(|| "application agent lifecycle shutdown is missing".to_owned())?;
    let store = ordered
        .find("self.store.shutdown_until(deadline)")
        .ok_or_else(|| "application terminal Store shutdown is missing".to_owned())?;
    let engine = ordered
        .rfind("self.shutdown_native_and_blocker_until(deadline)")
        .ok_or_else(|| "application terminal engine shutdown is missing".to_owned())?;
    if !(flush < agent && agent < store && store < engine) {
        return Err(
            "agent lifecycle must follow retryable durability preflight and precede Store and engine teardown"
                .to_owned(),
        );
    }

    let unexpected_start = shell
        .find("pub(super)fncleanup_after_unexpected_exit_until(")
        .ok_or_else(|| "application unexpected-exit cleanup is missing".to_owned())?;
    let unexpected_end = shell[unexpected_start..]
        .find("pub(super)fnreport_terminal_failure(")
        .map(|offset| unexpected_start + offset)
        .ok_or_else(|| "application unexpected-exit cleanup boundary is malformed".to_owned())?;
    let unexpected = &shell[unexpected_start..unexpected_end];
    for required in [
        "self.shutdown_agent_lifecycle_until(deadline)",
        "self.store.shutdown_until(deadline)",
        "self.shutdown_native_and_blocker_until(deadline)",
    ] {
        if !unexpected.contains(required) {
            return Err(format!(
                "unexpected Shell exit can skip agent-owned cleanup rule {required}"
            ));
        }
    }
    Ok(())
}

fn validate_store_agent_audit_boundary(
    actor_root: &str,
    actor: &str,
    hub: &str,
) -> Result<(), String> {
    for (path, source) in [
        (STORE_AGENT_AUDIT_ACTOR, actor),
        (STORE_AGENT_AUDIT_HUB, hub),
    ] {
        if !compact(source).contains("#![deny(unsafe_code)]") {
            return Err(format!(
                "durable agent audit module {path} must deny unsafe code"
            ));
        }
        validate_agentic_no_direct_logging_attribute(path, source)?;
        validate_agentic_no_invariant_panic_attribute(path, source)?;
        validate_agentic_no_direct_logging_calls(path, source)?;
    }

    let actor_root = compact(actor_root);
    for required in [
        "modagent_audit;",
        "useagent_audit::AgentAuditDeliveryPermit;",
        "agent_audit_delivery_admission:OnceLock<Arc<AtomicUsize>>",
        "agent_audit_delivery_admission:OnceLock::new()",
        "AppendAgentAudit(AgentAuditDelivery,AgentAuditDeliveryPermit,AgentAuditCompletion,)",
        "agent_audit::append_and_settle(&muthub,delivery,completion).diagnostic()",
        "eprintln!(\"{message}\")",
    ] {
        if !actor_root.contains(required) {
            return Err(format!(
                "Store actor lost isolated durable agent audit integration {required}"
            ));
        }
    }
    for forbidden in [
        "implAgentAuditPortforSqliteStore",
        "fncomplete_agent_audit(",
        "eprintln!(\"store:agentaudit",
    ] {
        if actor_root.contains(forbidden) {
            return Err(format!(
                "general Store actor reacquired agent audit authority {forbidden}"
            ));
        }
    }

    let actor = compact(
        &actor
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>(),
    );
    for required in [
        "pub(super)constMAX_PENDING_AGENT_AUDIT_DELIVERIES:usize=8;",
        "admission:&OnceLock<Arc<AtomicUsize>>",
        "get_or_init(||Arc::new(AtomicUsize::new(0)))",
        "pending.checked_add(1).filter(|next|*next<=MAX_PENDING_AGENT_AUDIT_DELIVERIES)",
        "pending.checked_sub(1)",
        "self.admission.store(usize::MAX,Ordering::Release)",
        "implAgentAuditPortforSqliteStore",
        // ca429d28: shared non-blocking read admission; shutdown holds the write side.
        "self.lifecycle.try_read()",
        "Err(TryLockError::WouldBlock)=>",
        "lifecycle.terminal_admitted||self.shutdown_clean.load(Ordering::Acquire)",
        "AgentAuditDeliveryPermit::acquire(&self.agent_audit_delivery_admission)",
        ".try_send(Cmd::AppendAgentAudit(delivery,permit,completion))",
        "Err(error)=>refuse_queued(error,proof)",
        "mpsc::TrySendError::Full(_)=>AgentAuditSinkFailure::Capacity",
        "mpsc::TrySendError::Disconnected(_)=>AgentAuditSinkFailure::Shutdown",
        "ifletCmd::AppendAgentAudit(_delivery,_permit,completion)=command{discard_callback(completion);}",
        "AgentAuditDispatch::Refused(proof.settle(AgentAuditDeliveryOutcome::Refused(failure)))",
        "hub::AgentAuditAppendOutcome::Committed=>{proof.settle(AgentAuditDeliveryOutcome::Committed)}",
        "hub::AgentAuditAppendOutcome::Refused(failure)=>{proof.settle(AgentAuditDeliveryOutcome::Refused(failure))}",
        "hub::AgentAuditAppendOutcome::Uncertain=>{discard_callback(completion);returnAgentAuditActorResult::Uncertain;}",
        "std::panic::catch_unwind(std::panic::AssertUnwindSafe(||completion(settlement))).is_ok()",
        "std::panic::catch_unwind(std::panic::AssertUnwindSafe(||drop(completion)))",
        "Self::Uncertain=>Some(\"store:agentauditappendoutcomeisuncertain\")",
        "Self::CompletionPanicked=>Some(\"store:agentauditcompletioncallbackpanicked\")",
    ] {
        if !actor.contains(required) {
            return Err(format!(
                "isolated Store agent audit actor lost bounded authority rule {required}"
            ));
        }
    }
    for forbidden in [
        ".send(Cmd::AppendAgentAudit",
        "self.lifecycle.lock()",
        "Serialize",
        "Deserialize",
        "serde::",
        "std::thread",
        "tokio::",
        "std::fs",
        "std::net",
    ] {
        if actor.contains(forbidden) {
            return Err(format!(
                "isolated Store agent audit actor acquired forbidden surface {forbidden}"
            ));
        }
    }

    let uncertain = actor
        .find("hub::AgentAuditAppendOutcome::Uncertain=>")
        .ok_or_else(|| "durable audit uncertain settlement is missing".to_owned())?;
    let callback = actor
        .find("ifcomplete(completion,settlement)")
        .ok_or_else(|| "durable audit callback settlement is missing".to_owned())?;
    if uncertain >= callback {
        return Err(
            "an uncertain durable audit append must retain settlement by dropping its callback"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_agent_context_shutdown_barrier_contract(
    domain: &str,
    port: &str,
    host: &str,
) -> Result<(), String> {
    fn production(source: &str) -> &str {
        source
            .split_once("\n#[cfg(test)]\nmod tests")
            .map_or(source, |(production, _)| production)
    }
    let domain = compact(production(domain));
    for required in [
        "pubenumContextShutdownDispatch{",
        "AuditScheduled,",
        "SealedWithoutAudit(ContextPortFailure),",
        "pubstructContextShutdownAuditSettlement{audit:ContextResourceAuditId,outcome:Result<ContextNativeResourceSnapshot,ContextPortFailure>,}",
        "ShutdownAuditSettled(ContextShutdownAuditSettlement)",
        "fnseal_for_shutdown(&self,audit:ContextResourceAuditId)->ContextShutdownDispatch;",
    ] {
        if !domain.contains(required) {
            return Err(format!(
                "agent context shutdown vocabulary lost distinct barrier contract {required}"
            ));
        }
    }

    let port = compact(production(port));
    let ordinary_start = port
        .find("fnreserve_audit(")
        .ok_or_else(|| "post-seal resource-audit admission is missing".to_owned())?;
    let shutdown_start = port[ordinary_start..]
        .find("fnreserve_shutdown_audit(")
        .map(|offset| ordinary_start + offset)
        .ok_or_else(|| "atomic shutdown-audit admission is missing".to_owned())?;
    let ordinary = &port[ordinary_start..shutdown_start];
    for required in [
        "ifstate.invariant_failed||state.retired||self.lineage_failed(){returnErr(ContextPortFailure::Shutdown);}",
        "ifstate.pending>=MAX_PENDING_NATIVE_CONTEXT_TASKS{returnErr(ContextPortFailure::ResourceExhausted);}",
        "state.pending+=1;",
    ] {
        if !ordinary.contains(required) {
            return Err(format!(
                "post-seal resource audit lost bounded admission rule {required}"
            ));
        }
    }
    if ordinary.contains("ifstate.sealed") || ordinary.contains("state.sealed||") {
        return Err("read-only resource audits cannot be closed by the mutation seal".to_owned());
    }

    let shutdown_end = port[shutdown_start..]
        .find("fnreserve_screenshot(")
        .map(|offset| shutdown_start + offset)
        .ok_or_else(|| "shutdown-audit admission boundary is missing".to_owned())?;
    let shutdown = &port[shutdown_start..shutdown_end];
    let closed = shutdown
        .find("ifstate.sealed||state.invariant_failed")
        .ok_or_else(|| "shutdown audit no longer rejects a prior seal".to_owned())?;
    let seal = shutdown[closed..]
        .find("state.sealed=true;")
        .map(|offset| closed + offset)
        .ok_or_else(|| "shutdown audit no longer seals mutation admission".to_owned())?;
    let capacity = shutdown[closed..]
        .find("ifstate.pending>=MAX_PENDING_NATIVE_CONTEXT_TASKS")
        .map(|offset| closed + offset)
        .ok_or_else(|| "shutdown audit lost the global queue ceiling".to_owned())?;
    let reserve = shutdown[closed..]
        .find("state.pending+=1;")
        .map(|offset| closed + offset)
        .ok_or_else(|| "shutdown audit lost its exact pending reservation".to_owned())?;
    if !(closed < seal && seal < capacity && capacity < reserve) {
        return Err(
            "shutdown mutation seal must linearize before capacity refusal and reservation"
                .to_owned(),
        );
    }

    for required in [
        "Audit(ContextResourceAuditId),ShutdownAudit(ContextResourceAuditId),",
        "pub(crate)fncomplete_audit(self,outcome:Result<ContextNativeResourceSnapshot,ContextPortFailure>,)",
        "Some(AgentPendingRequest::ShutdownAudit(audit))=>{",
        "ContextNativeEvent::ShutdownAuditSettled(",
        "ContextShutdownAuditSettlement::new(audit,Err(failure))",
        "fnaudit_resources(&self,audit:ContextResourceAuditId)->ContextDispatch{self.schedule_audit(AgentPendingRequest::Audit(audit))}",
        "fnseal_for_shutdown(&self,audit:ContextResourceAuditId)->ContextShutdownDispatch",
        "self.admission.reserve_shutdown_audit()",
        "self.schedule_reserved(AgentPendingRequest::ShutdownAudit(audit),permit)",
        "ContextDispatch::Scheduled=>ContextShutdownDispatch::AuditScheduled",
        "ContextShutdownDispatch::SealedWithoutAudit(failure)",
    ] {
        if !port.contains(required) {
            return Err(format!(
                "engine shutdown barrier lost exact bounded routing {required}"
            ));
        }
    }

    let host = compact(production(host));
    if !host.contains("task.complete_audit(outcome);")
        || host.contains("ContextResourceAuditSettlement::new(audit,outcome)")
    {
        return Err(
            "native host must return audited counts through the task's distinct barrier route"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_agentic_zero_idle_sources(repository: &Path) -> Result<(), String> {
    let source_directory = repository.join(AGENTIC_SOURCE_DIRECTORY);
    // The provider-transport module now spans a directory (1783cf12, 77e1dbcf),
    // and public_asset.rs (376918e4) is its cookie-free fetch; both stay behind
    // the optional feature, so they sit outside the zero-idle core.
    let root = compact(&read(repository.join(AGENTIC_ROOT))?);
    if !root.contains("#[cfg(feature=\"provider-transport\")]pubmodpublic_asset;") {
        return Err("zephium-agentic public asset fetch escaped provider-transport".to_owned());
    }
    let transport = source_directory.join("provider_transport");
    // Policy tests live beside agent_policy.rs and compile only under cfg(test).
    let policy_tests = source_directory.join("agent_policy").join("tests");
    let policy = compact(&read(source_directory.join("agent_policy.rs"))?);
    if !policy.contains(
        "#[cfg(test)]modtests{#[cfg(feature=\"provider-transport\")]modimage_tests;modnavigation_tests;",
    ) {
        return Err("zephium-agentic policy test modules escaped cfg(test)".to_owned());
    }
    let mut files = Vec::new();
    collect_files(&source_directory, &mut files)?;
    for path in files {
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs")
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    AGENTIC_DIAGNOSTIC_MODULES.contains(&name) || name == "provider_transport.rs"
                })
        {
            continue;
        }
        let source = read(&path)?;
        let transport_owned = path.starts_with(&transport)
            || path.parent() == Some(source_directory.as_path())
                && path.file_name().and_then(|name| name.to_str()) == Some("public_asset.rs");
        let test_only = path.parent() == Some(policy_tests.as_path())
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| matches!(name, "image_tests.rs" | "navigation_tests.rs"));
        if !transport_owned && !test_only {
            validate_agentic_zero_idle_source(
                &path
                    .strip_prefix(repository)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                &source,
            )?;
        }
        validate_agentic_no_direct_logging_calls(
            &path
                .strip_prefix(repository)
                .unwrap_or(&path)
                .display()
                .to_string(),
            &source,
        )?;
    }
    Ok(())
}

fn validate_agentic_zero_idle_source(label: &str, source: &str) -> Result<(), String> {
    // Reading the current thread's identity grants no authority; 0eeb8c21's
    // test uses it to keep parallel tests from sharing a global probe hook.
    let source = source.replace("std::thread::current().id()", "");
    let source = source.as_str();
    for forbidden in [
        "std::net::",
        "std::thread::",
        "std::process::",
        "std::fs::",
        "tokio::",
        "async_std::",
        "reqwest::",
        "hyper::",
        "TcpListener",
        "UdpSocket",
        "Command::new(",
        "OpenOptions::",
        "File::open(",
        "thread::spawn(",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "zero-idle agentic functional core {label} acquired forbidden authority {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_shipping_sources(repository: &Path) -> Result<(), String> {
    let forbidden = [
        "probe-harness",
        "native-agentic-input-probe",
        "native-agentic-semantic-probe",
        "agentic_input_probe",
        "agentic_semantic_probe",
        "macos-agentic-input-probe",
        "macos-agentic-semantic-probe",
        "windows-agentic-input-probe",
        "windows-agentic-semantic-probe",
        "windows-cookie-persistence-control",
        "windows_cookie_persistence_control",
        "__zephiumNativeInputFixtureV1",
    ];
    let mut files = Vec::new();
    for root in SHIPPING_ROOTS {
        collect_files(&repository.join(root), &mut files)?;
    }
    for path in files {
        let source = read(&path)?;
        // This exact opt-in edge is independently release-refused and checked
        // by the Work composition boundary. No other shipping diagnostic edge
        // is exempted from this scan.
        let source = if path == repository.join(APP_MANIFEST) {
            source
                .replace("work-execution-probe = [\"work-execution\", \"zephium-agent-controller/probe-harness\"]", "")
                // 68d28d51: the synthesis diagnostic lane, refused in optimized builds.
                .replacen("# Explicit public synthesis diagnostics; agentic refuses probe-harness in optimized builds.\nwork-synthesis-probe = [\"work-runtime\", \"zephium-agentic/probe-harness\"]", "", 1)
        } else if path == repository.join(DESKTOP_MANIFEST) {
            // a119435e: the debug Work trace lane; probe-harness refuses optimized builds.
            source.replacen("work-development-traces = [\"work-product\", \"zephium-work-composition/public-qualification\", \"zephium-work-composition/retained-lifetime-diagnostic\", \"zephium-agentic/probe-harness\"]", "", 1)
        } else {
            source
        };
        if let Some(token) = forbidden.iter().find(|token| source.contains(**token)) {
            return Err(format!(
                "shipping source {} references agentic diagnostic token {token}",
                path.strip_prefix(repository).unwrap_or(&path).display()
            ));
        }
    }
    Ok(())
}

fn cargo_metadata(repository: &Path) -> Result<CargoMetadata, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--locked", "--format-version", "1"])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot execute cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err("cargo metadata failed while checking the release graph".to_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cannot decode cargo metadata: {error}"))
}

fn validate_release_graph(metadata: &CargoMetadata) -> Result<(), String> {
    let package_names = metadata
        .packages
        .iter()
        .map(|package| (package.name.as_str(), package.id.as_str()))
        .collect::<BTreeMap<_, _>>();
    let desktop = package_names
        .get("zephium-desktop")
        .ok_or_else(|| "cargo metadata is missing zephium-desktop".to_owned())?;
    let agentic = package_names
        .get("zephium-agentic")
        .ok_or_else(|| "cargo metadata is missing zephium-agentic".to_owned())?;
    let app = package_names
        .get("zephium-app")
        .ok_or_else(|| "cargo metadata is missing zephium-app".to_owned())?;
    let engine = package_names
        .get("zephium-engine")
        .ok_or_else(|| "cargo metadata is missing zephium-engine".to_owned())?;
    let provider_transport = package_names
        .get("zephium-agent-provider-transport")
        .ok_or_else(|| "cargo metadata is missing zephium-agent-provider-transport".to_owned())?;
    let controller = package_names
        .get("zephium-agent-controller")
        .ok_or_else(|| "cargo metadata is missing zephium-agent-controller".to_owned())?;
    let resolve = metadata
        .resolve
        .as_ref()
        .ok_or_else(|| "cargo metadata is missing its resolved graph".to_owned())?;
    let graph = resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    // a119435e ships Work by default: desktop's `work-product` links the
    // provider-transport runtime and the app/engine agent lifecycle. Only those
    // product features are admitted; every probe/harness feature stays refused.
    let work_product = graph.get(*desktop).is_some_and(|node| {
        node.features
            .iter()
            .any(|feature| feature == "work-product")
    });
    let mut pending = vec![*desktop];
    let mut visited = BTreeSet::new();
    while let Some(package) = pending.pop() {
        if !visited.insert(package) {
            continue;
        }
        if let Some(node) = graph.get(package).filter(|_| work_product) {
            let admitted: &[&str] = if package == *agentic {
                &["default", "provider-transport"]
            } else if package == *app {
                &[
                    "agentic-browser",
                    "macos-page-permission-prompts",
                    "work-execution",
                    "work-planning",
                    "work-runtime",
                ]
            } else if package == *provider_transport || package == *controller {
                &["default", "provider-transport"]
            } else {
                &[]
            };
            if (package == *agentic
                || package == *app
                || package == *provider_transport
                || package == *controller)
                && node
                    .features
                    .iter()
                    .any(|feature| !admitted.contains(&feature.as_str()))
            {
                return Err(
                    "ordinary zephium-desktop Work product graph activates an unreviewed agentic feature"
                        .to_owned(),
                );
            }
            if package == *engine
                && node.features.iter().any(|feature| {
                    matches!(
                        feature.as_str(),
                        "agentic-browser-qa"
                            | "native-agentic-input-probe"
                            | "native-agentic-semantic-probe"
                    )
                })
            {
                return Err(
                    "ordinary zephium-desktop Work product graph activates a native agentic probe"
                        .to_owned(),
                );
            }
            pending.extend(node.dependencies.iter().map(String::as_str));
            continue;
        }
        if package == *provider_transport {
            return Err(
                "ordinary zephium-desktop release graph links the dormant agent provider transport"
                    .to_owned(),
            );
        }
        if let Some(node) = graph.get(package) {
            if package == *app
                && node
                    .features
                    .iter()
                    .any(|feature| feature == "agentic-browser")
            {
                return Err(
                    "ordinary zephium-desktop release graph activates the dormant agent lifecycle"
                        .to_owned(),
                );
            }
            if package == *agentic
                && node.features.iter().any(|feature| {
                    matches!(feature.as_str(), "probe-harness" | "provider-transport")
                })
            {
                return Err(
                    "ordinary zephium-desktop release graph activates an opt-in agentic runtime"
                        .to_owned(),
                );
            }
            if package == *engine
                && node
                    .features
                    .iter()
                    .any(|feature| feature == "native-agentic-input-probe")
            {
                return Err(
                    "ordinary zephium-desktop release graph activates native agentic input probe"
                        .to_owned(),
                );
            }
            if package == *engine
                && node
                    .features
                    .iter()
                    .any(|feature| feature == "native-agentic-semantic-probe")
            {
                return Err(
                    "ordinary zephium-desktop release graph activates native agentic semantic probe"
                        .to_owned(),
                );
            }
            if package == *engine
                && node
                    .features
                    .iter()
                    .any(|feature| feature == "agentic-browser")
            {
                return Err(
                    "ordinary zephium-desktop release graph activates the unwired native agent-context adapter"
                        .to_owned(),
                );
            }
            pending.extend(node.dependencies.iter().map(String::as_str));
        }
    }
    Ok(())
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(directory)
        .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "agentic release-boundary roots may not contain symlinks: {}",
                entry.path().display()
            ));
        }
        if file_type.is_dir()
            && matches!(
                entry.file_name().to_str(),
                Some("target" | "node_modules" | ".git")
            )
        {
            continue;
        }
        if file_type.is_dir() {
            collect_files(&entry.path(), output)?;
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| matches!(extension.to_str(), Some("rs" | "toml" | "json")))
        {
            output.push(entry.path());
        }
    }
    Ok(())
}

fn read(path: impl AsRef<Path>) -> Result<String, String> {
    let path = path.as_ref();
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn compact(source: &str) -> String {
    source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn has_crate_private_into_parts(source: &str, return_type: &str) -> bool {
    let trailing_comma_return = return_type
        .strip_suffix(')')
        .map(|prefix| format!("{prefix},)"));
    let mut signatures = vec![
        format!("pub(crate)fninto_parts(self)->{return_type}"),
        format!("pub(crate)fninto_parts(self,)->{return_type}"),
    ];
    if let Some(return_type) = trailing_comma_return {
        signatures.push(format!("pub(crate)fninto_parts(self)->{return_type}"));
        signatures.push(format!("pub(crate)fninto_parts(self,)->{return_type}"));
    }
    signatures
        .iter()
        .any(|signature| source.contains(signature))
}

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    resolve: Option<CargoResolve>,
}

#[derive(Deserialize)]
struct CargoPackage {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct CargoResolve {
    nodes: Vec<CargoNode>,
}

#[derive(Deserialize)]
struct CargoNode {
    id: String,
    dependencies: Vec<String>,
    #[serde(default)]
    features: Vec<String>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn sequential_native_lifetimes_cannot_reopen_or_replace_unproven_owners() {
        let source = include_str!("../../crates/zephium-engine/src/agent_context_port.rs");
        super::validate_engine_agent_lifetime_boundary(source).unwrap();
        for boundary in [
            "active.retire_for_successor()",
            "!state.native_shutdown_verified",
            "self.permit.admission.invalidate_retirement_audit()",
            "state.retired = true;",
            "self.permit.admission.verify_native_shutdown(snapshot)",
            "state.issued >= MAX_AGENT_BROWSER_LIFETIMES",
        ] {
            assert!(
                super::validate_engine_agent_lifetime_boundary(
                    &source.replace(boundary, "removed_boundary")
                )
                .is_err(),
                "{boundary}"
            );
        }
        for forbidden in ["state.sealed = false;", "state.retired = false;"] {
            assert!(super::validate_engine_agent_lifetime_boundary(&format!(
                "{source}\n{forbidden}"
            ))
            .is_err());
        }
    }

    use super::*;

    #[test]
    fn production_agentic_modules_refuse_direct_diagnostic_output() {
        assert_eq!(
            ENGINE_AGENTIC_PRODUCTION_MODULES,
            [
                ENGINE_AGENT_CONTEXT_PORT,
                ENGINE_AGENT_CONTEXT_HOST,
                ENGINE_AGENT_COOKIE_SOURCE,
                ENGINE_AGENT_NAVIGATION,
                ENGINE_AGENT_SUSPENSION,
                ENGINE_AGENT_SCREENSHOT_BUFFER,
                ENGINE_AGENT_COOKIE_PREFLIGHT,
                ENGINE_WINDOWS_SEMANTIC_PROTOCOL,
                ENGINE_MACOS_AGENT_CONTEXT,
                ENGINE_MACOS_SEMANTIC_RUNTIME,
                ENGINE_MACOS_SEMANTIC_ACTION,
                ENGINE_MACOS_SEMANTIC_SCREENSHOT,
                ENGINE_WINDOWS_AGENT_CONTEXT,
                ENGINE_WINDOWS_COOKIE_TRANSFER,
                ENGINE_WINDOWS_SEMANTIC_RUNTIME,
                ENGINE_WINDOWS_SEMANTIC_SCREENSHOT,
                ENGINE_WINDOWS_AGENT_TIMEOUT,
            ]
        );
        let valid = r#"
            #![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
            #![cfg_attr(
                not(test),
                deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
            )]
            fn content_free_observation() {}
        "#;
        validate_agentic_no_direct_logging_attribute("fixture", valid)
            .expect("compile-time direct-output lint");
        validate_agentic_no_invariant_panic_attribute("fixture", valid)
            .expect("compile-time invariant-abort lint");
        validate_agentic_no_direct_logging_calls("fixture", valid)
            .expect("content-free observation");
        assert!(validate_agentic_no_direct_logging_attribute(
            "fixture",
            &valid.replace(
                "#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]",
                ""
            )
        )
        .is_err());
        assert!(validate_agentic_no_invariant_panic_attribute(
            "fixture",
            &valid.replace(
                "deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
                "allow(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
            ),
        )
        .is_err());
        for forbidden in [
            "println!(\"page\");",
            "tracing::info!(\"page\");",
            "unsafe { OutputDebugStringW(message); }",
        ] {
            assert!(validate_agentic_no_direct_logging_calls(
                "fixture",
                &format!("{valid}\n{forbidden}")
            )
            .is_err());
        }
    }

    #[test]
    fn provider_credentials_and_network_errors_remain_redacted() {
        let valid = r#"
            /// Credential owner.
            #[must_use]
            pub struct AgentProviderCredential<P: AgentCredentialBinding = AgentProviderKind> {
                provider: P,
                secret: Zeroizing<Vec<u8>>,
            }

            mod credential_binding {
                pub trait Sealed {}
                impl Sealed for crate::AgentProviderKind {}
                impl Sealed for super::DecisionCredentialProvider {}
            }

            pub trait AgentCredentialBinding: credential_binding::Sealed + Copy + fmt::Debug {}

            struct AgentProviderAttemptCredential {
                secret: Zeroizing<Vec<u8>>,
            }

            impl AgentProviderAttemptCredential {
                fn try_from_credential(credential: &AgentProviderCredential) {
                    let mut secret = Zeroizing::new(Vec::new());
                    secret.try_reserve_exact(credential.secret.len());
                    secret.extend_from_slice(&credential.secret);
                }

                fn sensitive_header(self, provider: AgentProviderKind) {
                    sensitive_header(provider, &self.secret);
                }
            }

            fn sensitive_header() {
                let mut encoded = Zeroizing::new(Vec::new());
                value.set_sensitive(true);
            }

            fn credential_debug() {
                formatter.field("secret", &"[redacted]");
                formatter.field("credential", &"[redacted]");
            }

            fn network_failure(error: &reqwest::Error) -> AgentProviderTransportOutcome {
                let class = if error.is_timeout() { Timeout } else { Transport };
                AgentProviderTransportOutcome::Failed(class)
            }
        "#;
        validate_provider_secret_diagnostic_contract(valid)
            .expect("zeroizing, redacted credential diagnostic boundary");
        for invalid in [
            valid.replacen("Zeroizing<Vec<u8>>", "Vec<u8>", 1),
            valid.replace(
                "impl Sealed for super::DecisionCredentialProvider {}",
                "impl Sealed for super::DecisionCredentialProvider {}\n impl Sealed for String {}",
            ),
            valid.replace("value.set_sensitive(true);", ""),
            valid.replace(
                "formatter.field(\"secret\", &\"[redacted]\");",
                "formatter.field(\"secret\", &self.secret);"
            ),
            valid.replace(
                "formatter.field(\"credential\", &\"[redacted]\");",
                "formatter.field(\"credential\", &self.credential);"
            ),
            valid.replace(
                "let class = if error.is_timeout() { Timeout } else { Transport };",
                "let detail = format!(\"{error:?}\"); let class = Transport;"
            ),
            valid.replace(
                "secret.extend_from_slice(&credential.secret);",
                "secret.extend_from_slice(&credential.secret); sensitive_header(provider, &secret);"
            ),
            valid.replace(
                "#[must_use]\n            pub struct AgentProviderCredential",
                "#[derive(Debug)]\n            #[must_use]\n            pub struct AgentProviderCredential"
            ),
        ] {
            assert!(validate_provider_secret_diagnostic_contract(&invalid).is_err());
        }
    }

    #[test]
    fn provider_revocation_precedes_authentication_materialization() {
        let valid = r#"
            async fn execute_model<F>() {
                if self.cancellation.is_cancelled() || self.shutdown.is_cancelled() {
                    return self.finish_terminal();
                }
                let mut decoder = match AgentProviderStreamDecoder::try_new() {};
                let credential = match self.credential.as_ref() {};
                let Some(request) = provider_request() else { return self.finish_terminal(); };
                if self.cancellation.is_cancelled() || self.shutdown.is_cancelled() {
                    return self.finish_terminal();
                }
            }
            impl fmt::Debug for AgentProviderAttempt {}
        "#;
        validate_provider_revocation_before_secret_materialization(valid)
            .expect("pre-materialization revocation boundary");

        let cancellation = r#"
                if self.cancellation.is_cancelled() || self.shutdown.is_cancelled() {
                    return self.finish_terminal();
                }
        "#;
        assert!(
            validate_provider_revocation_before_secret_materialization(&valid.replacen(
                cancellation,
                "",
                1
            ))
            .is_err()
        );
        assert!(
            validate_provider_revocation_before_secret_materialization(&valid.replace(
                "let Some(request) = provider_request() else { return self.finish_terminal(); };\n                if self.cancellation",
                "if self.cancellation"
            ))
            .is_err()
        );
        assert!(validate_provider_revocation_before_secret_materialization(
            &valid.replace(
                "let mut decoder = match AgentProviderStreamDecoder::try_new() {};\n                let credential = match self.credential.as_ref() {};",
                "let credential = match self.credential.as_ref() {};\n                let mut decoder = match AgentProviderStreamDecoder::try_new() {};"
            )
        )
        .is_err());
    }

    #[test]
    fn profile_release_requires_constructor_closed_supervisor_cleanup_proof() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let profile_lease = include_str!("../../crates/zephium-agentic/src/profile_lease.rs");
        let supervisor_context = include_str!(
            "../../crates/zephium-agentic/src/agent_supervisor/runtime/context_schedule.rs"
        );
        validate_profile_lease_release_contract(root, profile_lease, supervisor_context)
            .expect("profile release proof boundary");
        assert!(validate_profile_lease_release_contract(
            &root.replace("AgentSupervisorContextRelease,", ""),
            profile_lease,
            supervisor_context,
        )
        .is_err());
        for invalid in [
            profile_lease.replace(
                "proof: AgentSupervisorContextRelease",
                "proof: ContextProfileLease",
            ),
            profile_lease.replace("proof.assignment().identity() != lease.identity", "false"),
            profile_lease.replacen("ContextTerminal::Adopted", "ContextTerminal::Closed", 1),
        ] {
            assert!(
                validate_profile_lease_release_contract(root, &invalid, supervisor_context)
                    .is_err()
            );
        }
        for invalid in [
            supervisor_context.replace("let identity = registry.cancel_queued(context)?;", ""),
            supervisor_context.replace("#[cfg(test)]", ""),
            supervisor_context.replace(
                "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub struct AgentSupervisorContextRelease",
                "#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]\npub struct AgentSupervisorContextRelease"
            ),
            format!(
                "{supervisor_context}\nimpl AgentSupervisorContextRelease {{ pub fn new() {{}} }}"
            ),
        ] {
            assert!(
                validate_profile_lease_release_contract(root, profile_lease, &invalid).is_err()
            );
        }
    }

    #[test]
    fn shutdown_seal_retains_queued_context_cleanup_authority() {
        let registry = include_str!("../../crates/zephium-agentic/src/context_registry.rs");
        validate_context_shutdown_retention_contract(registry)
            .expect("retained queued shutdown authority");
        for invalid in [
            registry.replace("self.sealed = true;", ""),
            registry.replacen("Some(queued.identity)", "None", 1),
            registry.replace(
                "self.validate()?;\n        Ok(queued)",
                "let _ = self.rows.remove(&queued[0].id());\n        self.validate()?;\n        Ok(queued)"
            ),
            registry.replace(
                "if queued.capabilities.kind() != identity.kind()",
                "if self.sealed || queued.capabilities.kind() != identity.kind()"
            ),
            registry.replace(
                "pub fn cancel_queued(\n        &mut self,\n        context: ContextId,\n    ) -> Result<ContextIdentity, ContextRegistryError> {",
                "pub fn cancel_queued(\n        &mut self,\n        context: ContextId,\n    ) -> Result<ContextIdentity, ContextRegistryError> {\n        if self.sealed { return Err(ContextRegistryError::Sealed); }"
            ),
        ] {
            assert!(validate_context_shutdown_retention_contract(&invalid).is_err());
        }
    }

    #[test]
    fn native_shutdown_coordinator_requires_full_logical_drain_and_exact_zero_audits() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let lifecycle = include_str!("../../crates/zephium-agentic/src/agent_lifecycle.rs");
        let shutdown = include_str!("../../crates/zephium-agentic/src/agent_native_shutdown.rs");
        let screenshot = include_str!("../../crates/zephium-agentic/src/semantic_screenshot.rs");
        validate_agent_native_shutdown_coordinator(root, lifecycle, shutdown, screenshot)
            .expect("native shutdown coordinator boundary");

        for invalid in [
            root.replace("mod agent_native_shutdown;", ""),
            root.replace("AgentNativeShutdownProof,", ""),
        ] {
            assert!(validate_agent_native_shutdown_coordinator(
                &invalid, lifecycle, shutdown, screenshot,
            )
            .is_err());
        }
        for invalid in [
            lifecycle.replace(
                "Clean(AgentNativeShutdownProof)",
                "Clean",
            ),
            lifecycle.replace("self: Box<Self>", "&mut self"),
            lifecycle.replace(
                "#[must_use = \"agent browser shutdown must gate clean application teardown\"]\npub enum AgentBrowserShutdownOutcome",
                "#[derive(Clone, Copy)]\n#[must_use = \"agent browser shutdown must gate clean application teardown\"]\npub enum AgentBrowserShutdownOutcome",
            ),
        ] {
            assert!(
                validate_agent_native_shutdown_coordinator(root, &invalid, shutdown, screenshot)
                    .is_err()
            );
        }
        for invalid in [
            screenshot.replace(
                "if self.shutdown_sealed {\n            return Err(SemanticScreenshotCoordinatorError::Shutdown);\n        }",
                "",
            ),
            screenshot.replace("self.shutdown_sealed = true;", ""),
            screenshot.replace(
                "let Some(index) = self.pending.iter().position(|entry| entry.id == pending.id)",
                "if self.shutdown_sealed { return Err(SemanticScreenshotCoordinatorError::Shutdown); }\n        let Some(index) = self.pending.iter().position(|entry| entry.id == pending.id)",
            ),
        ] {
            assert!(
                validate_agent_native_shutdown_coordinator(root, lifecycle, shutdown, &invalid)
                    .is_err()
            );
        }
        for (index, invalid) in [
            shutdown.replace(
                "MAX_AGENT_NATIVE_SHUTDOWN_AUDITS: u8 = 8",
                "MAX_AGENT_NATIVE_SHUTDOWN_AUDITS: u8 = 64",
            ),
            shutdown.replace("if !self.contexts.is_quiescent()", "if false"),
            shutdown.replace("if !self.screenshots.is_quiescent()", "if false"),
            shutdown.replace("counts.pending_captures == 0", "true"),
            shutdown.replace(
                "settlement: ContextShutdownAuditSettlement",
                "settlement: ContextResourceAuditSettlement",
            ),
            shutdown.replace(
                "#[must_use]\npub struct AgentNativeShutdownProof",
                "#[derive(Clone, Copy)]\n#[must_use]\npub struct AgentNativeShutdownProof",
            ),
            shutdown.replacen(
                "\n#[cfg(test)]\nmod tests",
                "\nimpl AgentNativeShutdownProof { pub fn new() {} }\n\n#[cfg(test)]\nmod tests",
                1,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                validate_agent_native_shutdown_coordinator(root, lifecycle, &invalid, screenshot,)
                    .is_err(),
                "shutdown mutation {index} was not rejected"
            );
        }
    }

    #[test]
    fn native_shutdown_driver_consumes_the_port_with_bounded_redacted_waits() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let driver =
            include_str!("../../crates/zephium-agentic/src/agent_native_shutdown_driver.rs");
        validate_agent_native_shutdown_driver(root, driver)
            .expect("bounded native shutdown driver boundary");

        for invalid in [
            root.replace("mod agent_native_shutdown_driver;", ""),
            root.replace("AgentNativeShutdownEventSource,", ""),
        ] {
            assert!(validate_agent_native_shutdown_driver(&invalid, driver).is_err());
        }
        for (index, invalid) in [
            driver.replace("let dispatch = port.seal_for_shutdown(first_audit);", ""),
            driver.replace(
                "mut coordinator: AgentNativeShutdownCoordinator",
                "resources: AgentNativeShutdownResources",
            ),
            driver.replace(
                "ContextNativeEvent::ShutdownAuditSettled(settlement)",
                "ContextNativeEvent::ResourceAuditSettled(settlement)",
            ),
            driver.replace(".checked_add(1)", ".wrapping_add(1)"),
            driver.replace(
                "AgentNativeShutdownEventSource: Send",
                "AgentNativeShutdownEventSource",
            ),
            driver.replace(
                "AgentNativeShutdownWait::Event([redacted])",
                "AgentNativeShutdownWait::Event({event:?})",
            ),
            driver.replace("if Instant::now() >= deadline", "if false"),
            driver.replacen(
                "\n#[cfg(test)]\nmod tests",
                "\nfn forbidden_worker() { std::thread::spawn(|| {}); }\n\n#[cfg(test)]\nmod tests",
                1,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                validate_agent_native_shutdown_driver(root, &invalid).is_err(),
                "native shutdown driver mutation {index} was not rejected"
            );
        }
    }

    #[test]
    fn application_agent_lifecycle_is_lossless_proof_gated_and_release_dormant() {
        let manifest = include_str!("../../crates/zephium-app/Cargo.toml");
        let root = include_str!("../../crates/zephium-app/src/lib.rs");
        let api = include_str!("../../crates/zephium-app/src/api.rs");
        let actor = include_str!("../../crates/zephium-app/src/actor/mod.rs");
        let shell = include_str!("../../crates/zephium-app/src/shell/mod.rs");
        let desktop = include_str!("../../desktop/Cargo.toml");
        validate_agent_app_lifecycle(manifest, root, api, actor, shell, desktop)
            .expect("application agent lifecycle boundary");

        for (index, invalid) in [
            manifest.replace("agentic-browser = [\"dep:zephium-agentic\"]", ""),
            manifest.replace(
                "zephium-agentic = { workspace = true, optional = true }",
                "zephium-agentic.workspace = true",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                validate_agent_app_lifecycle(&invalid, root, api, actor, shell, desktop).is_err(),
                "application manifest mutation {index} was not rejected"
            );
        }

        let invalid_actor = actor.replacen(
            "agent_lifecycle.into_spawn_failure(",
            "SpawnFailure::new(",
            1,
        );
        assert!(
            validate_agent_app_lifecycle(manifest, root, api, &invalid_actor, shell, desktop)
                .is_err()
        );
        let invalid_actor = actor.replace(
            "pub fn into_parts(self) -> (SpawnError, AgentLifecycle)",
            "pub fn into_parts(self) -> SpawnError",
        );
        assert!(
            validate_agent_app_lifecycle(manifest, root, api, &invalid_actor, shell, desktop)
                .is_err()
        );
        let invalid_actor = actor.replace(
            "        blocker,\n        agent_lifecycle,\n        terminal_failure,",
            "        blocker,\n        agent_lifecycle: Some(agent_lifecycle)\n            .take()\n            .expect(\"pending agent lifecycle owner is unique\"),\n        terminal_failure,",
        );
        assert!(
            validate_agent_app_lifecycle(manifest, root, api, &invalid_actor, shell, desktop)
                .is_err()
        );

        let invalid_shell = shell.replace(
            "Ok(AgentBrowserShutdownOutcome::Clean(native_zero_proof))",
            "Ok(AgentBrowserShutdownOutcome::Unclean)",
        );
        assert!(
            validate_agent_app_lifecycle(manifest, root, api, actor, &invalid_shell, desktop)
                .is_err()
        );
        let invalid_shell = shell.replacen(
            "self.shutdown_agent_lifecycle_until(deadline)",
            "self.shutdown_agent_lifecycle_after_store(deadline)",
            1,
        );
        assert!(
            validate_agent_app_lifecycle(manifest, root, api, actor, &invalid_shell, desktop)
                .is_err()
        );

        let invalid_desktop = desktop.replace(
            "macos-page-permission-prompts = [",
            "agentic-browser = [\"zephium-app/agentic-browser\"]\nmacos-page-permission-prompts = [",
        );
        assert!(
            validate_agent_app_lifecycle(manifest, root, api, actor, shell, &invalid_desktop)
                .is_err()
        );
        for invalid_desktop in [
            desktop.replace(
                "work-product = [",
                "work-product = [\n    \"zephium-engine/agentic-browser-qa\",",
            ),
            desktop.replace(
                "\"zephium-engine/agentic-browser-qa\"]",
                "\"zephium-engine/agentic-browser\"]",
            ),
        ] {
            assert!(validate_agent_app_lifecycle(
                manifest,
                root,
                api,
                actor,
                shell,
                &invalid_desktop
            )
            .is_err());
        }
    }

    #[test]
    fn durable_agent_audit_actor_is_isolated_bounded_and_abort_free() {
        let actor_root = include_str!("../../crates/zephium-store/src/actor.rs");
        let actor = include_str!("../../crates/zephium-store/src/actor/agent_audit.rs");
        let hub = include_str!("../../crates/zephium-store/src/hub/agent_audit.rs");
        validate_store_agent_audit_boundary(actor_root, actor, hub)
            .expect("isolated durable agent audit actor");

        for (index, invalid) in [
            actor.replace(
                "MAX_PENDING_AGENT_AUDIT_DELIVERIES: usize = 8",
                "MAX_PENDING_AGENT_AUDIT_DELIVERIES: usize = 9",
            ),
            actor.replace("self.lifecycle.try_read()", "self.lifecycle.read()"),
            actor.replace(
                ".try_send(Cmd::AppendAgentAudit",
                ".send(Cmd::AppendAgentAudit",
            ),
            actor.replace(
                "pending.checked_sub(1)",
                "Some(pending.wrapping_sub(1))",
            ),
            actor.replace(
                "std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| completion(settlement))).is_ok()",
                "completion(settlement); true",
            ),
            actor.replacen(
                "            discard_callback(completion);\n            return AgentAuditActorResult::Uncertain;",
                "            return AgentAuditActorResult::Uncertain;",
                1,
            ),
            actor.replace(
                "deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
                "allow(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                validate_store_agent_audit_boundary(actor_root, &invalid, hub).is_err(),
                "agent audit actor mutation {index} was not rejected"
            );
        }

        assert!(validate_store_agent_audit_boundary(
            &actor_root.replace(
                "agent_audit::append_and_settle(&mut hub, delivery, completion)",
                "hub.append_agent_audit(&delivery)",
            ),
            actor,
            hub,
        )
        .is_err());
        assert!(validate_store_agent_audit_boundary(
            actor_root,
            actor,
            &hub.replace(
                "deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)",
                "allow(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)",
            ),
        )
        .is_err());
    }

    #[test]
    fn native_shutdown_barrier_is_atomic_distinct_and_drain_auditable() {
        let domain = include_str!("../../crates/zephium-agentic/src/context_port.rs");
        let port = include_str!("../../crates/zephium-engine/src/agent_context_port.rs");
        let host = include_str!("../../crates/zephium-engine/src/host/agent_context.rs");
        validate_agent_context_shutdown_barrier_contract(domain, port, host)
            .expect("atomic native shutdown barrier");

        for invalid in [
            domain.replace(
                "fn seal_for_shutdown(&self, audit: ContextResourceAuditId) -> ContextShutdownDispatch;",
                "",
            ),
            domain.replace(
                "ShutdownAuditSettled(ContextShutdownAuditSettlement)",
                "ResourceAuditSettled(ContextResourceAuditSettlement)",
            ),
        ] {
            assert!(
                validate_agent_context_shutdown_barrier_contract(&invalid, port, host).is_err()
            );
        }
        for invalid in [
            port.replace(
                "if state.invariant_failed || state.retired || self.lineage_failed() {",
                "if state.sealed || state.invariant_failed || state.retired || self.lineage_failed() {",
            ),
            port.replace(
                "state.sealed = true;\n        if state.pending >= MAX_PENDING_NATIVE_CONTEXT_TASKS",
                "if state.pending >= MAX_PENDING_NATIVE_CONTEXT_TASKS",
            ),
            port.replace(
                "self.schedule_audit(AgentPendingRequest::Audit(audit))",
                "self.schedule(AgentPendingRequest::Audit(audit))",
            ),
            port.replace(
                "self.schedule_reserved(AgentPendingRequest::ShutdownAudit(audit), permit)",
                "self.schedule_reserved(AgentPendingRequest::Audit(audit), permit)",
            ),
        ] {
            assert!(
                validate_agent_context_shutdown_barrier_contract(domain, &invalid, host).is_err()
            );
        }
        assert!(validate_agent_context_shutdown_barrier_contract(
            domain,
            port,
            &host.replace(
                "task.complete_audit(outcome);",
                "task.refuse(ContextPortFailure::Shutdown);"
            )
        )
        .is_err());
    }

    #[test]
    fn every_production_agentic_native_module_requires_strict_unsafe_lints() {
        assert_eq!(
            ENGINE_AGENTIC_NATIVE_UNSAFE_MODULES,
            [
                ENGINE_MACOS_AGENT_CONTEXT,
                ENGINE_MACOS_SEMANTIC_RUNTIME,
                ENGINE_MACOS_SEMANTIC_ACTION,
                ENGINE_MACOS_SEMANTIC_SCREENSHOT,
                ENGINE_WINDOWS_AGENT_CONTEXT,
                ENGINE_WINDOWS_COOKIE_TRANSFER,
                ENGINE_WINDOWS_SEMANTIC_RUNTIME,
                ENGINE_WINDOWS_SEMANTIC_SCREENSHOT,
                ENGINE_WINDOWS_AGENT_TIMEOUT,
            ]
        );
        let valid = format!("{ENGINE_AGENTIC_NATIVE_UNSAFE_HEADER}\n//! audited native module\n");
        for path in ENGINE_AGENTIC_NATIVE_UNSAFE_MODULES {
            validate_engine_agentic_native_unsafe_contract(path, &valid)
                .expect("strict native unsafe contract");
            for removed in [
                "#![deny(unsafe_op_in_unsafe_fn)]\n",
                "#![deny(clippy::undocumented_unsafe_blocks)]\n",
            ] {
                assert!(validate_engine_agentic_native_unsafe_contract(
                    path,
                    &valid.replacen(removed, "", 1),
                )
                .is_err());
            }
            assert!(validate_engine_agentic_native_unsafe_contract(
                path,
                &format!("//! misplaced\n{ENGINE_AGENTIC_NATIVE_UNSAFE_HEADER}"),
            )
            .is_err());
        }
    }

    #[test]
    fn windows_windowless_timer_guard_must_remain_non_send() {
        let valid = r#"
            struct ContentPolicyTimeout {
                timer: usize,
                generation: u64,
                _thread_bound: PhantomData<Rc<()>>,
            }
            fn guard(timer: usize, generation: u64) -> ContentPolicyTimeout {
                ContentPolicyTimeout { timer, generation, _thread_bound: PhantomData }
            }
        "#;
        validate_engine_windows_timeout_thread_binding(valid).expect("thread-bound timer guard");
        assert!(validate_engine_windows_timeout_thread_binding(
            &valid.replace("_thread_bound: PhantomData<Rc<()>>", "")
        )
        .is_err());
        assert!(
            validate_engine_windows_timeout_thread_binding(&valid.replace(
                "ContentPolicyTimeout { timer, generation, _thread_bound: PhantomData }",
                "ContentPolicyTimeout { timer, generation }"
            ))
            .is_err()
        );
    }

    #[test]
    fn macos_agent_view_hardening_and_attestation_require_main_thread_proof() {
        let marker =
            "let _mtm = MainThreadMarker::new().ok_or(AgentOwnedViewConstructionError::Native)?;";
        let valid = format!(
            "fn harden_owned_agent_view(view: &WebView) -> Result<(), AgentOwnedViewConstructionError> {{ {marker} Ok(()) }} fn attest() {{ {marker} }}"
        );
        validate_engine_macos_agent_main_thread_contract(&valid)
            .expect("main-thread refusing native view operations");
        assert!(
            validate_engine_macos_agent_main_thread_contract(&valid.replacen(marker, "", 1))
                .is_err()
        );
        assert!(validate_engine_macos_agent_main_thread_contract(
            &valid.replace("-> Result<(), AgentOwnedViewConstructionError>", "")
        )
        .is_err());
    }

    fn metadata(desktop_dependencies: Vec<String>) -> CargoMetadata {
        CargoMetadata {
            packages: vec![
                CargoPackage {
                    id: "desktop".to_owned(),
                    name: "zephium-desktop".to_owned(),
                },
                CargoPackage {
                    id: "engine".to_owned(),
                    name: "zephium-engine".to_owned(),
                },
                CargoPackage {
                    id: "app".to_owned(),
                    name: "zephium-app".to_owned(),
                },
                CargoPackage {
                    id: "agentic".to_owned(),
                    name: "zephium-agentic".to_owned(),
                },
                CargoPackage {
                    id: "transport".to_owned(),
                    name: "zephium-agent-provider-transport".to_owned(),
                },
                CargoPackage {
                    id: "controller".to_owned(),
                    name: "zephium-agent-controller".to_owned(),
                },
            ],
            resolve: Some(CargoResolve {
                nodes: vec![
                    CargoNode {
                        id: "desktop".to_owned(),
                        dependencies: desktop_dependencies,
                        features: Vec::new(),
                    },
                    CargoNode {
                        id: "transport".to_owned(),
                        dependencies: vec!["agentic".to_owned()],
                        features: Vec::new(),
                    },
                    CargoNode {
                        id: "engine".to_owned(),
                        dependencies: Vec::new(),
                        features: Vec::new(),
                    },
                    CargoNode {
                        id: "app".to_owned(),
                        dependencies: Vec::new(),
                        features: Vec::new(),
                    },
                    CargoNode {
                        id: "agentic".to_owned(),
                        dependencies: Vec::new(),
                        features: Vec::new(),
                    },
                    CargoNode {
                        id: "controller".to_owned(),
                        dependencies: vec!["transport".to_owned()],
                        features: Vec::new(),
                    },
                ],
            }),
        }
    }

    fn with_features(mut graph: CargoMetadata, id: &str, features: &[&str]) -> CargoMetadata {
        graph
            .resolve
            .as_mut()
            .expect("resolve")
            .nodes
            .iter_mut()
            .find(|node| node.id == id)
            .expect("node")
            .features
            .extend(features.iter().map(|feature| (*feature).to_owned()));
        graph
    }

    #[test]
    fn snapshot_action_kinds_remain_the_closed_native_set() {
        let provider = include_str!("../../crates/zephium-agentic/src/agent_provider.rs");
        validate_snapshot_action_kinds(provider).expect("closed snapshot action kinds");
        assert!(validate_snapshot_action_kinds(&provider.replace(
            "crate::SemanticActionKind::Scroll,\n];",
            "crate::SemanticActionKind::Scroll,\n    crate::SemanticActionKind::Navigate,\n];"
        ))
        .is_err());
    }

    #[test]
    fn snapshot_press_does_not_admit_a_sixth_or_substitute_action() {
        let provider = include_str!("../../crates/zephium-agentic/src/agent_provider.rs");
        validate_snapshot_action_kinds(provider).expect("five reviewed native actions");
        for invalid in [
            provider.replacen(
                "crate::SemanticActionKind::Press,",
                "crate::SemanticActionKind::Navigate,",
                1,
            ),
            provider.replacen("crate::SemanticActionKind::Press,", "", 1),
            provider
                .replacen(
                    "crate::SemanticActionKind::Press,",
                    "crate::SemanticActionKind::Press, crate::SemanticActionKind::Navigate,",
                    1,
                )
                .replacen(
                    "[crate::SemanticActionKind; 5]",
                    "[crate::SemanticActionKind; 6]",
                    1,
                ),
        ] {
            assert_ne!(invalid, provider);
            assert!(validate_snapshot_action_kinds(&invalid).is_err());
        }
    }

    #[test]
    fn work_product_graph_admits_only_its_reviewed_runtime_features() {
        let product = || {
            let graph = metadata(vec![
                "app".to_owned(),
                "engine".to_owned(),
                "controller".to_owned(),
                "agentic".to_owned(),
            ]);
            let graph = with_features(graph, "desktop", &["default", "work-product"]);
            let graph = with_features(graph, "agentic", &["default", "provider-transport"]);
            let graph = with_features(graph, "transport", &["default", "provider-transport"]);
            let graph = with_features(graph, "controller", &["default", "provider-transport"]);
            let graph = with_features(
                graph,
                "app",
                &[
                    "agentic-browser",
                    "work-execution",
                    "work-planning",
                    "work-runtime",
                ],
            );
            with_features(graph, "engine", &["agentic-browser"])
        };
        validate_release_graph(&product()).expect("reviewed Work product graph");
        for (id, feature) in [
            ("agentic", "probe-harness"),
            ("controller", "probe-harness"),
            ("transport", "probe-harness"),
            ("app", "work-execution-probe"),
            ("engine", "agentic-browser-qa"),
            ("engine", "native-agentic-semantic-probe"),
        ] {
            assert!(validate_release_graph(&with_features(product(), id, &[feature])).is_err());
        }
    }

    #[test]
    fn release_graph_accepts_production_graph_without_diagnostic_features() {
        validate_release_graph(&metadata(vec!["app".to_owned(), "engine".to_owned()]))
            .expect("isolated graph");
        validate_release_graph(&metadata(vec!["agentic".to_owned()]))
            .expect("production agentic graph");
    }

    #[test]
    fn release_graph_rejects_direct_or_transitive_diagnostic_features() {
        let mut direct = metadata(vec!["agentic".to_owned()]);
        direct
            .resolve
            .as_mut()
            .expect("resolve")
            .nodes
            .iter_mut()
            .find(|node| node.id == "agentic")
            .expect("agentic")
            .features
            .push("probe-harness".to_owned());
        assert!(validate_release_graph(&direct).is_err());
        let mut provider_runtime = metadata(vec!["agentic".to_owned()]);
        provider_runtime
            .resolve
            .as_mut()
            .expect("resolve")
            .nodes
            .iter_mut()
            .find(|node| node.id == "agentic")
            .expect("agentic")
            .features
            .push("provider-transport".to_owned());
        assert!(validate_release_graph(&provider_runtime).is_err());
        let mut transitive = metadata(vec!["engine".to_owned()]);
        transitive
            .resolve
            .as_mut()
            .expect("resolve")
            .nodes
            .iter_mut()
            .find(|node| node.id == "engine")
            .expect("engine")
            .features
            .push("native-agentic-input-probe".to_owned());
        assert!(validate_release_graph(&transitive).is_err());
        let mut app = metadata(vec!["app".to_owned()]);
        app.resolve
            .as_mut()
            .expect("resolve")
            .nodes
            .iter_mut()
            .find(|node| node.id == "app")
            .expect("app")
            .features
            .push("agentic-browser".to_owned());
        assert!(validate_release_graph(&app).is_err());
        assert!(validate_release_graph(&metadata(vec!["transport".to_owned()])).is_err());
    }

    #[test]
    fn exact_compile_refusal_is_required() {
        let valid = r#"
            #[cfg(all(feature = "probe-harness", not(debug_assertions)))]
            compile_error!("the agentic probe harness is forbidden in optimized builds");
            #[cfg(feature = "provider-transport")]
            mod provider_transport;
            #[cfg(all(feature = "provider-transport", feature = "probe-harness"))]
            pub use provider_transport::{exact_loopback_url, ProviderEndpoints};
            #[cfg(feature = "probe-harness")]
            mod contract;
            #[cfg(feature = "probe-harness")]
            mod control;
            #[cfg(feature = "probe-harness")]
            mod evidence;
            #[cfg(feature = "probe-harness")]
            mod fixture_server;
            #[cfg(feature = "probe-harness")]
            mod probe_evidence_path;
            #[cfg(feature = "probe-harness")]
            mod probe_recipes;
            #[cfg(feature = "probe-harness")]
            mod probe_qualification;
            #[cfg(feature = "probe-harness")]
            mod protocol;
            #[cfg(feature = "probe-harness")]
            mod semantic_probe_evidence;
        "#;
        validate_root(valid).expect("valid guard");
        assert!(validate_root("mod fixture_server;").is_err());
    }

    #[test]
    fn semantic_diff_policy_requires_exact_baseline_and_delivery_proof() {
        let root = "pub use semantic_diff_model::SemanticDiffDeliveryReceipt;";
        let diff = r#"
            struct SemanticDiff {
                baseline_guard: [u8; 32],
                current_fingerprint: SemanticObservationFingerprint,
                guard: [u8; 32],
            }
            let baseline_guard = acknowledgement.guard();
            let guard = semantic_diff_guard(baseline_guard, current_fingerprint.digest());
            fn semantic_diff_guard(
                baseline_guard: [u8; 32],
                current_guard: [u8; 32]
            ) -> [u8; 32] {
                hasher.update(b"ZEPHIUM-SEMANTIC-DIFF-1\0");
            }
        "#;
        let model = r#"
            pub struct SemanticDiffDeliveryReceipt { diff_guard: [u8; 32] }
            pub(crate) fn matches_diff(&self, diff: &SemanticDiff) -> bool {
                self.diff_guard == diff.guard()
                    && self.current_fingerprint == *diff.current_fingerprint()
            }
            pub fn settle_delivery_receipt() {
                diff_guard: self.diff_guard,
            }
        "#;
        let policy = r#"
            struct AgentTaintCohort { source_guard: [u8; 32] }
            ModelInputKind::Diff;
            pub fn prepare_diff_input() {
                if !payload.matches_diff(diff) {}
                cohort.source_guard == diff.baseline_guard();
                source_guard: diff.current_guard();
                source_guard: read.guard();
                hasher.update(candidate.source_guard);
            }
            fn retire_taint_reference() {}
            fn insert_taint_reference() {}
            pub fn commit_diff_input() {
                self.commit_model_input(admission, ModelInputKind::Diff, receipt.guard())
            }
        "#;
        validate_semantic_diff_policy_contract(root, diff, model, policy)
            .expect("exact semantic-diff policy");
        assert!(validate_semantic_diff_policy_contract(
            root,
            &diff.replace("baseline_guard: [u8; 32],", ""),
            model,
            policy,
        )
        .is_err());
        assert!(validate_semantic_diff_policy_contract(
            root,
            diff,
            model,
            &policy.replace("cohort.source_guard == diff.baseline_guard();", ""),
        )
        .is_err());
        assert!(validate_semantic_diff_policy_contract(root, diff, "", policy).is_err());
    }

    #[test]
    fn semantic_locate_requires_acknowledged_bounded_non_document_search() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let locate = include_str!("../../crates/zephium-agentic/src/semantic_locate.rs");
        let locate_model =
            include_str!("../../crates/zephium-agentic/src/semantic_locate_model.rs");
        let provider_root = include_str!("../../crates/zephium-agentic/src/agent_provider.rs");
        let provider_tool = include_str!("../../crates/zephium-agentic/src/agent_provider/tool.rs");
        let continuation =
            include_str!("../../crates/zephium-agentic/src/agent_provider/continuation.rs");
        let provider_request =
            include_str!("../../crates/zephium-agentic/src/agent_provider/request.rs");
        let policy = include_str!("../../crates/zephium-agentic/src/agent_policy.rs");
        validate_semantic_locate_contract(
            root,
            locate,
            locate_model,
            provider_root,
            provider_tool,
            continuation,
            provider_request,
            policy,
        )
        .expect("bounded semantic locate");
        // The advertised byte bound must govern fixed term storage. Restoring
        // an unrelated term ceiling or an unbounded cache is checker-visible.
        for changed in [
            locate.replace("MAX_SEMANTIC_LOCATE_QUERY_BYTES.div_ceil(2)", "usize::MAX"),
            locate.replace(
                "[u64; MAX_SEMANTIC_LOCATE_QUERY_TERMS.div_ceil(64)]",
                "Vec<u64>",
            ),
            locate.replace(
                "vec![QueryTermBits::default(); frame.nodes().len()]",
                "Vec::new()",
            ),
        ] {
            assert_ne!(changed, locate);
            assert!(validate_semantic_locate_contract(
                root,
                &changed,
                locate_model,
                provider_root,
                provider_tool,
                continuation,
                provider_request,
                policy
            )
            .is_err());
        }
        assert!(validate_semantic_locate_contract(
            root,
            &locate.replace("|| looks_like_secret_value(&source)", ""),
            locate_model,
            provider_root,
            provider_tool,
            continuation,
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_locate_contract(
            root,
            &format!("{locate}\nfn escape() {{ query_selector(\"*\"); }}"),
            locate_model,
            provider_root,
            provider_tool,
            continuation,
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_locate_contract(
            root,
            locate,
            locate_model,
            provider_root,
            &provider_tool.replace(
                "Self::SurroundingText { .. } | Self::TextSearch { .. } => {\n                Err(AgentBrowserToolContractError::Scope)\n            }",
                "Self::SurroundingText { target, .. } => Ok(SemanticLocateScope::Subtree(target))",
            ),
            continuation,
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_locate_contract(
            root,
            locate,
            locate_model,
            &provider_root.replace(
                "if locate.quality() != crate::SemanticTokenCountQuality::ExactLocal",
                "if false",
            ),
            provider_tool,
            continuation,
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_locate_contract(
            root,
            locate,
            &locate_model.replace("ZLOC{} content=untrusted", "ZLOC{} content=page"),
            provider_root,
            provider_tool,
            continuation,
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_locate_contract(
            root,
            locate,
            locate_model,
            provider_root,
            provider_tool,
            &continuation.replace(
                "if self.correlation.kind() != AgentBrowserToolKind::Locate",
                "if false",
            ),
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_locate_contract(
            root,
            locate,
            locate_model,
            provider_root,
            provider_tool,
            continuation,
            provider_request,
            &policy.replace("cohort.contains_reference(matched.reference())", "true",),
        )
        .is_err());
    }

    #[test]
    fn semantic_read_continuation_requires_exact_baseline_and_local_counting() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let read = include_str!("../../crates/zephium-agentic/src/semantic_read.rs");
        let read_model = include_str!("../../crates/zephium-agentic/src/semantic_read_model.rs");
        let provider_root = include_str!("../../crates/zephium-agentic/src/agent_provider.rs");
        let continuation =
            include_str!("../../crates/zephium-agentic/src/agent_provider/continuation.rs");
        let provider_request =
            include_str!("../../crates/zephium-agentic/src/agent_provider/request.rs");
        let policy = include_str!("../../crates/zephium-agentic/src/agent_policy.rs");
        validate_semantic_read_continuation_contract(
            root,
            read,
            read_model,
            provider_root,
            continuation,
            provider_request,
            policy,
        )
        .expect("bounded semantic read continuation");
        assert!(validate_semantic_read_continuation_contract(
            root,
            &read.replace(
                "&& acknowledgement.guard() == self.observation_fingerprint.digest()",
                "",
            ),
            read_model,
            provider_root,
            continuation,
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_read_continuation_contract(
            root,
            read,
            read_model,
            &provider_root.replace(
                "if read.quality() != crate::SemanticTokenCountQuality::ExactLocal",
                "if false",
            ),
            continuation,
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_read_continuation_contract(
            root,
            read,
            read_model,
            provider_root,
            &continuation.replace(
                "if !read.matches_acknowledgement(&self.baseline)",
                "if false",
            ),
            provider_request,
            policy,
        )
        .is_err());
        assert!(validate_semantic_read_continuation_contract(
            root,
            read,
            read_model,
            provider_root,
            continuation,
            &provider_request.replace(
                "continuation_baseline: Some(self.baseline)",
                "continuation_baseline: None",
            ),
            policy,
        )
        .is_err());
        assert!(validate_semantic_read_continuation_contract(
            root,
            read,
            read_model,
            provider_root,
            continuation,
            provider_request,
            &policy.replace("cohort.contains_reference(provenance.reference())", "true",),
        )
        .is_err());
    }

    #[test]
    fn semantic_extraction_requires_tool_free_schema_read_and_terminal_binding() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let extract = include_str!("../../crates/zephium-agentic/src/semantic_extract.rs");
        let extract_model =
            include_str!("../../crates/zephium-agentic/src/semantic_extract_model.rs");
        let provider_root = include_str!("../../crates/zephium-agentic/src/agent_provider.rs");
        let provider_tool = include_str!("../../crates/zephium-agentic/src/agent_provider/tool.rs");
        let continuation =
            include_str!("../../crates/zephium-agentic/src/agent_provider/continuation.rs");
        let provider_request =
            include_str!("../../crates/zephium-agentic/src/agent_provider/request.rs");
        let provider_extraction =
            include_str!("../../crates/zephium-agentic/src/agent_provider/extraction.rs");
        let policy = include_str!("../../crates/zephium-agentic/src/agent_policy.rs");
        validate_semantic_extraction_provider_contract(
            root,
            extract,
            extract_model,
            provider_root,
            provider_tool,
            continuation,
            provider_request,
            provider_extraction,
            policy,
        )
        .expect("bounded terminal semantic extraction");
        assert!(validate_semantic_extraction_provider_contract(
            root,
            extract,
            extract_model,
            provider_root,
            provider_tool,
            &continuation.replace(
                "|| self.correlation.extraction_schema != Some(schema.id())",
                "",
            ),
            provider_request,
            provider_extraction,
            policy,
        )
        .is_err());
        assert!(validate_semantic_extraction_provider_contract(
            root,
            extract,
            extract_model,
            provider_root,
            provider_tool,
            continuation,
            &provider_request.replace(
                "struct OpenAiExtractionRequestWire<'a> {",
                "struct OpenAiExtractionRequestWire<'a> { tools: Vec<()>,",
            ),
            provider_extraction,
            policy,
        )
        .is_err());
        assert!(validate_semantic_extraction_provider_contract(
            root,
            extract,
            extract_model,
            provider_root,
            provider_tool,
            continuation,
            provider_request,
            &provider_extraction.replace("self.output.clear();", ""),
            policy,
        )
        .is_err());
        assert!(validate_semantic_extraction_provider_contract(
            root,
            extract,
            extract_model,
            provider_root,
            provider_tool,
            continuation,
            provider_request,
            provider_extraction,
            &policy.replace("match input.subtree_target", "removed_boundary"),
        )
        .is_err());
    }

    #[test]
    fn semantic_execution_requires_one_shot_policy_and_native_rejoin() {
        let root = r#"
            mod semantic_execute;
            mod semantic_execute_coordinator;
            pub(crate) use semantic_execute::{
                prepare_semantic_action_execution,
                SemanticActionExecutionPending,
            };
            pub use semantic_execute::{
                begin_semantic_action_settlement,
                SemanticActionSettlementStart,
            };
            pub use semantic_execute_coordinator::SemanticActionExecutionCoordinator;
        "#;
        let effect = r#"
            pub fn matches_action(&self, action: &SemanticPreparedAction) -> bool {
                self.effect == action.effect()
                    && self.action_guard == action.verification_guard()
            }
        "#;
        let action = "target_geometry: Option<SemanticRect>";
        let execution = r#"
            const MAX_SEMANTIC_ACTION_NATIVE_EXECUTION_MILLIS: u32 = 5_000;
            active: AgentActiveEffect,
            active.matches_action(action);
            action.target_geometry().filter(|rect| true);
            action.verification_guard();
            NativeRecipe::Fill(value);
            ExactVisibleUnoccludedTarget;
            ExactConnectedScrollTarget;
            SemanticActionExecutionDisposition::ContractViolation(error);
            if current_frame != &self.correlation.frame {}
            if applied.completed_at > deadline {}
            SemanticActionFailure::BackendRefused;
            SemanticSettleInstant::from_millis(1);
            pub(crate) struct SemanticActionExecutionPending {}
            pub struct SemanticActionExecutionOutcome;
            impl SemanticActionExecutionOutcome {
                pub(crate) fn into_parts(
                    self,
                ) -> (AgentActiveEffect, SemanticActionExecutionDisposition) {}
            }
            pub struct SemanticActionSettlementStart {
                execution: SemanticActionExecutionApplied,
            }
            impl SemanticActionSettlementStart {
                pub(crate) const fn tracker_mut(&mut self) {}
                pub(crate) fn into_parts(self) {}
            }
            pub struct SemanticActionSettlementRefusal {
                active: Box<AgentActiveEffect>,
                error: SemanticActionSettlementStartError,
            }
            impl SemanticActionSettlementRefusal {
                pub const fn action_failure(&self) -> SemanticActionFailure {
                    self.error.action_failure()
                }
                pub(crate) fn into_parts(
                    self,
                ) -> (AgentActiveEffect, SemanticActionSettlementStartError) {}
            }
            impl SemanticActionSettlementStartError {
                pub const fn action_failure(self) -> SemanticActionFailure {
                    match self {
                        Self::ExecutionFailed(failure) => failure,
                        _ => SemanticActionFailure::BackendRefused,
                    }
                }
            }
            pub(crate) struct SemanticActionExecutionRefusal;
            impl SemanticActionExecutionRefusal {
                pub(crate) fn into_parts(
                    self,
                ) -> (AgentActiveEffect, SemanticActionExecutionPreparationError) {}
            }
            pub fn begin_semantic_action_settlement(
                outcome: SemanticActionExecutionOutcome,
                action: &SemanticPreparedAction,
            ) {
                let (active, disposition) = outcome.into_parts();
                if !active.matches_action(action) {}
                SemanticSettleTracker::begin(
                    active.attempt(),
                    action,
                    applied.settle_started_at(),
                );
                SemanticActionSettlementStartError::ExecutionFailed(failure);
                SemanticActionSettlementStartError::ExecutionContract(error);
                SemanticActionSettlementStartError::Settlement(error);
            }
            pub(crate) fn prepare_semantic_action_execution() {}
        "#;
        let coordinator = r#"
            const MAX_PENDING_SEMANTIC_ACTION_EXECUTIONS: usize = MAX_AGENT_PENDING_EFFECTS;
            pending: Vec<SemanticActionExecutionPending>,
            deadline: SemanticActionExecutionInstant,
            let deadline = pending.deadline();
            self.pending.len() >= MAX_PENDING_SEMANTIC_ACTION_EXECUTIONS;
            entry.coordinator_key().context() == action.frame().context().identity();
            !self.pending[index].matches_native_settlement(&settlement);
            now < self.pending[index].deadline();
            self.sealed = true;
            pub(crate) const fn semantic_action_dispatch_failure(
                dispatch: ContextDispatch,
            ) -> Option<SemanticActionFailure> {
            semantic_action_dispatch_failure(dispatch);
            ContextDispatch::Scheduled => None;
            ContextDispatch::Unsupported
                | ContextDispatch::Rejected(ContextPortFailure::Unsupported);
            ContextPortFailure::ResourceExhausted) => {
                Some(SemanticActionFailure::ResourceExhausted)
            }
            SemanticActionExecutionCoordinatorError::PrematureTimeout;
            }
            pub struct SemanticActionExecutionCoordinatorRefusal {
                active: Box<AgentActiveEffect>,
                error: SemanticActionExecutionCoordinatorError,
            }
            impl SemanticActionExecutionCoordinatorRefusal {
                pub const fn action_failure(&self) -> SemanticActionFailure {
                    match self.error {
                        SemanticActionExecutionCoordinatorError::Preparation(error) =>
                            error.action_failure(),
                        SemanticActionExecutionCoordinatorError::ContextBusy |
                        SemanticActionExecutionCoordinatorError::Capacity => {
                            SemanticActionFailure::ResourceExhausted
                        }
                        _ => SemanticActionFailure::BackendRefused,
                    }
                }
                pub(crate) fn into_parts(
                    self,
                ) -> (AgentActiveEffect, SemanticActionExecutionCoordinatorError) {}
            }
            SemanticEffectClass::LocalWrite;
            SemanticWaitCondition::TargetState {
                state: crate::SemanticState::Expanded,
                present: true,
            };
            AgentActiveEffect::for_execution_qualification(&action, attempt);
            pub fn settle_and_verify() {
                crate::begin_semantic_action_settlement(outcome, &self.action);
                crate::SemanticActionSettlementCoordinator::new();
                coordinator.observe_snapshot(reservation, observed_at, snapshot);
                crate::verify_semantic_action_terminal(*terminal, &self.action, evidence);
            }
        "#;
        let settle = r#"
            pub(crate) fn begin() {}
            pub const fn next_wake(&self) -> Option<SemanticSettleInstant> {
                if self.status.is_terminal() {}
                self.last_mutation_at.checked_add(quiet.millis());
                match candidate {
                    Some(candidate) if candidate.millis() < self.deadline.millis() => Some(candidate),
                    Some(_) | None => Some(self.deadline),
                }
            }
        "#;
        let context_port = r#"
            pub type SemanticActionNativeCompletion =
                Box<dyn FnOnce(SemanticActionNativeSettlement) + Send + 'static>;
            fn execute_semantic_action(
                &self,
                request: SemanticActionNativeRequest,
                completion: SemanticActionNativeCompletion,
            ) -> ContextDispatch;
        "#;
        let engine_port = r#"
            fn execute_semantic_action(
                &self,
                request: SemanticActionNativeRequest,
                completion: SemanticActionNativeCompletion,
            ) -> ContextDispatch {
                #[cfg(target_os = "macos")]
                {
                    if !matches!(
                        request.kind(),
                        zephium_agentic::SemanticActionKind::Click
                            | zephium_agentic::SemanticActionKind::Fill
                            | zephium_agentic::SemanticActionKind::Select
                    ) || request.frame().frame() != zephium_agentic::FrameId::MAIN
                        || request.frame().trust() != zephium_agentic::SemanticFrameTrust::SameOrigin
                        || request.frame().context().identity().kind()
                            != zephium_agentic::ContextKind::Owned
                    {
                        return ContextDispatch::Unsupported;
                    }
                    self.schedule_action(request, completion)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = (request, completion);
                    ContextDispatch::Unsupported
                }
            }
            fn capture_semantic_screenshot(&self) {}
        "#;
        let macos_action = r#"
            dispatch_guarded(_view, semantic, request, admitted_at, None, completion);
            pub(super) fn dispatch_guarded(
                authority: Option<Box<dyn Fn() -> bool>>,
            ) {
            if !zephium_agentic::AGENT_BROWSER_SNAPSHOT_ACTION_KINDS.contains(&request.kind()) {}
            encode_semantic_action_runtime_invocation(&request);
            semantic.dispatch_action_guarded(invocation, authority, move |outcome| {
                match outcome {
                    Ok(evidence) => complete_runtime_recipe(request, evidence, admitted_at),
                    Err(_) => unreachable!(),
                }
            });
            request.complete(
                evidence.backend(),
                evidence.readiness(),
                evidence.viewport(),
                evidence.geometry(),
                completed_at,
                completed_at,
            );
            }
        "#;
        validate_semantic_execution_contract(
            root,
            effect,
            action,
            execution,
            coordinator,
            context_port,
            (engine_port, macos_action),
        )
        .expect("closed semantic execution handoff");
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            &format!("{execution}\nquerySelector(target);"),
            coordinator,
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            &execution.replace(
                "pub(crate) const fn tracker_mut(&mut self) {}",
                "pub const fn tracker_mut(&mut self) {}",
            ),
            coordinator,
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            &root.replace("begin_semantic_action_settlement,", ""),
            effect,
            action,
            execution,
            coordinator,
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            &execution.replace("let (active, disposition) = outcome.into_parts();", ""),
            coordinator,
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            &execution.replace("if !active.matches_action(action) {}", ""),
            coordinator,
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            execution,
            coordinator,
            context_port,
            (
                &engine_port.replace(
                    "let _ = (request, completion);",
                    "if qualify() { return ContextDispatch::Scheduled; }\nlet _ = (request, completion);",
                ),
                macos_action,
            ),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            &execution.replace("if applied.completed_at > deadline {}", ""),
            coordinator,
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            execution,
            &coordinator.replace("self.sealed = true;", ""),
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            execution,
            &coordinator.replace(
                "SemanticEffectClass::LocalWrite",
                "SemanticEffectClass::Read",
            ),
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            &execution.replace(
                "pub(crate) fn prepare_semantic_action_execution() {}",
                "pub fn prepare_semantic_action_execution() {}",
            ),
            coordinator,
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            execution,
            &coordinator.replace("pub(crate) fn into_parts(", "pub fn into_parts(",),
            context_port,
            (engine_port, macos_action),
        )
        .is_err());
        validate_semantic_settle_wake(settle).expect("exact no-poll wake plan");
        assert!(validate_semantic_settle_wake(
            &settle.replace("self.last_mutation_at.checked_add(quiet.millis());", ""),
        )
        .is_err());
        assert!(validate_semantic_execution_contract(
            root,
            effect,
            action,
            execution,
            coordinator,
            context_port,
            (
                &engine_port.replace("ContextDispatch::Unsupported", "ContextDispatch::Scheduled"),
                macos_action,
            ),
        )
        .is_err());
    }

    #[test]
    fn semantic_settlement_coordination_requires_bounded_move_only_wakes() {
        let root = r#"
            mod semantic_settle_coordinator;
            pub use semantic_settle_coordinator::{
                SemanticActionSettlementCoordinator,
                SemanticActionSettlementReservation,
                SemanticActionSettlementTerminal,
                MAX_PENDING_SEMANTIC_ACTION_SETTLEMENTS,
            };
        "#;
        let coordinator = r#"
            const MAX_PENDING_SEMANTIC_ACTION_SETTLEMENTS: usize =
                MAX_AGENT_PENDING_EFFECTS;
            struct SettlementEntry {
                start: SemanticActionSettlementStart,
            }
            pub struct SemanticActionSettlementTerminal {
                start: SemanticActionSettlementStart,
            }
            enum Update {
                Terminal(Box<SemanticActionSettlementTerminal>),
            }
            fn terminal(start: SemanticActionSettlementStart) {
                SemanticActionSettlementTerminal::new(start);
            }
            impl SemanticActionSettlementTerminal {
                pub(crate) fn into_parts(
                    self,
                ) -> (
                    AgentActiveEffect,
                    SemanticActionExecutionApplied,
                    SemanticSettleTracker,
                ) {}
            }
            pub struct SemanticActionSettlementAdmissionRefusal {
                start: Box<SemanticActionSettlementStart>,
                error: SemanticActionSettlementCoordinatorError,
            }
            impl SemanticActionSettlementAdmissionRefusal {
                pub const fn action_failure(&self) -> SemanticActionFailure {
                    match self.error {
                        SemanticActionSettlementCoordinatorError::ContextBusy |
                        SemanticActionSettlementCoordinatorError::Capacity => {
                            SemanticActionFailure::ResourceExhausted
                        }
                        _ => SemanticActionFailure::BackendRefused,
                    }
                }
                pub(crate) fn into_parts(
                    self,
                ) -> (
                    SemanticActionSettlementStart,
                    SemanticActionSettlementCoordinatorError,
                ) {}
            }
            struct Reservation {
                next_wake: SemanticSettleInstant,
            }
            struct Coordinator {
                pending: Vec<SettlementEntry>,
            }
            fn begin(&mut self, key: Key, start: SemanticActionSettlementStart) {
                if start.tracker().status().is_terminal() {}
                entry.key.context() == key.context();
                self.pending.len() >= MAX_PENDING_SEMANTIC_ACTION_SETTLEMENTS;
                self.pending.push(SettlementEntry { key, start });
            }
            fn advance(&mut self, index: usize, reservation: Reservation, now: Instant) {
                if self.pending[index].start.tracker().next_wake()
                    != Some(reservation.next_wake) {
                        SemanticActionSettlementCoordinatorError::ScheduleMismatch;
                }
                SemanticSettleFact::Tick;
                if now < reservation.next_wake {}
                self.pending.remove(index).start;
            }
            pub fn observe_snapshot() {}
            fn seal(&mut self) {
                self.sealed = true;
            }
        "#;
        validate_semantic_settlement_coordinator(root, coordinator)
            .expect("bounded settlement owner");
        assert!(validate_semantic_settlement_coordinator(
            root,
            &coordinator.replace(
                "self.pending.len() >= MAX_PENDING_SEMANTIC_ACTION_SETTLEMENTS;",
                ""
            ),
        )
        .is_err());
        assert!(validate_semantic_settlement_coordinator(
            root,
            &coordinator.replace("pub(crate) fn into_parts(", "pub fn into_parts("),
        )
        .is_err());
        assert!(validate_semantic_settlement_coordinator(
            root,
            &coordinator.replace(
                "self.pending[index].start.tracker().next_wake()\n                    != Some(reservation.next_wake)",
                "false"
            ),
        )
        .is_err());
        assert!(validate_semantic_settlement_coordinator(
            root,
            &format!("{coordinator}\nstd::thread::spawn(run);"),
        )
        .is_err());
    }

    #[test]
    fn semantic_verification_consumes_only_coordinator_terminals() {
        let root = r#"
            pub use semantic_verify::{
                verify_semantic_action_terminal,
                SemanticActionVerificationRefusal,
                SemanticActionVerifiedTerminal,
                AgentFailedSemanticEffect,
                AgentVerifiedSemanticEffect,
                SemanticSnapshotEvidenceError,
                prepare_semantic_action_snapshot_evidence,
            };
            #[cfg(test)]
            pub(crate) use semantic_verify::verify_semantic_action;
        "#;
        let verification = r#"
            pub struct SemanticActionVerifiedTerminal {
                active: AgentActiveEffect,
                execution: SemanticActionExecutionApplied,
                settlement: SemanticSettleTracker,
                verified: SemanticVerifiedAction,
            }
            impl SemanticActionVerifiedTerminal {
                pub(crate) fn into_parts(self) {}
            }
            pub struct SemanticActionVerificationRefusal {
                terminal: Box<SemanticActionSettlementTerminal>,
                observed_at: SemanticSettleInstant,
            }
            impl SemanticActionVerificationRefusal {
                pub(crate) fn into_parts(self) {}
            }
            pub fn verify_semantic_action_terminal(
                terminal: SemanticActionSettlementTerminal,
                action: &SemanticPreparedAction,
                evidence: SemanticEffectEvidence<'_>,
            ) {
                let observed_at = evidence.observed_at();
                let verified = match verify_semantic_action(terminal.tracker(), action, evidence) {};
                let (active, execution, settlement) = terminal.into_parts();
            }
            pub(crate) fn verify_semantic_action(
                settlement: &SemanticSettleTracker,
            ) {}
            pub(crate) const fn exact_target_value() {}
            pub enum SemanticSnapshotEvidenceError {
                NonSnapshotEvidenceRequired,
            }
            pub fn prepare_semantic_action_snapshot_evidence() {}
        "#;
        let policy = r#"
            pub struct AgentVerifiedSemanticEffect {
                receipt: AgentEffectReceipt,
            }
            pub struct AgentFailedSemanticEffect {
                receipt: AgentEffectReceipt,
            }
            struct AgentFailedSemanticEffectEvidence {
                verification_observed_at: SemanticSettleInstant,
            }
            pub fn settle_verified_semantic_terminal(
                &mut self,
                terminal: SemanticActionVerifiedTerminal,
                action: &SemanticPreparedAction,
            ) {
                let (active, execution, settlement, verified) = terminal.into_parts();
                self.settle_verified_semantic_effect(active, action, &verified)?;
            }
            pub fn settle_refused_semantic_terminal(
                &mut self,
                refusal: SemanticActionVerificationRefusal,
            ) {
                let (active, execution, settlement, verification_observed_at, verification_error) =
                    refusal.into_parts();
                verification_error.action_failure();
            }
            pub(crate) fn settle_verified_semantic_effect() {}
        "#;
        validate_semantic_terminal_verification(root, verification, policy)
            .expect("consuming terminal verification");
        assert!(validate_semantic_terminal_verification(
            root,
            &verification.replace(
                "pub(crate) fn verify_semantic_action(",
                "pub fn verify_semantic_action(",
            ),
            policy,
        )
        .is_err());
        assert!(validate_semantic_terminal_verification(
            root,
            &verification.replace(
                "let (active, execution, settlement) = terminal.into_parts();",
                ""
            ),
            policy,
        )
        .is_err());
        assert!(validate_semantic_terminal_verification(
            root,
            &format!("{verification}\nfn into_terminal() {{}}"),
            policy,
        )
        .is_err());
        assert!(validate_semantic_terminal_verification(
            root,
            verification,
            &policy.replace("verification_error.action_failure();", ""),
        )
        .is_err());
    }

    #[test]
    fn action_result_consumes_accounted_proof_without_losing_current_state() {
        let root = r#"
            pub use semantic_action_result::{
                finalize_accounted_semantic_action_result,
                AgentAccountedSemanticActionResult,
                AgentAccountedSemanticActionResultRefusal,
            };
            pub use semantic_action_batch_result::{
                SemanticActionBatchAdmissionRefusal,
                SemanticActionBatchFailureAdmissionRefusal,
                SemanticActionBatchFailureStage,
                MAX_SEMANTIC_ACTION_BATCH_COMPLETION_BYTES,
                MAX_SEMANTIC_ACTION_BATCH_FAILURE_BYTES,
            };
        "#;
        let policy = r#"
            pub struct AgentEffectReceipt {
                action_guard: [u8; 32],
            }
            pub struct AgentVerifiedSemanticEffect {
                receipt: AgentEffectReceipt,
            }
            impl AgentVerifiedSemanticEffect {
                pub(crate) fn into_parts(self) {}
            }
            enum AgentFailedSemanticEffectEvidence {}
            pub struct AgentFailedSemanticEffect {
                evidence: Option<Box<AgentFailedSemanticEffectEvidence>>,
            }
            impl AgentFailedSemanticEffect {
                pub(crate) fn into_parts(self) {}
            }
            pub fn settle_execution_admission_refusal(
                refusal: SemanticActionExecutionCoordinatorRefusal,
                action: &SemanticPreparedAction,
            ) -> Result<AgentFailedSemanticEffect, AgentPolicyError> {
                let failure = refusal.action_failure();
            }
            pub fn settle_settlement_start_refusal(
                refusal: SemanticActionSettlementRefusal,
                action: &SemanticPreparedAction,
            ) -> Result<AgentFailedSemanticEffect, AgentPolicyError> {
                let failure = refusal.action_failure();
            }
            pub fn settle_settlement_admission_refusal(
                refusal: SemanticActionSettlementAdmissionRefusal,
                action: &SemanticPreparedAction,
            ) -> Result<AgentFailedSemanticEffect, AgentPolicyError> {
                let failure = refusal.action_failure();
            }
            pub(crate) fn settle_failed_semantic_effect(
                active: AgentActiveEffect,
                action: &SemanticPreparedAction,
            ) -> Result<AgentFailedSemanticEffect, AgentPolicyError> {
                if !active.matches_action(action) {}
            }
        "#;
        let result = r#"
            pub struct AgentAccountedSemanticActionResult {
                receipt: AgentEffectReceipt,
                result: SemanticActionResult,
            }
            impl AgentAccountedSemanticActionResult {
                pub(crate) fn into_parts(self) {}
            }
            pub struct AgentAccountedSemanticActionResultRefusal {
                accounted: Box<AgentVerifiedSemanticEffect>,
                current: Box<SemanticPostActionObservation>,
            }
            pub fn finalize_accounted_semantic_action_result(
                accounted: AgentVerifiedSemanticEffect,
            ) {
                if let Err(error) = validate_semantic_action_result(
                    action,
                    accounted.verified(),
                    baseline,
                    acknowledgement,
                    &current,
                ) {}
                let (receipt, execution, settlement, verified) = accounted.into_parts();
                let result = finish_semantic_action_result(
                    verified,
                    baseline,
                    acknowledgement,
                    current,
                    budget,
                );
            }
            fn validate_semantic_action_result(
                current: &SemanticPostActionObservation,
            ) {}
            #[cfg(test)]
            pub(crate) fn finalize_semantic_action_result() {}
        "#;
        let batch = r#"
            pub const MAX_SEMANTIC_ACTION_BATCH_COMPLETION_BYTES: usize = 512;
            pub const MAX_SEMANTIC_ACTION_BATCH_FAILURE_BYTES: usize = 512;
            pub struct SemanticActionBatchCompletion {
                receipt: AgentEffectReceipt,
                execution: SemanticActionExecutionApplied,
                settlement_event_count: u16,
                settlement_elapsed_millis: u64,
                settlement_terminal_at: SemanticSettleInstant,
            }
            const _: () = assert!(
                std::mem::size_of::<SemanticActionBatchCompletion>()
                    <= MAX_SEMANTIC_ACTION_BATCH_COMPLETION_BYTES
            );
            pub struct SemanticActionBatchFailure {
                verification_observed_at: Option<SemanticSettleInstant>,
            }
            impl SemanticActionBatchFailure {
                pub const fn verification_observed_at(self) -> Option<SemanticSettleInstant> {}
            }
            const _: () = assert!(
                std::mem::size_of::<SemanticActionBatchFailure>()
                    <= MAX_SEMANTIC_ACTION_BATCH_FAILURE_BYTES
            );
            pub struct SemanticActionBatchResult {
                failure: Option<SemanticActionBatchFailure>,
            }
            pub struct SemanticActionBatchAdmissionRefusal {
                accounted: Box<AgentAccountedSemanticActionResult>,
            }
            impl SemanticActionBatchAdmissionRefusal {
                pub fn into_parts(self) -> (
                    AgentAccountedSemanticActionResult,
                    SemanticActionBatchExecutionError,
                ) {}
            }
            pub struct SemanticActionBatchFailureAdmissionRefusal {
                execution: Box<SemanticActionBatchExecution>,
                failed: Box<AgentFailedSemanticEffect>,
            }
            pub fn record_success(
                &mut self,
                action: &SemanticPreparedAction,
                accounted: AgentAccountedSemanticActionResult,
            ) {
                if let Err(error) = self.validate_success(action, accounted.result()) {}
                receipt.settlement() != AgentEffectSettlement::Verified(verified_proof);
                settlement.status() != SemanticSettleStatus::ReadyForVerification;
                let (_, _, _, result) = accounted.into_parts();
                AccountingMismatch;
            }
            pub fn fail(
                self,
                action: &SemanticPreparedAction,
                failed: AgentFailedSemanticEffect,
            ) {
                if let Err(error) = self.validate_failure(action, &failed) {}
                let (receipt, failure, evidence) = failed.into_parts();
            }
        "#;
        validate_accounted_action_result(root, policy, result, batch)
            .expect("accounted action result");
        assert!(validate_accounted_action_result(
            root,
            policy,
            &result.replace("current: Box<SemanticPostActionObservation>,", ""),
            batch,
        )
        .is_err());
        assert!(validate_accounted_action_result(
            root,
            policy,
            &result.replace(
                "#[cfg(test)]\n            pub(crate) fn finalize_semantic_action_result() {}",
                "pub fn finalize_semantic_action_result() {}",
            ),
            batch,
        )
        .is_err());
        assert!(validate_accounted_action_result(
            root,
            policy,
            &result.replace(
                "if let Err(error) = validate_semantic_action_result(",
                "if false && ("
            ),
            batch,
        )
        .is_err());
        assert!(validate_accounted_action_result(
            root,
            policy,
            result,
            &batch.replace(
                "accounted: AgentAccountedSemanticActionResult,",
                "result: SemanticActionResult,"
            ),
        )
        .is_err());
        assert!(validate_accounted_action_result(
            root,
            policy,
            result,
            &batch.replace("accounted: Box<AgentAccountedSemanticActionResult>,", ""),
        )
        .is_err());
        assert!(validate_accounted_action_result(
            root,
            policy,
            result,
            &batch.replace("failed: Box<AgentFailedSemanticEffect>,", ""),
        )
        .is_err());
        assert!(validate_accounted_action_result(
            root,
            policy,
            result,
            &batch.replace(
                "failed: AgentFailedSemanticEffect,",
                "failure: SemanticActionFailure,"
            ),
        )
        .is_err());
        assert!(validate_accounted_action_result(
            root,
            &policy.replace(
                "pub(crate) fn settle_failed_semantic_effect(",
                "pub fn settle_failed_semantic_effect(",
            ),
            result,
            batch,
        )
        .is_err());
        assert!(validate_accounted_action_result(
            root,
            &policy.replace(
                "let failure = refusal.action_failure();",
                "let failure = SemanticActionFailure::BackendRefused;",
            ),
            result,
            batch,
        )
        .is_err());
    }

    #[test]
    fn provider_input_evidence_requires_authority_joined_stateless_diffs() {
        let root = r#"
            pub use request::{
                AgentCommittedProviderInput,
                AgentPreparedDiffRequest,
                AgentProviderInputEvidence,
                AgentProviderInputMetrics,
                AgentProviderInputTokenCount,
                AgentProviderSemanticInputStats,
                AgentProviderLocalInputTokenCounter,
            };
        "#;
        let request = r#"
            pub enum AgentProviderInputEvidence {
                Observation(SemanticObservationAcknowledgement),
                Diff(SemanticDiffDeliveryReceipt),
                Locate(SemanticLocateDeliveryReceipt),
                Read(SemanticReadDeliveryReceipt),
                Extraction(SemanticExtractionDeliveryReceipt),
                Screenshot(SemanticScreenshotDeliveryReceipt),
            }
            pub enum AgentProviderSemanticInputStats {
                Observation(SemanticEncodingStats),
                Diff(SemanticDiffEncodingStats),
                Locate(SemanticLocateEncodingStats),
                Read(SemanticReadEncodingStats),
                Extraction(SemanticExtractionEncodingStats),
                Screenshot(SemanticScreenshotStats),
            }
            pub struct AgentProviderInputTokenCount {
                tokens: u32,
                quality: SemanticTokenCountQuality,
            }
            pub struct AgentProviderInputMetrics {
                serialized_request_bytes: u32,
                semantic: AgentProviderSemanticInputStats,
                semantic_payload_tokens: Option<AgentProviderInputTokenCount>,
                structured_input_tokens: Option<AgentProviderInputTokenCount>,
            }
            assert!(MAX_AGENT_PROVIDER_REQUEST_BYTES <= u32::MAX as usize);
            assert!(std::mem::size_of::<AgentProviderInputMetrics>() <= 64);
            impl AgentProviderInputEvidence {}
            pub struct AgentCommittedProviderInput {
                metrics: AgentProviderInputMetrics,
            }
            evidence: AgentProviderInputEvidence::Observation(acknowledgement),
            evidence: AgentProviderInputEvidence::Diff(receipt),
            evidence: AgentProviderInputEvidence::Read(receipt),
            Committed(Box<AgentCommittedProviderInput>),
            pub const fn metrics(&self) -> AgentProviderInputMetrics {}
            pub const fn input_evidence(&self) -> &AgentProviderInputEvidence {}
            pub const fn input_metrics(&self) -> AgentProviderInputMetrics {}
            commitment.commit(policy, input_metrics);
            commitment.settle(policy, settlement, input_metrics);
            AgentProviderRequestSettlement::Committed => Ok(
                AgentProviderInputOutcome::Committed(Box::new(self.commit(policy, metrics)?))
            );
            AgentProviderSemanticInputStats::Observation(self.semantic_stats);
            AgentProviderSemanticInputStats::Diff(self.semantic_stats);
            AgentProviderSemanticInputStats::Locate(self.semantic_stats);
            AgentProviderSemanticInputStats::Read(self.semantic_stats);
            AgentProviderSemanticInputStats::Extraction(self.semantic_stats);
            AgentProviderSemanticInputStats::Screenshot(self.screenshot_stats);
            pub trait AgentProviderLocalInputTokenCounter {
                fn count_openai_responses_input();
                fn count_anthropic_messages_input();
            }
            pub struct AgentProviderDiffRequestDraft;
            counter: &dyn AgentProviderLocalInputTokenCounter;
            config.validate_diff_request();
            policy.prepare_provider_diff_input();
            pub struct AgentPreparedDiffRequest;
            commitment: AgentProviderInputCommitment::Diff;
            continuation_transcript: Some(self.continuation_transcript);
        "#;
        let provider = r#"
            fn validate_diff_request() {
                structured_input.quality() != crate::SemanticTokenCountQuality::ExactLocal;
                u64::from(structured_input.tokens()) > allowed_input_tokens;
                !self.pricing_profile().contains(u64::from(structured_input.tokens()));
            }
        "#;
        let policy = r#"
            fn prepare_provider_diff_input() {
                request.id() != expected.call;
                request.lease() != expected.lease;
                self.manifest.id() != expected.manifest;
                delivery.matches_diff(diff);
                measured: structured_input_tokens, additional: 0;
            }
        "#;
        let diff_model = r#"
            struct SemanticDiffDeliveryAuthority {
                measurement: SemanticTokenMeasurement,
            }
            fn matches_diff(&self, diff: &SemanticDiff) -> bool {}
            measurement: self.measurement;
        "#;
        let transport = r#"
            enum AgentProviderAttemptCore {
                PreDispatch { committed: AgentCommittedProviderRequest },
                Generation(AgentProviderTerminalCore),
            }
            struct AgentProviderTerminalCore {
                input: AgentCommittedProviderInput,
            }
            impl AgentProviderAttempt {
                pub fn input_evidence(&self) -> Option<&AgentProviderInputEvidence> {
                    match self.core.as_ref() {
                        Some(AgentProviderAttemptCore::PreDispatch { committed, .. }) => {
                            Some(committed.input_evidence())
                        }
                        Some(AgentProviderAttemptCore::Generation(terminal)) => Some(terminal.input.evidence()),
                        None => None,
                    }
                }
                pub fn input_metrics(&self) -> Option<AgentProviderInputMetrics> {
                    match self.core.as_ref() {
                        Some(AgentProviderAttemptCore::PreDispatch { committed, .. }) => {
                            Some(committed.input_metrics())
                        }
                        Some(AgentProviderAttemptCore::Generation(terminal)) => Some(terminal.input.metrics()),
                        None => None,
                    }
                }
            }
        "#;
        validate_provider_input_evidence_contract(
            root, request, provider, policy, diff_model, transport,
        )
        .expect("exact provider input evidence");
        assert!(validate_provider_input_evidence_contract(
            root,
            &request.replace("Read(SemanticReadDeliveryReceipt),", ""),
            provider,
            policy,
            diff_model,
            transport,
        )
        .is_err());
        assert!(validate_provider_input_evidence_contract(
            root,
            request,
            provider,
            &policy.replace("delivery.matches_diff(diff);", ""),
            diff_model,
            transport,
        )
        .is_err());
        assert!(validate_provider_input_evidence_contract(
            root,
            request,
            &provider.replace("SemanticTokenCountQuality::ExactLocal", "ProviderExact"),
            policy,
            diff_model,
            transport,
        )
        .is_err());
        assert!(validate_provider_input_evidence_contract(
            root, request, provider, policy, diff_model, "",
        )
        .is_err());
        assert!(validate_provider_input_evidence_contract(
            root,
            &request.replace(
                "semantic_payload_tokens: Option<AgentProviderInputTokenCount>,",
                "content: String,",
            ),
            provider,
            policy,
            diff_model,
            transport,
        )
        .is_err());
        assert!(validate_provider_input_evidence_contract(
            root,
            &request.replace("commitment.commit(policy, input_metrics);", ""),
            provider,
            policy,
            diff_model,
            transport,
        )
        .is_err());
    }

    #[test]
    fn supervisor_progress_requires_exact_manifest_revision_provenance() {
        let policy = r#"
            pub struct AgentActiveModelCall {
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            }
            pub struct AgentModelCallReceipt {
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            }
            pub(crate) fn matches_manifest_revision() {
                self.manifest == manifest && self.manifest_guard == manifest_guard
            }
            pub(crate) fn matches_manifest_revision() {
                self.manifest == manifest && self.manifest_guard == manifest_guard
            }
            Ok(AgentActiveModelCall {
                manifest: admission.manifest,
                manifest_guard: self.manifest.guard(),
            });
            Ok(AgentModelCallReceipt {
                manifest: self.manifest.id(),
                manifest_guard: self.manifest.guard(),
            });
            !active.matches_manifest_revision(self.manifest.id(), self.manifest.guard());
        "#;
        let effect_policy = r#"
            pub struct AgentNeedsHumanTransition {
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            }
            pub struct AgentEffectPermit {
                approval_binding: Option<crate::SemanticApprovalBinding>,
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            }
            pub struct AgentActiveEffect {
                approval_binding: Option<crate::SemanticApprovalBinding>,
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            }
            pub struct AgentEffectReceipt {
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            }
            pub(crate) fn matches_manifest_revision() {
                self.manifest == manifest && self.manifest_guard == manifest_guard
            }
            pub(crate) fn matches_manifest_revision() {
                self.manifest == manifest && self.manifest_guard == manifest_guard
            }
            pub(crate) fn matches_manifest_revision() {
                self.manifest == manifest && self.manifest_guard == manifest_guard
            }
            pub(crate) fn matches_manifest_revision() {
                self.manifest == manifest && self.manifest_guard == manifest_guard
            }
            AgentEffectAuthorization::Permit(AgentEffectPermit {
                approval_binding: assessment.approval_binding,
                manifest: self.manifest.id(),
                manifest_guard: self.manifest.guard(),
            });
            Ok(AgentActiveEffect {
                approval_binding: permit.approval_binding,
                manifest: permit.manifest,
                manifest_guard: permit.manifest_guard,
            });
            Ok(AgentEffectReceipt {
                manifest: self.manifest.id(),
                manifest_guard: active.manifest_guard,
            });
            AgentEffectAuthorization::NeedsHuman(AgentNeedsHumanTransition {
                manifest: self.manifest.id(),
                manifest_guard: self.manifest.guard(),
            });
            permit.matches_manifest_revision(self.manifest.id(), self.manifest.guard());
            permit.matches_manifest_revision(self.manifest.id(), self.manifest.guard());
            active.matches_manifest_revision(self.manifest.id(), self.manifest.guard());
        "#;
        let supervisor = r#"
            pub(super) const fn manifest_guard(&self) -> [u8; 32] {
                self.manifest_guard
            }
            pub(super) fn matches_manifest_revision(
                &self,
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            ) -> bool {
                self.manifest == manifest && self.manifest_guard == manifest_guard
            }
        "#;
        let join = r#"
            value.matches_manifest_revision(
                self.topology.manifest(),
                self.topology.manifest_guard()
            );
        "#;
        let progress = join.repeat(9);
        let audit = r#"
            if !supervisor
                .topology()
                .matches_manifest_revision(self.manifest, self.manifest_guard)
            {}
        "#;

        validate_progress_manifest_revision_contract(
            policy,
            effect_policy,
            supervisor,
            &progress,
            audit,
        )
        .expect("exact revision joins");
        assert!(validate_progress_manifest_revision_contract(
            &policy.replacen("manifest_guard: [u8; 32]", "", 1),
            effect_policy,
            supervisor,
            &progress,
            audit,
        )
        .is_err());
        assert!(validate_progress_manifest_revision_contract(
            policy,
            effect_policy,
            supervisor,
            &join.repeat(8),
            audit,
        )
        .is_err());
        assert!(validate_progress_manifest_revision_contract(
            policy,
            effect_policy,
            supervisor,
            &progress,
            "",
        )
        .is_err());
    }

    #[test]
    fn provider_call_identity_keeps_manifest_guard_private_through_progress_and_lineage() {
        let policy = r#"
            pub struct AgentModelCallAdmission {
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            }
            manifest_guard: self.manifest.guard(),
            pub(crate) const fn manifest_guard_for_provider(&self) -> [u8; 32] {
                self.manifest_guard
            }
        "#;
        let provider = r#"
            pub struct AgentProviderCallIdentity {
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            }
            pub(crate) fn from_admission(admission: &AgentModelCallAdmission) -> Self {}
            manifest_guard: admission.manifest_guard_for_provider(),
            pub(crate) fn from_active(active: &AgentActiveModelCall) -> Self {}
            manifest_guard: active.manifest_guard_for_metrics(),
            pub(crate) fn matches_manifest_revision(
                self,
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
            ) -> bool {
                self.manifest == manifest && self.manifest_guard == manifest_guard
            }
            pub(crate) const fn manifest_guard_for_continuation(self) -> [u8; 32] {
                self.manifest_guard
            }
            formatter.field("manifest_guard", &"[redacted]");
        "#;
        let lineage = r#"
            next_call.matches_manifest_revision(
                self.prior_call.manifest(),
                self.prior_call.manifest_guard_for_continuation(),
            );
        "#
        .repeat(5);
        let progress = r#"
            pub fn record_active_model_call(
                &mut self,
                execution: &AgentNodeExecution,
                call: crate::AgentProviderCallIdentity,
            ) -> Result<AgentSemanticProgress, AgentSupervisorRuntimeError> {
                call.matches_manifest_revision(
                    self.topology.manifest(),
                    self.topology.manifest_guard()
                );
                Some(AgentProgressResource::ModelCall(call.call()));
            }
        "#;
        validate_provider_call_identity_progress_boundary(policy, provider, &lineage, progress)
            .expect("guarded provider correlation boundary");
        assert!(validate_provider_call_identity_progress_boundary(
            policy,
            &provider.replace("pub(crate) fn from_active", "pub fn from_active"),
            &lineage,
            progress,
        )
        .is_err());
        assert!(validate_provider_call_identity_progress_boundary(
            policy,
            &provider.replace(
                "pub(crate) const fn manifest_guard_for_continuation",
                "pub const fn manifest_guard_for_continuation",
            ),
            &lineage,
            progress,
        )
        .is_err());
        assert!(validate_provider_call_identity_progress_boundary(
            policy,
            provider,
            &lineage.replacen(
                "next_call.matches_manifest_revision",
                "next_call.matches_public",
                1
            ),
            progress,
        )
        .is_err());
        assert!(validate_provider_call_identity_progress_boundary(
            policy,
            provider,
            &lineage,
            &progress.replace(
                "call.matches_manifest_revision",
                "call.matches_public_manifest"
            ),
        )
        .is_err());
    }

    #[test]
    fn run_accounting_metrics_remain_bounded_local_and_receipt_derived() {
        let root = r#"
            mod agent_metrics;
            pub use agent_metrics::{
                AgentRunAccountingMetrics,
                MAX_AGENT_METRIC_PRICING_SCHEDULES,
            };
        "#;
        let metrics = r#"
            pub const MAX_AGENT_METRIC_PRICING_SCHEDULES: usize = 8;
            pub struct AgentRunAccountingMetrics;
            pub fn try_new(
                manifest: &AgentRunManifest,
                supervisor: &AgentRunSupervisor,
            ) {}
            pub fn record_model_receipt(receipt: Receipt) {
                receipt.matches_manifest_revision(self.manifest, self.manifest_guard);
                values.binary_search(&receipt.id());
                self.validate_run_totals(next_operations, next_model)?;
                AgentMetricError::PricingScheduleLimit;
            }
            pub fn record_effect_receipt(receipt: Receipt) {
                receipt.matches_manifest_revision(self.manifest, self.manifest_guard);
                values.binary_search(&receipt.id());
                values.binary_search(&receipt.attempt());
                if next_operations > self.operation_limit {}
            }
        "#;
        validate_agent_metrics_contract(root, metrics).expect("bounded local metrics");
        assert!(validate_agent_metrics_contract(
            root,
            &metrics.replace(".binary_search(&receipt.id())", ".push(receipt.id())"),
        )
        .is_err());
        assert!(validate_agent_metrics_contract(
            root,
            &format!("{metrics}\ntrait AgentMetricPort {{}}"),
        )
        .is_err());
    }

    #[test]
    fn run_action_metrics_remain_fixed_bounded_and_batch_derived() {
        let root = r#"
            mod agent_action_metrics;
            pub use agent_action_metrics::{
                AgentRunActionPerformanceMetrics,
                AgentRunActionPerformanceSnapshot,
                AGENT_ACTION_DURATION_BUCKET_UPPER_BOUNDS_MILLIS,
                MAX_AGENT_ACTION_PERFORMANCE_SNAPSHOT_BYTES,
            };
        "#;
        let metrics = r#"
            pub const AGENT_ACTION_DURATION_BUCKET_UPPER_BOUNDS_MILLIS: [u64; 17] = [];
            pub const AGENT_ACTION_DURATION_BUCKET_COUNT: usize = 18;
            pub const MAX_AGENT_ACTION_PERFORMANCE_SNAPSHOT_BYTES: usize = 1_024;
            const _: () = assert!(
                std::mem::size_of::<AgentRunActionPerformanceSnapshot>()
                    <= MAX_AGENT_ACTION_PERFORMANCE_SNAPSHOT_BYTES
            );
            pub struct AgentRunActionPerformanceMetrics;
            pub fn try_new(
                manifest: &AgentRunManifest,
                supervisor: &AgentRunSupervisor,
            ) {
                if manifest.plan_nodes().len() > MAX_AGENT_PLAN_NODES {}
                values.try_reserve_exact(manifest.plan_nodes().len());
                operation_limit: manifest.budget().operations();
            }
            pub fn record_batch_result(
                &mut self,
                result: &SemanticActionBatchResult,
            ) {
                self.batches.binary_search(&result.batch());
                receipt.matches_manifest_revision(self.manifest, self.manifest_guard);
                existing_ids.binary_search(&id);
                existing_attempts.binary_search(&attempt);
                if next_actions > self.operation_limit {}
                if result.total() == 0
                    || usize::from(result.total()) > MAX_SEMANTIC_ACTIONS_PER_BATCH {}
                if admitted != usize::try_from(executed) {}
                let ids = [None; MAX_SEMANTIC_ACTIONS_PER_BATCH];
                self.batches.try_reserve(1);
                self.effect_receipts.try_reserve(admitted);
                self.effect_attempts.try_reserve(admitted);
            }
            pub const fn snapshot(&self) -> AgentRunActionPerformanceSnapshot {}
        "#;
        validate_agent_action_metrics_contract(root, metrics)
            .expect("fixed bounded action metrics");
        assert!(validate_agent_action_metrics_contract(
            root,
            &metrics.replace("existing_ids.binary_search(&id)", "existing_ids.push(id)"),
        )
        .is_err());
        assert!(validate_agent_action_metrics_contract(
            root,
            &format!("{metrics}\ntrait AgentActionMetricPort {{}}"),
        )
        .is_err());
    }

    #[test]
    fn run_provider_input_metrics_remain_commit_derived_and_bounded() {
        let root = r#"
            mod agent_input_metrics;
            pub use agent_input_metrics::{
                AgentRunProviderInputMetrics,
                AgentRunProviderInputSnapshot,
                MAX_AGENT_PROVIDER_INPUT_SNAPSHOT_BYTES,
            };
            pub use agent_provider::{
                AgentProviderInputMetricReceipt,
                MAX_AGENT_PROVIDER_INPUT_METRIC_RECEIPT_BYTES,
            };
        "#;
        let metrics = r#"
            pub const MAX_AGENT_PROVIDER_INPUT_SNAPSHOT_BYTES: usize = 1_024;
            pub struct AgentProviderInputKindMetrics {
                calls: u32,
                serialized_request_bytes: u64,
                disclosed_bytes: u64,
                semantic_lines: u64,
                semantic_payload_token_samples: u32,
                semantic_payload_tokens: u64,
                semantic_payload_qualities: [u32; 4],
                structured_input_token_samples: u32,
                structured_input_tokens: u64,
                structured_input_qualities: [u32; 4],
            }
            pub struct AgentProviderInputShapeMetrics {
                observation_secret_nodes: u64,
                diff_secret_nodes: u64,
                locate_withheld_secret_nodes: u64,
                read_sensitive_items: u64,
                extraction_sensitive_items: u64,
                screenshot_dropped_ancillary_bytes: u64,
                screenshot_layouts: [u32; 2],
            }
            pub struct AgentRunProviderInputSnapshot;
            const _: () = assert!(
                size_of::<AgentRunProviderInputSnapshot>()
                    <= MAX_AGENT_PROVIDER_INPUT_SNAPSHOT_BYTES
            );
            pub struct AgentRunProviderInputMetrics {
                kinds: [AgentProviderInputKindMetrics; 7],
                receipts: Vec<AgentModelCallId>,
            }
            pub fn try_new(
                manifest: &AgentRunManifest,
                supervisor: &AgentRunSupervisor,
            ) {
                if manifest.plan_nodes().len() > MAX_AGENT_PLAN_NODES {}
                values.try_reserve_exact(manifest.plan_nodes().len());
                operation_limit: manifest.budget().operations();
            }
            pub fn record(
                &mut self,
                receipt: AgentProviderInputMetricReceipt,
            ) {
                receipt.matches_manifest_revision(self.manifest, self.manifest_guard);
                self.receipts.binary_search(&receipt.call());
                values.binary_search_by_key(&receipt.node(), |row| row.metrics.node());
                if next_calls > self.operation_limit
                    || next_node_calls > self.nodes[node_index].operation_limit {}
                validate_input_metrics(metrics)?;
                self.receipts.try_reserve(1);
                self.receipts.insert(receipt_index, receipt.call());
            }
            pub const fn snapshot(&self) -> AgentRunProviderInputSnapshot {}
            AgentProviderSemanticInputStats::Observation(stats);
            AgentProviderSemanticInputStats::Diff(stats);
            AgentProviderSemanticInputStats::Locate(stats);
            AgentProviderSemanticInputStats::Read(stats);
            AgentProviderSemanticInputStats::Extraction(stats);
            AgentProviderSemanticInputStats::Screenshot(stats);
        "#;
        let request = r#"
            pub const MAX_AGENT_PROVIDER_INPUT_METRIC_RECEIPT_BYTES: usize = 192;
            assert!(
                size_of::<AgentProviderInputMetricReceipt>()
                    <= MAX_AGENT_PROVIDER_INPUT_METRIC_RECEIPT_BYTES
            );
            pub struct AgentProviderInputMetricReceipt {
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
                call: crate::AgentModelCallId,
                lease: crate::AgentPlanLeaseId,
                node: crate::AgentPlanNodeId,
                metrics: AgentProviderInputMetrics,
            }
            impl AgentProviderInputMetricReceipt {
                fn from_committed(input: &AgentCommittedProviderInput) -> Self {
                    manifest_guard: input.active.manifest_guard_for_metrics();
                }
                pub(crate) fn matches_manifest_revision(
                    self,
                    manifest: AgentRunManifestId,
                    manifest_guard: [u8; 32],
                ) -> bool {
                    self.manifest == manifest && self.manifest_guard == manifest_guard
                }
            }
            impl AgentProviderInputEvidence {}
            impl AgentCommittedProviderInput {
                pub fn metric_receipt(&self) -> Option<AgentProviderInputMetricReceipt> {
                    self.metrics.accounting_is_final();
                    Some(AgentProviderInputMetricReceipt::from_committed(self))
                }
            }
            impl AgentCommittedProviderRequest {
                pub fn input_metric_receipt(&self) -> Option<AgentProviderInputMetricReceipt> {
                    self.input.metric_receipt()
                }
                pub(crate) fn sealed_input_metric_receipt(&self) -> AgentProviderInputMetricReceipt {
                    self.input.sealed_metric_receipt()
                }
            }
        "#;
        let policy = r#"
            pub(crate) const fn manifest_guard_for_metrics(&self) -> [u8; 32] {
                self.manifest_guard
            }
        "#;
        let transport = r#"
            pub struct AgentProviderTransportResult {
                input_metric_receipt: AgentProviderInputMetricReceipt,
            }
            impl AgentProviderTransportResult {
                pub const fn input_metric_receipt(&self) -> AgentProviderInputMetricReceipt {
                    self.input_metric_receipt
                }
            }
            impl AgentProviderCountedAttempt {
                pub fn input_metric_receipt(&self) -> AgentProviderInputMetricReceipt {
                    self.input_metric_receipt
                }
            }
            enum AgentProviderAttemptCore {
                PreDispatch { committed: AgentCommittedProviderRequest },
                Generation(AgentProviderTerminalCore),
            }
            struct AgentProviderTerminalCore {
                input: AgentCommittedProviderInput,
            }
            impl AgentProviderAttempt {
                pub fn input_metric_receipt(&self) -> Option<AgentProviderInputMetricReceipt> {
                    match self.core.as_ref() {
                        Some(AgentProviderAttemptCore::PreDispatch { committed, .. }) => {
                            committed.input_metric_receipt()
                        }
                        Some(AgentProviderAttemptCore::Generation(terminal)) => {
                            terminal.input.sealed_metric_receipt()
                        }
                        None => None,
                    }
                }
            }
        "#;

        validate_agent_input_metrics_contract(root, metrics, request, policy, transport)
            .expect("commit-derived input metrics");
        assert!(validate_agent_input_metrics_contract(
            root,
            &metrics.replace(
                "receipt.matches_manifest_revision(self.manifest, self.manifest_guard);",
                "receipt.manifest() == self.manifest;",
            ),
            request,
            policy,
            transport,
        )
        .is_err());
        assert!(validate_agent_input_metrics_contract(
            root,
            &format!("{metrics}\ntrait AgentProviderInputMetricPort {{}}"),
            request,
            policy,
            transport,
        )
        .is_err());
        assert!(validate_agent_input_metrics_contract(
            root,
            metrics,
            &request.replace("manifest_guard: [u8; 32],", "content: String,"),
            policy,
            transport,
        )
        .is_err());
        assert!(validate_agent_input_metrics_contract(root, metrics, request, policy, "").is_err());
    }

    #[test]
    fn run_progress_metrics_remain_bounded_local_and_audit_derived() {
        let root = r#"
            mod agent_progress_metrics;
            pub use agent_progress_metrics::{
                AgentRunProgressMetrics,
                AgentRunProgressSnapshot,
                AgentProgressMetricError,
            };
        "#;
        let metrics = r#"
            use MAX_AGENT_PENDING_MODEL_CALLS;
            use MAX_AGENT_PENDING_EFFECTS;
            use MAX_AGENT_PLAN_NODES;
            pub struct AgentRunProgressMetrics;
            pub struct AgentRunProgressSnapshot;
            pub struct AgentDurationMetrics;
            pub struct AgentProgressMetricError;
            pub fn try_new(
                manifest: &AgentRunManifest,
                supervisor: &AgentRunSupervisor,
            ) {
                values.try_reserve_exact(topology_nodes.len());
            }
            pub fn record_event(
                &mut self,
                event: AgentAuditEvent
            ) -> Result<(), AgentProgressMetricError> {
                event.matches_manifest_revision(
                    self.manifest,
                    self.manifest_guard,
                    self.supervisor
                );
                self.active_models.try_reserve_exact(1);
                self.active_effects.try_reserve_exact(1);
                self.takeover_cancellations.try_reserve_exact(1);
                if self.active_models.iter().any(|active| active.node == node)
                    || self.active_effects.iter().any(|active| active.node == node)
                {}
                match model_action {
                    VectorAction::Insert(index, active) => self.active_models.insert(index, active),
                }
                match effect_action {
                    VectorAction::Insert(index, active) => self.active_effects.insert(index, active),
                }
            }
            pub const fn snapshot(&self) -> AgentRunProgressSnapshot {}
            pub const fn queue_wait(self) -> Option<AgentDurationMetrics> {}
            pub const fn model(self) -> Option<AgentDurationMetrics> {}
            pub const fn effect(self) -> Option<AgentDurationMetrics> {}
            pub const fn human_wait(self) -> Option<AgentDurationMetrics> {}
            pub const fn total_elapsed_millis(self) -> Option<u64> {}
        "#;
        let audit = r#"
            pub(crate) fn matches_manifest_revision(
                self,
                manifest: AgentRunManifestId,
                manifest_guard: [u8; 32],
                supervisor: AgentSupervisorId,
            ) -> bool {
                self.progress.manifest() == manifest
                    && self.progress.supervisor() == supervisor
                    && self.guard == event_guard(manifest_guard, self.record)
            }
        "#;

        validate_agent_progress_metrics_contract(root, metrics, audit)
            .expect("bounded local progress metrics");
        assert!(validate_agent_progress_metrics_contract(
            root,
            &metrics.replace("-> Option<AgentDurationMetrics>", "-> AgentDurationMetrics"),
            audit,
        )
        .is_err());
        assert!(validate_agent_progress_metrics_contract(
            root,
            &format!("{metrics}\ntrait AgentProgressMetricPort {{}}"),
            audit,
        )
        .is_err());
        assert!(validate_agent_progress_metrics_contract(root, metrics, "").is_err());
        assert!(validate_agent_progress_metrics_contract(
            root,
            &metrics.replace(
                "self.active_models.iter().any(|active| active.node == node)",
                "false",
            ),
            audit,
        )
        .is_err());
        assert!(validate_agent_progress_metrics_contract(
            root,
            &format!("{metrics}\nunreachable!();"),
            audit,
        )
        .is_err());
    }

    #[test]
    fn supervisor_cancellation_finalization_carries_its_proven_variant() {
        let runtime = include_str!("../../crates/zephium-agentic/src/agent_supervisor/runtime.rs");
        validate_agent_supervisor_cancellation_finalization(runtime)
            .expect("fail-closed cancellation finalization");

        for invalid in [
            runtime.replace(".enumerate().find_map(|(index, row)|", ".position(|row|"),
            runtime.replace(".then_some((index, cancelling))", ".then_some(index)"),
            format!("{runtime}\nunreachable!();"),
        ] {
            assert!(validate_agent_supervisor_cancellation_finalization(&invalid).is_err());
        }
    }

    #[test]
    fn default_agentic_core_carries_invariants_without_release_aborts() {
        let semantic = include_str!("../../crates/zephium-agentic/src/semantic.rs");
        let wire = include_str!("../../crates/zephium-agentic/src/semantic_wire.rs");
        let execution = include_str!("../../crates/zephium-agentic/src/semantic_execute.rs");
        let continuation =
            include_str!("../../crates/zephium-agentic/src/agent_provider/continuation.rs");
        let request = include_str!("../../crates/zephium-agentic/src/agent_provider/request.rs");

        validate_agentic_default_core_panic_boundary(
            semantic,
            wire,
            execution,
            continuation,
            request,
        )
        .expect("default core invariant carry");
        assert!(validate_agentic_default_core_panic_boundary(
            &semantic.replace("into_nonzero", "get"),
            wire,
            execution,
            continuation,
            request,
        )
        .is_err());
        assert!(validate_agentic_default_core_panic_boundary(
            semantic,
            &wire.replace(
                "SemanticOperations::try_new(operations).map_err(|_| SemanticDecodeError::NodeContract)",
                "SemanticOperations::try_new(operations)\n        .expect(\"static operations are duplicate-free\")",
            ),
            execution,
            continuation,
            request,
        )
        .is_err());
        assert!(validate_agentic_default_core_panic_boundary(
            semantic,
            wire,
            &execution.replace("Self(key.into_nonzero())", "unreachable!()"),
            continuation,
            request,
        )
        .is_err());
        assert!(validate_agentic_default_core_panic_boundary(
            semantic,
            wire,
            execution,
            &continuation.replace(".try_reserve(1)", ".reserve(1)"),
            request,
        )
        .is_err());
        assert!(validate_agentic_default_core_panic_boundary(
            semantic,
            wire,
            execution,
            &continuation.replacen(
                "\n#[cfg(test)]\nmod tests",
                "\nunreachable!();\n#[cfg(test)]\nmod tests",
                1,
            ),
            request,
        )
        .is_err());
        assert!(validate_agentic_default_core_panic_boundary(
            semantic,
            wire,
            execution,
            continuation,
            &request.replace("&AgentProviderBoundTranscript", "&AgentProviderTranscript"),
        )
        .is_err());
    }

    #[test]
    fn terminal_metric_closure_keeps_exact_private_coverage_joins() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let closure = include_str!("../../crates/zephium-agentic/src/agent_metric_closure.rs");
        let accounting = include_str!("../../crates/zephium-agentic/src/agent_metrics.rs");
        let progress = include_str!("../../crates/zephium-agentic/src/agent_progress_metrics.rs");
        let actions = include_str!("../../crates/zephium-agentic/src/agent_action_metrics.rs");
        let inputs = include_str!("../../crates/zephium-agentic/src/agent_input_metrics.rs");

        validate_agent_metric_closure_contract(
            root, closure, accounting, progress, actions, inputs,
        )
        .expect("exact terminal metric closure");
        assert!(validate_agent_metric_closure_contract(
            root,
            &closure.replace(
                "accounting.model_receipt_ids() != inputs.receipt_ids()",
                "model.calls() != input_snapshot.calls()",
            ),
            accounting,
            progress,
            actions,
            inputs,
        )
        .is_err());
        assert!(validate_agent_metric_closure_contract(
            root,
            &closure.replace("status.is_sealed()", "false"),
            accounting,
            progress,
            actions,
            inputs,
        )
        .is_err());
        assert!(validate_agent_metric_closure_contract(
            root,
            &format!("{closure}\ntrait AgentRunMetricClosurePort {{}}"),
            accounting,
            progress,
            actions,
            inputs,
        )
        .is_err());
    }

    #[test]
    fn clean_policy_settlement_stays_move_only_and_exactly_reconciled() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let closure = include_str!("../../crates/zephium-agentic/src/agent_metric_closure.rs");
        let accounting = include_str!("../../crates/zephium-agentic/src/agent_metrics.rs");
        let audit = include_str!("../../crates/zephium-agentic/src/agent_audit.rs");
        let policy = include_str!("../../crates/zephium-agentic/src/agent_policy.rs");

        validate_agent_policy_settlement_contract(root, closure, accounting, audit, policy)
            .expect("clean move-only policy settlement");
        assert!(validate_agent_policy_settlement_contract(
            root,
            closure,
            accounting,
            audit,
            &policy.replace("policy.pending_effects() != 0", "false"),
        )
        .is_err());
        assert!(validate_agent_policy_settlement_contract(
            root,
            closure,
            accounting,
            audit,
            &policy.replace("policy: self", "policy: AgentRunPolicy::placeholder()"),
        )
        .is_err());
        assert!(validate_agent_policy_settlement_contract(
            root,
            &format!(
                "{closure}\nimpl AgentRunMetricClosure {{ pub fn matches_manifest_revision() {{}} }}"
            ),
            accounting,
            audit,
            policy,
        )
        .is_err());
        assert!(validate_agent_policy_settlement_contract(
            root,
            closure,
            accounting,
            audit,
            &format!("{policy}\ntrait AgentRunPolicySettlementPort {{}}"),
        )
        .is_err());
        assert!(validate_agent_policy_settlement_contract(
            root,
            closure,
            accounting,
            audit,
            &policy.replace("audit_status.pending() != 0", "false"),
        )
        .is_err());
        assert!(validate_agent_policy_settlement_contract(
            root,
            closure,
            accounting,
            &audit.replace(
                "pub(crate) fn matches_run_scope",
                "pub fn matches_run_scope"
            ),
            policy,
        )
        .is_err());
    }

    #[test]
    fn default_agentic_core_retains_closed_dependency_and_authority_sets() {
        let manifest = r#"
            [package]
            publish = false
            [features]
            default = []
            provider-transport = ["dep:futures-util", "dep:reqwest", "dep:rig-core", "dep:tokio", "dep:zeroize", "dep:security-framework", "dep:security-framework-sys", "dep:dirs", "dep:zephium-decision", "dep:httpdate", "dep:zephium-credentials"]
            probe-harness = ["zephium-decision?/evals"]
            [[bin]]
            name = "windows-agentic-input-evidence-review"
            path = "src/bin/windows_agentic_input_evidence_review.rs"
            required-features = ["probe-harness"]
            [[bin]]
            name = "windows-agentic-semantic-evidence-review"
            path = "src/bin/windows_agentic_semantic_evidence_review.rs"
            required-features = ["probe-harness"]
            [dependencies]
            base64 = "0.22"
            crc32fast = "1"
            futures-util = { version = "1", optional = true }
            psl = "=2.1.220"
            reqwest = { version = "1", optional = true }
            rig-core = { version = "1", optional = true }
            serde = "1"
            serde_json = "1"
            sha2 = "1"
            thiserror = "2"
            tokio = { version = "1", optional = true }
            ulid = "1"
            url = "2"
            zeroize = { version = "1", optional = true }
            zephium-decision = { workspace = true, optional = true }
            httpdate = { version = "1", optional = true }
            zephium-core = "1"
            [target.'cfg(target_os = "windows")'.dependencies]
            zephium-credentials = { workspace = true, optional = true }
            [target.'cfg(target_os = "macos")'.dependencies]
            security-framework = { version = "1", optional = true }
            security-framework-sys = { version = "1", optional = true }
            dirs = { version = "1", optional = true }
        "#;
        validate_manifest(manifest).expect("closed functional-core manifest");
        for (optional, eager) in [
            (
                "zephium-decision = { workspace = true, optional = true }",
                "zephium-decision.workspace = true",
            ),
            (
                "rig-core = { version = \"1\", optional = true }",
                "rig-core = \"1\"",
            ),
            (
                "httpdate = { version = \"1\", optional = true }",
                "httpdate = \"1\"",
            ),
            (
                "dirs = { version = \"1\", optional = true }",
                "dirs = \"1\"",
            ),
            ("psl = \"=2.1.220\"", "psl = \"2\""),
        ] {
            assert!(validate_manifest(&manifest.replace(optional, eager)).is_err());
        }
        for harness in [
            "[\"zephium-decision/evals\"]",
            "[\"dep:zephium-decision\"]",
            "[]",
        ] {
            assert!(validate_manifest(&manifest.replace(
                "probe-harness = [\"zephium-decision?/evals\"]",
                &format!("probe-harness = {harness}")
            ))
            .is_err());
        }
        assert!(validate_manifest(&manifest.replace(
            "tokio = { version = \"1\", optional = true }",
            "tokio = \"1\""
        ))
        .is_err());
        assert!(
            validate_manifest(&manifest.replace("default = []", "default = [\"runtime\"]"))
                .is_err()
        );

        validate_agentic_zero_idle_source("core.rs", "use std::collections::BTreeMap;")
            .expect("allocation-only core");
        assert!(
            validate_agentic_zero_idle_source("core.rs", "use std::net::TcpListener;").is_err()
        );
        assert!(validate_agentic_zero_idle_source("core.rs", "tokio::spawn(work);").is_err());
    }

    #[test]
    fn provider_transport_requires_fixed_https_and_move_only_settlement() {
        let manifest = r#"
            [package]
            publish = false
            [features]
            provider-transport = ["dep:futures-util", "dep:reqwest", "dep:tokio", "dep:zeroize", "dep:security-framework", "dep:security-framework-sys"]
            [dependencies]
            futures-util = { version = "=0.3.32", optional = true }
            reqwest = { version = "=0.13.4", default-features = false, features = ["http2", "rustls", "stream", "system-proxy"], optional = true }
            tokio = { workspace = true, features = ["sync", "time", "net"], optional = true }
            zeroize = { version = "=1.9.0", optional = true }
            [target.'cfg(target_os = "macos")'.dependencies]
            security-framework = { version = "=3.7.0", optional = true }
            security-framework-sys = { version = "=2.17.0", optional = true }
        "#;
        validate_provider_transport_manifest(manifest).expect("valid transport manifest");
        assert!(
            validate_provider_transport_manifest(&manifest.replace("=0.13.4", "=0.13.5")).is_err()
        );
        assert!(
            validate_provider_transport_manifest(&manifest.replace(", \"system-proxy\"", ""))
                .is_err()
        );
        assert!(validate_provider_transport_manifest(&manifest.replace(", \"time\"", "")).is_err());

        let root = r#"
            const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
            const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
            pub struct ProviderEndpoints {}
            impl ProviderEndpoints {
                #[cfg(any(test, feature = "probe-harness"))]
                pub fn loopback(openai: &str, anthropic: &str) {
                    openai_input_tokens
                        .path_segments_mut()
                        .map_err(|_| AgentProviderTransportConfigError::Endpoint)?
                        .pop_if_empty()
                        .push("input_tokens");
                }
            }
            fn exact_production_url(url: &Url, host: &str, path: &str) -> bool {}
            #[cfg(any(test, feature = "probe-harness"))]
            pub fn exact_loopback_url(url: &str) -> bool {
                parse_exact_loopback_url(url).is_some()
            }
            #[cfg(any(test, feature = "probe-harness"))]
            fn parse_exact_loopback_url(source: &str) -> Option<Url> {
                let url = Url::parse(source).ok()?;
                (url.as_str() == source
                    && url.scheme() == "http"
                    && url.host_str() == Some("127.0.0.1")
                    && url.port().is_some_and(|port| port != 0)
                    && exact_loopback_path(url.path()))
                .then_some(url)
            }
            #[cfg(any(test, feature = "probe-harness"))]
            fn exact_loopback_path(path: &str) -> bool {
                path.split('/').all(|segment| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
                })
            }
            #[cfg(any(test, feature = "probe-harness"))]
            pub fn try_new_loopback(config: Config, openai: &str, anthropic: &str) {
                Self::try_new_with_endpoints(config, ProviderEndpoints::loopback(openai, anthropic)?)
            }
            builder
                .https_only(endpoints.https_only)
                .redirect(Policy::none())
                .referer(false)
                .retry(reqwest::retry::never())
                .pool_max_idle_per_host(0);
            #[cfg(any(test, feature = "probe-harness"))]
            let builder = if endpoints.https_only { builder } else { builder.no_proxy() };
            request.header(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
            value.set_sensitive(true);
            pub enum AgentProviderUsageKnowledge {
                ExactZeroBeforeModelDispatch,
                UnknownAfterDispatch,
            }
            pub fn into_policy_settlement(self) -> AgentProviderPolicySettlement {}
            if self.cancellation.is_cancelled() || self.shutdown.is_cancelled() {}
        "#;
        validate_provider_transport_root(root).expect("valid transport boundary");
        assert!(validate_provider_transport_root(&root.replace(
            "#[cfg(any(test, feature = \"probe-harness\"))]\n            pub fn try_new_loopback",
            "pub fn try_new_loopback"
        ))
        .is_err());
        assert!(validate_provider_transport_root(&format!(
            "{root}\nbuilder.danger_accept_invalid_certs(true);"
        ))
        .is_err());
        assert!(
            validate_provider_transport_root(&format!("{root}\npub fn into_parts() {{}}")).is_err()
        );
    }

    #[test]
    fn provider_transport_facade_is_opt_in_and_authority_free() {
        let manifest = include_str!("../../crates/zephium-agent-provider-transport/Cargo.toml");
        let root = include_str!("../../crates/zephium-agent-provider-transport/src/lib.rs");
        validate_provider_transport_facade(manifest, root).expect("safe provider transport facade");

        for invalid_manifest in [
            manifest.replacen("publish = false", "publish = true", 1),
            manifest.replacen("default = []", "default = [\"provider-transport\"]", 1),
            manifest.replacen(
                "provider-transport = [\"zephium-agentic/provider-transport\"]",
                "provider-transport = []",
                1,
            ),
            manifest.replacen(
                "provider-transport = [\"zephium-agentic/provider-transport\"]",
                "provider-transport = [\"zephium-agentic/provider-transport\", \"zephium-agentic/probe-harness\"]",
                1,
            ),
            manifest.replacen(
                "probe-harness = [\"provider-transport\", \"zephium-agentic/probe-harness\"]",
                "probe-harness = [\"zephium-agentic/probe-harness\"]",
                1,
            ),
            manifest.replacen(
                "zephium-agentic.workspace = true",
                "zephium-agentic = { workspace = true, features = [\"provider-transport\"] }",
                1,
            ),
        ] {
            assert!(validate_provider_transport_facade(&invalid_manifest, root).is_err());
        }

        for invalid_root in [
            root.replacen("AgentProviderTransport,", "AgentProviderTransportInput,", 1),
            root.replacen("AgentProviderAbortReason,", "AgentProviderAbortClass,", 1),
            root.replacen(
                "AgentProviderAttemptStateError,",
                "AgentProviderAttemptState,",
                1,
            ),
            root.replacen(
                "AgentProviderImmediateSettlementError,",
                "AgentProviderImmediateSettlementFailure,",
                1,
            ),
            format!("{root}\npub use reqwest::Client;"),
            format!("{root}\npub fn finish() {{}}"),
        ] {
            assert!(validate_provider_transport_facade(manifest, &invalid_root).is_err());
        }
    }

    #[test]
    fn provider_transport_disclosure_commit_is_linearized_with_cancellation() {
        let root = include_str!("../../crates/zephium-agentic/src/provider_transport.rs");
        validate_provider_transport_commit_boundary(root)
            .expect("provider transport disclosure commit boundary");

        for invalid in [
            root.replacen("commit_gate: Mutex<()>,", "", 1),
            root.replacen(
                "        let _commit_gate = self\n            .inner\n            .commit_gate\n            .lock()\n            .unwrap_or_else(std::sync::PoisonError::into_inner);",
                "",
                1,
            ),
            root.replacen(
                "let cancellation_gate = match cancellation.lock_commit_gate()",
                "let cancellation_gate = match unguarded_cancellation()",
                1,
            ),
            root.replacen(
                "match self.shared.shutdown.lock_commit_gate()",
                "match unguarded_shutdown()",
                1,
            ),
            root.replacen(
                "if cancellation.is_cancelled() || self.shared.shutdown.is_cancelled()",
                "if false",
                1,
            ),
            root.replacen(
                "        slot.mark_committed();\n        drop(shutdown_gate);\n        drop(cancellation_gate);",
                "        drop(shutdown_gate);\n        drop(cancellation_gate);\n        slot.mark_committed();",
                1,
            ),
            root.replacen(
                "                self.publish_cancellation();\n                drop(guard);\n                None",
                "                drop(guard);\n                None",
                1,
            ),
        ] {
            assert!(validate_provider_transport_commit_boundary(&invalid).is_err());
        }
    }

    #[test]
    fn provider_operation_retains_terminal_authority_across_dropped_drives() {
        let transport = include_str!("../../crates/zephium-agentic/src/provider_transport.rs");
        validate_abortable_provider_operation_contract(transport)
            .expect("abortable provider operation boundary");

        for (index, invalid) in [
            transport.replacen(
                "        self.phase = AgentProviderAttemptPhase::CountDriving;",
                "        self.phase = AgentProviderAttemptPhase::Ready;",
                1,
            ),
            transport.replacen(
                "        self.mark_input_count_disclosed();",
                "        // missing count disclosure",
                1,
            ),
            transport.replacen(
                "        self.mark_model_may_have_dispatched();",
                "        // missing model disclosure",
                1,
            ),
            transport
                .replacen("self.mark_count_drive_started()", "__COUNT_PHASE__", 1)
                .replacen(
                    "self.mark_input_count_disclosed()",
                    "self.mark_count_drive_started()",
                    1,
                )
                .replacen("__COUNT_PHASE__", "self.mark_input_count_disclosed()", 1),
            transport
                .replacen("self.begin_generation_drive()?", "__GENERATION_BEGIN__", 1)
                .replacen(
                    "self.mark_model_may_have_dispatched()",
                    "self.begin_generation_drive()?",
                    1,
                )
                .replacen(
                    "__GENERATION_BEGIN__",
                    "self.mark_model_may_have_dispatched()",
                    1,
                ),
            transport.replacen(
                "    pub fn abort(\n        mut self,",
                "    pub fn abort(\n        &mut self,",
                1,
            ),
            transport.replacen(
                "        let outcome = match reason {",
                "        self.prior_disclosure_stage = AgentProviderDisclosureStage::NotDispatched;\n        let outcome = match reason {",
                1,
            ),
            transport.replacen(
                "            self.fail_stop();\n            self.phase = AgentProviderAttemptPhase::Terminal;",
                "            // missing fail stop\n            self.phase = AgentProviderAttemptPhase::Terminal;",
                1,
            ),
            transport.replacen(
                "\n#[cfg(test)]\nmod tests",
                "\n#[cfg(any())]\nfn dormant_provider_drive() {}\n\n#[cfg(test)]\nmod tests",
                1,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                validate_abortable_provider_operation_contract(&invalid).is_err(),
                "abortable operation mutation {index} unexpectedly passed"
            );
        }
    }

    #[test]
    fn immediate_provider_settlement_retains_authority_before_policy_use() {
        let transport = include_str!("../../crates/zephium-agentic/src/provider_transport.rs");
        validate_immediate_provider_settlement_recovery_contract(transport)
            .expect("immediate settlement recovery boundary");

        for (index, invalid) in [
            transport.replacen(
                "AgentProviderImmediateSettlementError::PolicyPrecondition",
                "AgentProviderImmediateSettlementError::Policy",
                1,
            ),
            transport.replacen(
                "unsettled: Box::new(self),",
                "unsettled: Box::new(terminal),",
                1,
            ),
            transport.replacen(
                "#[error(\"agent provider immediate policy settlement failed\")]\n    Policy(#[source] AgentPolicyError)",
                "#[error(\"agent provider immediate policy settlement failed\")]\n    Policy(AgentPolicyError)",
                1,
            ),
            transport.replacen(
                "pub const fn policy_precondition_error",
                "pub const fn policy_error",
                1,
            ),
            transport.replacen(
                "pub fn into_unsettled(self) -> Option<AgentProviderImmediateSettlement>",
                "pub(crate) fn into_unsettled(self) -> Option<AgentProviderImmediateSettlement>",
                1,
            ),
            transport.replacen(
                "#[derive(Debug, Error)]\npub enum AgentProviderImmediateSettlementError",
                "#[derive(Clone, Debug, Error)]\npub enum AgentProviderImmediateSettlementError",
                1,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                validate_immediate_provider_settlement_recovery_contract(&invalid).is_err(),
                "immediate settlement mutation {index} unexpectedly passed"
            );
        }
    }

    #[test]
    fn provider_batch_consumer_panic_is_contained_and_fail_stopped() {
        let transport = include_str!("../../crates/zephium-agentic/src/provider_transport.rs");
        let provider = include_str!("../../crates/zephium-agentic/src/agent_provider.rs");
        validate_provider_consumer_panic_boundary(transport, provider)
            .expect("provider consumer panic boundary");

        for invalid in [
            transport.replacen("std::panic::catch_unwind", "Ok", 1),
            transport.replacen("                            self.fail_stop();", "", 1),
            transport.replace("        self.shared.shutdown.cancel();", ""),
            transport.replacen(
                "AgentProviderFailureClass::Integration",
                "AgentProviderFailureClass::Protocol",
                1,
            ),
        ] {
            assert!(validate_provider_consumer_panic_boundary(&invalid, provider).is_err());
        }
        assert!(validate_provider_consumer_panic_boundary(
            transport,
            &provider.replacen("            | Self::Integration", "", 1)
        )
        .is_err());
    }

    #[test]
    fn provider_transport_bounds_decoded_headers_before_status_or_body() {
        let root = include_str!("../../crates/zephium-agentic/src/provider_transport.rs");
        validate_provider_transport_response_header_boundary(root)
            .expect("provider transport response-header boundary");

        for invalid in [
            root.replacen(
                "            || !response_headers_admitted(response.headers())",
                "            || false",
                1,
            ),
            root.replacen(
                "            .checked_add(name.as_str().len())?",
                "            .checked_add(0)?",
                1,
            ),
            root.replacen(
                "                .checked_add(value.as_bytes().len())?",
                "                .checked_add(0)?",
                1,
            ),
            root.replacen(
                "                .checked_add(HTTP_HEADER_FIELD_OVERHEAD_BYTES)",
                "                .checked_add(0)",
                1,
            ),
            root.replacen(
                "        if response.url() != &self.endpoint || !response_headers_admitted(response.headers()) {",
                "        if response.status() != StatusCode::OK {",
                1,
            ),
            root.replacen(
                "        if response.url() != &self.endpoint || !response_headers_admitted(response.headers()) {",
                "        if false {",
                1,
            ),
            root.replacen(
                "        if !response_content_length_admitted(\n            response.headers(),\n            config.stream_budget().max_wire_bytes(),\n        ) || !response_encoding_admitted(response.headers())",
                "        if !response_encoding_admitted(response.headers())",
                1,
            ),
            root.replacen(
                "        .is_some_and(|declared| declared <= u64::from(max_wire_bytes))",
                "        .is_some()",
                1,
            ),
        ] {
            assert!(validate_provider_transport_response_header_boundary(&invalid).is_err());
        }
    }

    #[test]
    fn provider_transport_pins_http2_ingress_flow_control_and_frames() {
        let root = include_str!("../../crates/zephium-agentic/src/provider_transport.rs");
        validate_provider_transport_http2_ingress_boundary(root)
            .expect("provider transport HTTP/2 ingress boundary");

        for invalid in [
            root.replacen(
                "pub const AGENT_PROVIDER_HTTP2_INITIAL_RECEIVE_WINDOW_BYTES: u32 = 65_535;",
                "pub const AGENT_PROVIDER_HTTP2_INITIAL_RECEIVE_WINDOW_BYTES: u32 = 1_048_576;",
                1,
            ),
            root.replacen(
                "pub const MAX_AGENT_PROVIDER_HTTP2_FRAME_BYTES: u32 = 16 * 1_024;",
                "pub const MAX_AGENT_PROVIDER_HTTP2_FRAME_BYTES: u32 = 64 * 1_024;",
                1,
            ),
            root.replacen(
                ".http2_adaptive_window(false)",
                ".http2_adaptive_window(true)",
                1,
            ),
            root.replacen(
                "            .http2_initial_stream_window_size(AGENT_PROVIDER_HTTP2_INITIAL_RECEIVE_WINDOW_BYTES)",
                "",
                1,
            ),
            root.replacen(
                "            .http2_initial_connection_window_size(AGENT_PROVIDER_HTTP2_INITIAL_RECEIVE_WINDOW_BYTES)",
                "",
                1,
            ),
            root.replacen(
                ".http2_max_frame_size(MAX_AGENT_PROVIDER_HTTP2_FRAME_BYTES)",
                ".http2_max_frame_size(65_535)",
                1,
            ),
        ] {
            assert!(validate_provider_transport_http2_ingress_boundary(&invalid).is_err());
        }
    }

    #[test]
    fn provider_transport_shutdown_proof_requires_sticky_seal_and_exact_idle() {
        let root = include_str!("../../crates/zephium-agentic/src/provider_transport.rs");
        let lead_keys =
            include_str!("../../crates/zephium-agentic/src/provider_transport/lead/keys.rs");
        validate_provider_transport_shutdown_contract(root, lead_keys)
            .expect("provider transport shutdown proof boundary");

        for (index, invalid) in [
            root.replace("self.sealed && self.active == 0", "self.active == 0"),
            root.replace("if !snapshot.is_sealed()", "if false"),
            root.replace("if !snapshot.is_idle()", "if false"),
            root.replacen("self.shared.shutdown.cancel();", "", 1),
            root.replacen("self.seal();", "", 1),
            root.replace(
                ".compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)",
                ".store(true, Ordering::Release)",
            ),
            root.replace("let _wait_guard = wait_guard;", ""),
            root.replace("self.waiting.store(false, Ordering::Release);", ""),
            root.replace("drained.as_mut().enable();", ""),
            root.replace("self.shared.drained.notify_waiters();", ""),
            root.replace(
                "tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))",
                "std::future::pending()",
            ),
            root.replace(
                "#[must_use]\npub struct AgentProviderTransportShutdownProof",
                "#[derive(Clone, Copy)]\n#[must_use]\npub struct AgentProviderTransportShutdownProof",
            ),
            root.replace(
                "#[must_use]\npub struct AgentProviderTransportShutdownProof",
                "impl Clone for AgentProviderTransportShutdownProof { fn clone(&self) -> Self { unreachable!() } }\n#[must_use]\npub struct AgentProviderTransportShutdownProof",
            ),
            root.replacen(
                "\n#[cfg(test)]\nmod tests",
                "\nimpl AgentProviderTransportShutdownProof { pub fn new() {} }\n\n#[cfg(test)]\nmod tests",
                1,
            ),
            root.replacen(
                "\n#[cfg(test)]\nmod tests",
                "\ntokio::spawn(async {});\n\n#[cfg(test)]\nmod tests",
                1,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                validate_provider_transport_shutdown_contract(&invalid, lead_keys).is_err(),
                "provider shutdown proof mutation {index} was not rejected"
            );
        }
    }

    #[test]
    fn keychain_retry_exceptions_are_single_exact_and_loader_local() {
        let root = include_str!("../../crates/zephium-agentic/src/provider_transport.rs");
        let keys = include_str!("../../crates/zephium-agentic/src/provider_transport/lead/keys.rs");
        validate_provider_transport_shutdown_contract(root, keys).expect("reviewed retries");
        for (source, loader, is_lead) in [
            (root, KEYCHAIN_LOGIN_LOADER, false),
            (keys, LEAD_KEYCHAIN_LOADER, true),
            (root, WINDOWS_VAULT_LOADER, false),
            (keys, LEAD_WINDOWS_VAULT_LOADER, true),
        ] {
            let sleep = if loader == WINDOWS_VAULT_LOADER {
                WINDOWS_VAULT_RETRY_SLEEP
            } else {
                KEYCHAIN_RETRY_SLEEP
            };
            for changed in [
                loader.replace("from_millis(40)", "from_millis(41)"),
                loader.replace(sleep, &format!("{sleep}{sleep}")),
                loader.replace(sleep, &format!("loop {{ {sleep} }}")),
                loader.replace("::Inaccessible)", "::Missing)"),
                loader
                    .replace("let _turn = keychain_turn().map_err(vault_error)?;", "")
                    .replace("let _turn = turn()?;", "")
                    .replace(
                        "let _turn = crate::provider_transport::keychain_turn().map_err(map)?;",
                        "",
                    ),
            ] {
                assert_ne!(changed, loader);
                let invalid = source.replacen(loader, &changed, 1);
                let result = if is_lead {
                    validate_provider_transport_shutdown_contract(root, &invalid)
                } else {
                    validate_provider_transport_shutdown_contract(&invalid, keys)
                };
                assert!(result.is_err());
            }
            // The identical sleep outside its approved loader is still forbidden.
            let invalid = format!("fn outside_loader() {{ {KEYCHAIN_RETRY_SLEEP} }}\n{source}");
            let result = if is_lead {
                validate_provider_transport_shutdown_contract(root, &invalid)
            } else {
                validate_provider_transport_shutdown_contract(&invalid, keys)
            };
            assert!(result.is_err());
        }
    }

    #[test]
    fn provider_billing_mode_requires_fixed_requests_and_terminal_attestation() {
        let root = r#"
            const OPENAI_STANDARD_SERVICE_TIER: &str = "default";
            const ANTHROPIC_STANDARD_SERVICE_TIER_REQUEST: &str = "standard_only";
            const ANTHROPIC_STANDARD_SERVICE_TIER_RESPONSE: &str = "standard";
            const ANTHROPIC_GLOBAL_INFERENCE_GEO: &str = "global";
            pub enum AgentProviderBillingClass {}
            pub enum AgentProviderResponseRoute {}
            struct Config { catalog: Arc<AgentProviderCatalogBinding> }
            pub(in crate::agent_provider) fn from_catalog() {}
            pub fn billing_class(&self) -> AgentProviderBillingClass {}
            pub fn pricing_profile(&self) -> AgentProviderPricingProfile {}
            pub fn response_route(&self) -> AgentProviderResponseRoute {}
            pub fn accounting_guard(&self) -> [u8; 32] {}
            if !self.pricing_profile().contains(total_input_tokens) {}
        "#;
        let request = r#"
            OpenAiRequestWire {
                service_tier: config.response_route().request_service_tier()
            };
            AnthropicRequestWire {
                service_tier: config.response_route().request_service_tier(),
                inference_geo: config.response_route().response_inference_geo(),
            };
        "#;
        let openai = r#"
            if event.response.service_tier
                != self.config.response_route().response_service_tier() {}
            AgentProviderResponseIdentity::try_attested(
                &self.config,
                event.response.model,
                event.response.service_tier,
                None,
            );
        "#;
        let anthropic = r#"
            AgentProviderResponseIdentity::try_attested(
                &self.config,
                event.message.model,
                event.message.usage.service_tier,
                Some(event.message.usage.inference_geo),
            );
        "#;

        validate_provider_billing_contract(root, request, openai, anthropic)
            .expect("valid fixed billing boundary");
        assert!(validate_provider_billing_contract(
            &root.replace("\"default\"", "\"auto\""),
            request,
            openai,
            anthropic,
        )
        .is_err());
        assert!(validate_provider_billing_contract(
            root,
            &format!("{request}\nstruct Drift {{ speed: &'static str }}"),
            openai,
            anthropic,
        )
        .is_err());
        assert!(validate_provider_billing_contract(root, request, "", anthropic,).is_err());
        assert!(validate_provider_billing_contract(root, request, openai, "").is_err());
    }

    #[test]
    fn provider_pricing_requires_checked_identity_range_and_move_only_join() {
        let pricing = r#"
            pub struct AgentProviderPricingSchedule {
                binding: Arc<AgentProviderCatalogBinding>,
            }
            fn exact_binding_matches(&self, config: &AgentProviderCallConfig) -> bool {
                config.accounting_guard() == self.binding.guard
                    && config.catalog.as_ref() == self.binding.as_ref()
            }
            if !self.binding.matches_response_identity(identity) {}
            if !self.binding.profile.contains(usage.input_tokens()) {}
            usage.cached_input_tokens().checked_add(usage.cache_write_input_tokens());
            usage.input_tokens().checked_sub(priced_input_subsets);
            u64::try_from(rounded);
            pub(crate) struct AgentProviderPricedUsage;
            pub struct AgentProviderPricingAttribution {
                schedule_guard: [u8; 32],
                response_identity_guard: [u8; 32],
            }
            fn retain(value: AgentProviderPricingAttribution) {}
        "#;
        let policy = r#"
            enum AgentModelUsageAccounting { PricedCeiling }
            struct Receipt {
                pricing_attribution: Option<AgentProviderPricingAttribution>,
            }
            pub(crate) fn prevalidate_active_model_call() {}
            pub(crate) fn settle_model_call_priced() {
                AgentModelUsageAccounting::PricedCeiling;
                Receipt { pricing_attribution: Some(pricing_attribution) };
            }
        "#;
        let settlement = r#"
            pub fn settle(
                self,
                policy: &mut AgentRunPolicy,
                schedule: &AgentProviderPricingSchedule,
            ) {
                policy.prevalidate_active_model_call(&self.active);
                if !schedule.exact_binding_matches(&self.config) {}
                schedule.try_price(&self.config, self.identity, self.usage);
                policy.settle_model_call_priced(active, settlement, priced);
            }
            pub fn settle_at_reservation_ceiling() {
                AgentModelCallUnaccountedSettlement::ProviderFailed;
            }
            struct Error { unsettled: Box<AgentProviderPricingSettlement> }
            pub fn into_unsettled(self) -> Option<AgentProviderPricingSettlement> {}
        "#;

        validate_provider_pricing_contract(pricing, policy, settlement)
            .expect("valid checked pricing boundary");
        assert!(validate_provider_pricing_contract(
            &pricing.replace("config.catalog.as_ref() == self.binding.as_ref()", "true"),
            policy,
            settlement,
        )
        .is_err());
        assert!(validate_provider_pricing_contract(
            pricing,
            policy,
            &format!(
                "{settlement}\npub fn settle(self, policy: &mut AgentRunPolicy, cost_micro_usd: u64) {{}}"
            ),
        )
        .is_err());
    }

    #[test]
    fn engine_probe_requires_optional_dependency_binary_and_exact_feature() {
        let valid = r#"
            [features]
            native-agentic-input-probe = [
              "dep:serde",
              "dep:tempfile",
              "dep:windows-probe-sys",
              "dep:zephium-agentic",
              "zephium-agentic/probe-harness",
            ]
            agentic-browser = ["dep:zephium-agentic", "dep:image"]
            native-agentic-semantic-probe = [
              "agentic-browser",
              "dep:tempfile",
              "dep:windows-probe-sys",
              "zephium-agentic/probe-harness",
            ]
            [[bin]]
            name = "macos-agentic-input-probe"
            path = "src/bin/macos_agentic_input_probe.rs"
            required-features = ["native-agentic-input-probe"]
            [[bin]]
            name = "windows-agentic-input-probe"
            path = "src/bin/windows_agentic_input_probe.rs"
            required-features = ["native-agentic-input-probe"]
            [[bin]]
            name = "macos-agentic-semantic-probe"
            path = "src/bin/macos_agentic_semantic_probe.rs"
            required-features = ["native-agentic-semantic-probe"]
            [[bin]]
            name = "windows-agentic-semantic-probe"
            path = "src/bin/windows_agentic_semantic_probe.rs"
            required-features = ["native-agentic-semantic-probe"]
            [[bin]]
            name = "windows-cookie-persistence-control"
            path = "src/bin/windows_cookie_persistence_control.rs"
            required-features = ["native-agentic-semantic-probe"]
            [dependencies]
            zephium-agentic = { optional = true }
            zeroize = "=1.9.0"
            image = { version = "=0.25.10", default-features = false, features = ["png", "jpeg", "webp", "gif"], optional = true }
            [target.'cfg(target_os = "windows")'.dependencies]
            windows = { version = "0.61", features = ["Win32_System_ProcessStatus", "Win32_System_Threading"] }
            windows-probe-sys = { package = "windows-sys", version = "=0.61.2", optional = true, features = [
              "Win32_Foundation",
              "Win32_System_ProcessStatus",
              "Win32_System_Threading",
            ] }
        "#;
        validate_engine_manifest(valid).expect("valid engine probe gate");
        assert!(
            validate_engine_manifest(&valid.replace("optional = true", "optional = false"))
                .is_err()
        );
        assert!(validate_engine_manifest(
            &valid.replace("zephium-agentic/probe-harness", "shipping-probe")
        )
        .is_err());
        assert!(validate_engine_manifest(&valid.replace(
            "agentic-browser = [\"dep:zephium-agentic\", \"dep:image\"]",
            "agentic-browser = [\"zephium-agentic/probe-harness\"]",
        ))
        .is_err());
        assert!(validate_engine_manifest(&valid.replace("=1.9.0", "=1.9.1")).is_err());
        assert!(validate_engine_manifest(&valid.replace(
            "zeroize = \"=1.9.0\"",
            "zeroize = { version = \"=1.9.0\", optional = true }",
        ))
        .is_err());
        assert!(validate_engine_manifest(&valid.replace("=0.25.10", "=0.25.11")).is_err());
        assert!(validate_engine_manifest(
            &valid.replace("default-features = false", "default-features = true")
        )
        .is_err());
        assert!(validate_engine_manifest(
            &valid.replace("\"webp\", \"gif\"]", "\"webp\", \"gif\", \"bmp\"]")
        )
        .is_err());
        assert!(validate_engine_manifest(&valid.replace(
            "\"Win32_System_ProcessStatus\"",
            "\"Win32_System_Threading\""
        ))
        .is_err());
        assert!(validate_engine_manifest(&valid.replace(
            "\"Win32_System_ProcessStatus\", \"Win32_System_Threading\"]",
            "\"Win32_System_ProcessStatus\", \"Win32_System_ProcessStatus\"]"
        ))
        .is_err());
    }

    #[test]
    fn page_memory_keeps_limited_query_rights() {
        let source =
            include_str!("../../crates/zephium-engine/src/platform/windows/page_memory.rs");
        validate_engine_page_memory(source).expect("shipping page memory");
        assert!(validate_engine_page_memory(&source.replace(
            "PROCESS_QUERY_LIMITED_INFORMATION, false",
            "PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false"
        ))
        .is_err());
        assert!(
            validate_engine_page_memory(&source.replace("let _ = CloseHandle(process);", ""))
                .is_err()
        );
        assert!(validate_engine_page_memory(&format!(
            "{source}\nfn leak(p: HANDLE) {{ ReadProcessMemory(p); }}"
        ))
        .is_err());
    }

    #[test]
    fn presented_rendering_rejects_input_authority_hidden_hacks_unbounded_leases_and_missing_restore(
    ) {
        let source = include_str!(
            "../../crates/zephium-engine/src/platform/macos/agentic_rendering_presented_probe.rs"
        );
        let parent = include_str!(
            "../../crates/zephium-engine/src/platform/macos/agentic_semantic_probe.rs"
        );
        let binary =
            include_str!("../../crates/zephium-engine/src/bin/macos_agentic_semantic_probe.rs");
        validate_macos_rendering_presented_probe(source, parent, binary).unwrap();
        for required in [
            "Duration::from_secs(5)",
            "const MAX_APPKIT_EVENTS: usize = 32;",
            "actual == expected && !active && windows == 0",
            "app.activationPolicy() != NSApplicationActivationPolicy::Prohibited",
            "app.activationPolicy() != NSApplicationActivationPolicy::Accessory",
            "app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited)",
            "app.finishLaunching();",
            "app.setActivationPolicy(NSApplicationActivationPolicy::Accessory)",
            "event_type == NSEventType::AppKitDefined && dispatched < MAX_APPKIT_EVENTS",
            "NSEventMask::AppKitDefined,",
            "self.app.sendEvent(&event);",
            "operation.context() != context",
            "original.window.canBecomeKeyWindow()",
            "original.window.canBecomeMainWindow()",
            "!self.app.isActive()",
            "self.window.ignoresMouseEvents()",
            "self.first_responder",
            "responder_owned: responder_is_owned(",
            "exact_page != 0 && (actual == original || actual == Some(exact_page))",
            "Some(\"presented_responder_changed\") => Some(self.responder_refusal(responder))",
            "Some(\"presented_surface_geometry\") => geometry_failure",
            "admit_actual_frame(scope.window.frame(), visible_screen)?",
            "actual.size != NSSize::new(1280.0, 800.0)",
            "expected_window_frame: presented_frame",
            "self.page.frame() != viewport_frame()",
            "scope.page.setFrame(viewport_frame())",
            "self.page.setFrame(self.original_page_frame)",
            "self.page.frame() != self.original_page_frame",
            "self.page.visibleRect() == viewport_frame()",
            "self.window.alphaValue() == 1.0",
            "NSWindowOcclusionState::Visible",
            "scope.window.setIgnoresMouseEvents(true)",
            "guard.require_visible_pixels.set(true)",
            "let restore = scope.hide();",
            "restore.err().or_else(|| runtime.failure_stage())",
            "self.window.orderOut(None)",
            "impl Drop for PresentedScope",
        ] {
            assert!(source.contains(required));
            assert!(validate_macos_rendering_presented_probe(
                &source.replace(required, "removed"),
                parent,
                binary
            )
            .is_err());
        }
        for forbidden in [
            "window.makeKeyWindow()",
            "app.activateIgnoringOtherApps()",
            "window.setAlphaValue(0.0)",
            "preferences.setValue_forKey()",
            "page.evaluateJavaScript()",
            "CGEvent",
            "NSEvent::",
            "NSEventMask::Any",
            "NSEventMask::KeyDown",
            "NSEventMask::SystemDefined",
            "app.postEvent()",
            "window.setLevel()",
            "window.orderFrontRegardless()",
            "page.setHidden(false)",
            "std::env::var(\"KEY\")",
            "reqwest::get(url)",
        ] {
            assert!(validate_macos_rendering_presented_probe(
                &format!("{source}\n{forbidden}"),
                parent,
                binary
            )
            .is_err());
        }
        assert!(validate_macos_rendering_presented_probe(
            source,
            &parent.replace("match finish(pending)", "match Ok(None)"),
            binary
        )
        .is_err());
        assert!(validate_macos_rendering_presented_probe(
            source,
            parent,
            &binary.replace("--ci-presented-rendering-readiness", "--arbitrary-url")
        )
        .is_err());
    }

    #[test]
    fn rendering_opportunity_rejects_policy_leaks_extra_dispatch_and_presentation_changes() {
        let source = include_str!(
            "../../crates/zephium-engine/src/platform/macos/agentic_rendering_opportunity_probe.rs"
        );
        let parent = include_str!(
            "../../crates/zephium-engine/src/platform/macos/agentic_semantic_probe.rs"
        );
        let binary =
            include_str!("../../crates/zephium-engine/src/bin/macos_agentic_semantic_probe.rs");
        validate_macos_rendering_opportunity_probe(source, parent, binary).unwrap();
        for required in [
            "Duration::from_secs(1)",
            "original != WKInactiveSchedulingPolicy::Throttle",
            "scope.policy()? != WKInactiveSchedulingPolicy::None",
            "self.policy()? != WKInactiveSchedulingPolicy::Throttle",
            "scope.restore()?",
            "snapshot.attest()?",
            "NSSize::new(1.0, 1.0)",
            "operation.context() != context",
            "document_finished_for_audit(operation)",
            "runtime.native_guard.sample()",
        ] {
            assert!(source.contains(required));
            assert!(validate_macos_rendering_opportunity_probe(
                &source.replace(required, "removed"),
                parent,
                binary
            )
            .is_err());
        }
        for forbidden in [
            "page.setHidden(false)",
            "window.orderFront(None)",
            "preferences.setValue_forKey()",
            "page.evaluateJavaScript()",
            "reqwest::get(url)",
            "std::env::var(\"KEY\")",
            "CGImage",
            "page.takeSnapshotWithConfiguration_completionHandler()",
            "preferences.setInactiveSchedulingPolicy()",
        ] {
            assert!(validate_macos_rendering_opportunity_probe(
                &format!("{source}\n{forbidden}"),
                parent,
                binary
            )
            .is_err());
        }
        assert!(validate_macos_rendering_opportunity_probe(
            source,
            &parent.replace("finish(pending)?", "Ok(None)?"),
            binary
        )
        .is_err());
        assert!(validate_macos_rendering_opportunity_probe(
            source,
            parent,
            &binary.replace("--ci-hidden-rendering-snapshot", "--arbitrary-url")
        )
        .is_err());
    }

    #[test]
    fn rendering_diagnostic_rejects_unbounded_sampling_policy_changes_and_false_evidence() {
        let source = include_str!(
            "../../crates/zephium-engine/src/platform/macos/agentic_rendering_probe.rs"
        );
        let parent = include_str!(
            "../../crates/zephium-engine/src/platform/macos/agentic_semantic_probe.rs"
        );
        let binary =
            include_str!("../../crates/zephium-engine/src/bin/macos_agentic_semantic_probe.rs");
        let fixture = include_str!("../../crates/zephium-agentic/src/fixture_server.rs");
        validate_macos_rendering_probe(source, parent, binary, fixture).unwrap();
        for (required, replacement) in [
            (
                "if (window.location.hash === '#windows-retained-progress')",
                "if (true)",
            ),
            ("let ticks = 0;", "let ticks = -1;"),
            ("ticks += 1;", "ticks += 0;"),
            ("if (ticks === 4096) clearInterval(timer);", ""),
            ("}, 500);", "}, 1);"),
            (
                "  window.addEventListener('load',",
                "  setInterval(() => {}, 500);\n  window.addEventListener('load',",
            ),
            (
                "progress.textContent = 'Retained native progress 0';",
                "fetch('https://example.test');",
            ),
        ] {
            assert!(
                fixture.contains(required),
                "mutation must alter the real fixture"
            );
            let invalid = fixture.replacen(required, replacement, 1);
            assert!(validate_macos_rendering_probe(source, parent, binary, &invalid).is_err());
        }
        for required in [
            "Duration::from_secs(5)",
            "document_finished_for_audit(operation)",
            "snapshot.frame().context() != context",
            "snapshot.frame().origin() != origin",
            "snapshot.completeness() != SemanticCompleteness::Complete",
            "sample.elapsed_ms >= SAMPLE_OFFSETS_MS[7]",
        ] {
            assert!(source.contains(required));
            assert!(validate_macos_rendering_probe(
                &source.replace(required, "removed"),
                parent,
                binary,
                fixture
            )
            .is_err());
        }
        for forbidden in [
            "view.setHidden(false)",
            "view.setInactiveSchedulingPolicy(None)",
            "evaluateJavaScript()",
            "dispatch_semantic_action()",
            "std::env::var(\"KEY\")",
            "reqwest::get(url)",
        ] {
            assert!(validate_macos_rendering_probe(
                &format!("{source}\n{forbidden}"),
                parent,
                binary,
                fixture
            )
            .is_err());
        }
        assert!(validate_macos_rendering_probe(
            source,
            &parent.replace("finish(pending)?", "Ok(None)?"),
            binary,
            fixture
        )
        .is_err());
        assert!(validate_macos_rendering_probe(
            source,
            parent,
            &binary.replace("--ci-hidden-rendering-readiness", "--arbitrary-url"),
            fixture
        )
        .is_err());
        assert!(validate_macos_rendering_probe(
            source,
            parent,
            binary,
            &fixture.replace("requestAnimationFrame(() =>", "setTimeout(() =>")
        )
        .is_err());
    }

    #[test]
    fn engine_source_guards_are_exact() {
        let root = r#"
            #[cfg(all(feature = "native-agentic-input-probe", not(debug_assertions)))]
            compile_error!("the native agentic input probe is forbidden in optimized builds");
            #[cfg(all(feature = "native-agentic-semantic-probe", not(debug_assertions)))]
            compile_error!("the native agentic semantic probe is forbidden in optimized builds");
            #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
            pub fn run_macos_agentic_semantic_probe() -> Result<(), &'static str> {
                platform::macos::run_agentic_semantic_probe()
            }
            pub fn run_macos_agentic_semantic_model_public_fill_probe(
                prepare: impl FnMut(),
            ) -> Result<(), &'static str> {
                platform::macos::run_agentic_semantic_model_public_fill_probe(prepare)
            }
            pub fn run_macos_agentic_semantic_model_two_action_probe(
                scenario: MacosAgenticSemanticTwoActionScenario,
                prepare_initial: impl FnMut(),
                prepare_continuation: impl FnMut(),
                verify_final: impl FnMut(),
            ) -> Result<(), &'static str> {
                platform::macos::run_agentic_semantic_model_two_action_probe(
                    scenario,
                    prepare_initial,
                    prepare_continuation,
                    verify_final,
                )
            }
            #[cfg(all(target_os = "windows", feature = "native-agentic-semantic-probe"))]
            pub fn run_windows_agentic_semantic_probe(
                request_id: u64,
                mode: zephium_agentic::WindowsSemanticProbeMode,
            ) -> Result<
                zephium_agentic::WindowsSemanticProbeEvidence,
                zephium_agentic::WindowsSemanticProbeFailure,
            > {
                platform::windows::run_agentic_semantic_probe(request_id, mode)
            }
        "#;
        validate_engine_root(root).expect("valid engine release refusal");
        assert!(validate_engine_root("pub fn shipping() {}").is_err());

        let module = r#"
            #[cfg(feature = "native-agentic-input-probe")]
            mod agentic_input_probe;
            #[cfg(feature = "native-agentic-input-probe")]
            pub(crate) use agentic_input_probe::run as run_agentic_input_matrix;
        "#;
        validate_engine_platform_module(module, "test").expect("valid module gates");
        assert!(validate_engine_platform_module("mod agentic_input_probe;", "test").is_err());

        let mut semantic_module = format!(
            "{module}\n#[cfg(feature = \"native-agentic-semantic-probe\")]\nmod agentic_semantic_probe;\n#[cfg(feature = \"native-agentic-semantic-probe\")]\npub(crate) use agentic_semantic_probe::run as run_agentic_semantic_probe;\n#[cfg(feature = \"native-agentic-semantic-probe\")]\npub(crate) use agentic_semantic_probe::run_model_public_fill as run_agentic_semantic_model_public_fill_probe;\n#[cfg(feature = \"native-agentic-semantic-probe\")]\npub(crate) use agentic_semantic_probe::run_model_two_action as run_agentic_semantic_model_two_action_probe;"
        );
        semantic_module.push_str(
            "\n#[cfg(feature = \"native-agentic-semantic-probe\")]\npub(crate) use agentic_semantic_probe::run_model_workflow as run_agentic_semantic_model_workflow_probe;",
        );
        let semantic_source = r#"
            FixtureRoute::SemanticRuntime;
            FixtureRoute::SemanticRuntimeMutation;
            FixtureRoute::SemanticRuntimeReplacement;
            FixtureServer::start();
            new_ephemeral_data_store();
            ContextProfileStorageClass::Ephemeral;
            ContextOwnedViewport::STANDARD;
            build_owned_agent_view();
            NativeContentPolicy::AllowAll;
            NSApplicationActivationPolicy::Accessory;
            NSWindowStyleMask::Borderless;
            window.orderOut(None);
            !self.page.isHidden();
            self.app.isActive();
            return Err("focus_baseline");
            view.prepare_semantic_document_load();
            view.dispatch_semantic();
            ContextCapability::Act;
            verify_primary_click_execution();
            verify_hostile_fill_refusal();
            SemanticActionNativeFailure::AppliedUnverified;
            execution.settle_and_verify(pending.settlement, snapshot, observed_at);
            SemanticObservationAssembler::new();
            SemanticFrameUnsupported::PlatformIsolationUnavailable;
            SemanticRuntimeFault::AnchorMissing;
            wait_for_mutation_gate();
            server.release_semantic_mutation();
            wait_for_mutation_application();
            verify_mutation_before();
            verify_mutation_after();
            server.semantic_mutation_completed();
            struct NativeStateGuard;
            pump_once(self.run_loop, Some(self.native_guard));
            view.attest();
            SemanticRuntimeFault::DocumentLoading;
            MAX_DOCUMENT_LOADING_RETRIES;
            view.retire_semantic_runtime();
            Weak::from_retained(&page);
            server.shutdown();
            const PUBLIC_DISCOVERY_PROBE_URL: &str = "https://www.wikipedia.org/";
            ProbeMode::ModelPublicFill;
            MacosAgenticSemanticTwoActionScenario::PublicFillSelect;
            SemanticActionKind::Select;
            url == PUBLIC_DISCOVERY_PROBE_URL;
            allow_hidden_responder_change: public_fill_probe;
            ProbeMode::ModelWorkflow { initial, next };
            for _ in 0..8 {}
            registry.acknowledge_observation(identity.id(), first);
            generation = capture.snapshot.generation();
            None => return Ok(None),
            let execution = execution.and_then(|terminal| {});
        "#;
        let semantic_binary = r#"
            if arguments.as_slice() != ["--ci-hidden-fixed-dom"] {}
            run_macos_agentic_semantic_probe();
            "profile=ephemeral viewport=1280x800-logical fixture=loopback-only snapshots=18 snapshots=22 world_epochs=4 fixed_click=verified click_postcondition=expanded isolated_fixed_fill=verified controls=text-input,search-input,textarea,contenteditable,nested-leaf nested_delegated_model=retained nested_context_attacks=6-refused-nonretryable fill_postcondition=exact-value event_trust=untrusted user_activation=0 popup_admitted=0 mutation_gate=host-released stale_anchor=refused mutation_recovery=verified epoch_rotation_fill=verified focus_theft=0 retained_views=0 hostile_terminal_spoof=ignored hostile_cross_node_terminal=refused hostile_reparent_type_repurpose=applied-unverified hostile_reparent_recovery=verified hostile_credential_relabel=applied-unverified hostile_credential_recovery=verified";
        "#;
        let semantic_fixture = r#"
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0));
            if !address.ip().is_loopback() {}
            format!("http://127.0.0.1:{}{}", port, path);
            Self::SemanticRuntime => "/semantic-runtime-v1.html";
            Self::SemanticRuntimeRelayHostile => "/semantic-runtime-relay-hostile-v1.html";
            Self::SemanticRuntimeMutation => "/semantic-runtime-mutation-v1.html";
            Self::SemanticRuntimeReplacement => "/semantic-runtime-replacement-v1.html";
            const SEMANTIC_MUTATION_GATE_TIMEOUT: Duration = Duration::from_secs(15);
            const SEMANTIC_MUTATION_TRIGGER_PATH: &str = "/semantic-runtime-mutation-trigger-v1.js";
            struct SemanticGate { wake: Condvar }
            if state.waiting || state.released || state.completed {}
            if !state.waiting || state.released || state.completed {}
            semantic_mutation.wait_for_release(stop, SEMANTIC_MUTATION_GATE_TIMEOUT);
            semantic_mutation.mark_completed();
            "application/javascript; charset=utf-8";
            "<script defer src="/semantic-runtime-mutation-trigger-v1.js"></script>";
            FixtureScriptPolicy::SameOrigin;
            "connect-src 'none'; form-action 'none'; frame-src 'self'";
            <details><summary id="primary-semantic-action"><span>Primary semantic action</span></summary><p>Native disclosure content</p></details>
            "Semantic hostile fill";
            "Semantic hostile recovery";
            "Semantic hostile credential fill";
            "semantic-hostile-credential-label";
            credential.setAttribute('aria-labelledby', 'semantic-hostile-credential-label');
            credential.removeAttribute('aria-labelledby');
            main.insertBefore(hostile, hostileNextSibling);
        "#;
        validate_macos_semantic_probe(
            &semantic_module,
            semantic_source,
            semantic_binary,
            semantic_fixture,
        )
        .expect("valid semantic probe gate");
        for boundary in [
            "for _ in 0..8 {}",
            "registry.acknowledge_observation(identity.id(), first);",
            "let execution = execution.and_then(|terminal| {});",
        ] {
            assert!(validate_macos_semantic_probe(
                &semantic_module,
                &semantic_source.replace(boundary, ""),
                semantic_binary,
                semantic_fixture,
            )
            .is_err());
        }
        assert!(validate_macos_semantic_probe(
            &semantic_module,
            &format!("{semantic_source}\nevaluate_script();"),
            semantic_binary,
            semantic_fixture,
        )
        .is_err());
        assert!(validate_macos_semantic_probe(
            &semantic_module,
            &semantic_source.replace("SemanticRuntimeFault::AnchorMissing;", ""),
            semantic_binary,
            semantic_fixture,
        )
        .is_err());
        assert!(validate_macos_semantic_probe(
            &semantic_module,
            semantic_source,
            &semantic_binary.replace("mutation_recovery=verified", ""),
            semantic_fixture,
        )
        .is_err());
        assert!(validate_macos_semantic_probe(
            &semantic_module,
            semantic_source,
            &semantic_binary.replace("fixed_click=verified", ""),
            semantic_fixture,
        )
        .is_err());
        assert!(validate_macos_semantic_probe(
            &semantic_module,
            semantic_source,
            &semantic_binary.replace("postcondition=expanded", ""),
            semantic_fixture,
        )
        .is_err());
        assert!(validate_macos_semantic_probe(
            &semantic_module,
            semantic_source,
            &semantic_binary.replace("user_activation=0", ""),
            semantic_fixture,
        )
        .is_err());
        assert!(validate_macos_semantic_probe(
            &semantic_module,
            semantic_source,
            semantic_binary,
            &semantic_fixture.replace("wake: Condvar", "wake: bool"),
        )
        .is_err());
    }

    #[test]
    fn production_cookie_preflight_is_bounded_and_secret_owning() {
        let preflight =
            include_str!("../../crates/zephium-engine/src/platform/agent_cookie_preflight.rs");
        validate_engine_agent_cookie_preflight(preflight)
            .expect("bounded secret-owning cookie preflight");
        for invalid in [
            preflight.replace(
                "name: AgentCookieText,\n        value: AgentCookieText,",
                "name: String,\n        value: String,",
            ),
            preflight.replace("> MAX_COOKIES_PER_TRANSFER", "> usize::MAX"),
            preflight.replace(".same_snapshot(&fields)", ".same_identity(&fields)"),
            preflight.replace(".and_then(Option::take)", ".and_then(Option::as_ref)"),
            preflight.replace(
                "pub(crate) struct AgentCookieFields",
                "#[derive(Debug)]\npub(crate) struct AgentCookieFields",
            ),
            preflight.replace(
                "now.checked_duration_since(admitted_at)?;",
                "let _ = admitted_at;",
            ),
            preflight.replace(
                "Duration::from_millis(window.duration_millis())",
                "Duration::from_secs(300)",
            ),
        ] {
            assert!(validate_engine_agent_cookie_preflight(&invalid).is_err());
        }
    }

    #[test]
    fn cookie_transfer_deadline_is_bounded_exact_and_clock_free() {
        let root = include_str!("../../crates/zephium-agentic/src/lib.rs");
        let transfer = include_str!("../../crates/zephium-agentic/src/cookie_transfer.rs");
        validate_cookie_transfer_deadline_contract(root, transfer)
            .expect("bounded cookie transfer deadline");
        for invalid in [
            transfer.replace("pub const MAX_COOKIE_TRANSFER_MILLIS: u64 = 30_000;", ""),
            transfer.replace(
                ".checked_sub(requested_at.millis())",
                ".saturating_sub(requested_at.millis())",
            ),
            transfer.replace("duration > MAX_COOKIE_TRANSFER_MILLIS", "false"),
            transfer.replacen("window: ContextCookieTransferWindow,", "", 1),
            format!("{transfer}\nfn runtime_clock() {{ let _ = std::time::Instant::now(); }}"),
        ] {
            assert!(validate_cookie_transfer_deadline_contract(root, &invalid).is_err());
        }
        assert!(validate_cookie_transfer_deadline_contract(
            &root.replace("ContextCookieTransferWindow", "MissingCookieWindow"),
            transfer,
        )
        .is_err());
    }

    #[test]
    fn production_windows_cookie_adapter_is_bounded_callback_driven_and_recoverable() {
        let module = include_str!("../../crates/zephium-engine/src/platform/windows/mod.rs");
        let transfer =
            include_str!("../../crates/zephium-engine/src/platform/windows/cookie_transfer.rs");
        validate_engine_windows_cookie_transfer(module, transfer)
            .expect("bounded dormant Windows cookie adapter");
        // Target this module's gate: adding another preceding Windows module
        // must not turn the negative fixture into an unrelated mutation.
        let cookie_module_at = module.find("mod cookie_transfer;").expect("cookie module");
        let cookie_gate_at = module[..cookie_module_at]
            .rfind("#[cfg")
            .expect("cookie gate");
        let cookie_gate_end =
            cookie_gate_at + module[cookie_gate_at..].find(']').expect("cookie gate end") + 1;
        let mut ungated_cookie_module = module.to_owned();
        ungated_cookie_module.replace_range(cookie_gate_at..cookie_gate_end, "");
        let invalid_modules = [ungated_cookie_module];
        for invalid in invalid_modules {
            assert!(validate_engine_windows_cookie_transfer(&invalid, transfer).is_err());
        }
        for invalid in [
            transfer.replace(
                "AgentCookiePreflight::try_new(scope.len())",
                "AgentCookiePreflight::try_new(1)",
            ),
            transfer.replace("request.scope().clone()", "ContextCookieScope::default()"),
            compact(transfer).replace("callback_panicked,true,)", "callback_panicked,false,)"),
            transfer.replace("count as usize > MAX_COOKIES_PER_TRANSFER", "false"),
            transfer.replace("bytes > MAX_COOKIE_TRANSFER_BYTES", "false"),
            transfer.replace(
                "if !fields.supersedes(&current) => continue",
                "if false => continue",
            ),
            transfer.replace("seed.attempted.push(cookie.clone());", ""),
            transfer.replace(
                "destination.DeleteCookie(attempted)",
                "destination.DeleteAllCookies()",
            ),
            transfer.replace("destination.AddOrUpdateCookie(prior)", "Ok(())"),
            transfer.replace(
                "cookie.identity == *identity) == *restored",
                "cookie.identity == *identity) || true",
            ),
            transfer.replace(
                "state.destination.CopyCookie(&source_cookie)",
                "source_cookie.clone()",
            ),
            transfer.replace("count > maximum", "false"),
            transfer.replace(
                "state.phase = TransferPhase::Applying { in_flight: true };",
                "state.phase = TransferPhase::Applying { in_flight: false };",
            ),
            transfer.replace("let _ = unsafe { destination.DeleteAllCookies() };", ""),
            transfer.replace("profile.ClearBrowsingDataAll(&handler)", "handler.clone()"),
            transfer.replace(
                "destination.GetCookies(PCWSTR::null(), &handler)",
                "destination.GetCookies(PCWSTR::from_raw(origin.as_ptr()), &handler)",
            ),
            transfer.replace("shared.cancellation.set(None);", ""),
            transfer.replace(
                "super::same_environment(&view.environment(), expected_environment)",
                "true",
            ),
            transfer.replace(
                "(core.Environment(), core.CookieManager())",
                "(Ok(expected_environment.clone()), core.CookieManager())",
            ),
            transfer.replace(
                "request: &ContextCookieTransferRequest,",
                "scope: ContextCookieScope,",
            ),
            transfer.replace(
                "map_cookie_transfer_deadline(request.window(), admitted_at, now)",
                "admitted_at.checked_add(Duration::from_secs(300))",
            ),
            format!("{transfer}\nstd::sync::mpsc::channel();"),
            format!("{transfer}\nevaluate_script();"),
        ] {
            assert!(validate_engine_windows_cookie_transfer(module, &invalid).is_err());
        }
    }

    #[test]
    fn production_semantic_runtime_requires_the_exact_public_webkit_boundary() {
        let runtime = r#"
            addScriptMessageHandlerWithReply_contentWorld_name();
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld();
            WKUserScriptInjectionTime::AtDocumentStart, true, &world;
            message.world();
            message.name();
            message.webView();
            message.frameInfo();
            frame.isMainFrame();
            NEXT_SEMANTIC_RUNTIME_WORLD.fetch_update();
            SEMANTIC_RUNTIME_WORLD_NAME_PREFIX;
            channel.world_matches(&world);
            body.lengthOfBytesUsingEncoding(NSUTF8StringEncoding);
            MAX_SEMANTIC_RUNTIME_CHANNEL_RESULT_BYTES;
            MAX_SEMANTIC_RUNTIME_DOCUMENT_INVOCATIONS;
            SEMANTIC_RUNTIME_PROGRAM.installation_source();
            if scripts.count() != 1 {}
            let script = scripts.objectAtIndex(0);
            Retained::as_ptr(&script) != Retained::as_ptr(&active.script);
            source.to_string() != SEMANTIC_RUNTIME_PROGRAM.installation_source();
            removeScriptMessageHandlerForName_contentWorld();
            controller.removeAllScriptMessageHandlers();
            controller.removeAllUserScripts();
            fn begin_document_load() {}
            fn prepare_document_load() {}
            fn document_committed() {}
            fn revoke_document_authority() {}
            fn draining_action() {
                self.phase == DocumentPhase::AuthorityRevoked && self.settling_action(attempt);
                self.settling_action == Some(attempt);
                super::semantic_action::map_runtime_fault(*fault) ==
                    zephium_agentic::SemanticActionNativeFailure::AppliedUnverified;
                DocumentPhase::AuthorityRevoked;
                let handed_off = self.awaiting_result
                    && matches!(
                        self.pending,
                        Some(
                            PendingInvocation::Action { .. }
                                | PendingInvocation::Privacy { .. }
                                | PendingInvocation::Media { .. }
                        )
                    );
            }
            fn revoked_settling_action() {}
            fn renderer_lost() {}
            fn cancel() {}
            fn retire() {}
            fn admission_failure(&mut self) -> Option<AgentSemanticRuntimeDispatchError> {
                match self.phase {
                    DocumentPhase::Loading => {
                        return Some(AgentSemanticRuntimeDispatchError::NotReady);
                    }
                    _ => {}
                }
                if self.pending.is_some() || self.awaiting_result {
                    return Some(AgentSemanticRuntimeDispatchError::Busy);
                }
                None
            }
        "#;
        validate_engine_semantic_runtime_boundary(runtime).expect("closed semantic runtime");
        assert!(validate_engine_semantic_runtime_boundary(&format!(
            "{runtime}\nevaluateJavaScript();"
        ))
        .is_err());
        assert!(validate_engine_semantic_runtime_boundary(&format!(
            "{runtime}\nWKContentWorld::pageWorld();"
        ))
        .is_err());
        assert!(validate_engine_semantic_runtime_boundary(
            &runtime.replace("message.frameInfo();", "")
        )
        .is_err());
        assert!(validate_engine_semantic_runtime_boundary(&runtime.replace(
            "return Some(AgentSemanticRuntimeDispatchError::NotReady);",
            "unreachable!();",
        ))
        .is_err());
    }

    #[test]
    fn isolated_command_program_cannot_escape_diagnostic_install_or_attestation_gate() {
        let runtime =
            include_str!("../../crates/zephium-engine/src/platform/macos/semantic_runtime.rs");
        validate_engine_semantic_runtime_boundary(runtime).expect("closed diagnostic gate");
        for call in [
            "#[cfg(feature = \"native-agentic-semantic-probe\")]\n    let source =",
            "#[cfg(feature = \"native-agentic-semantic-probe\")]\n        let source_mismatch =",
            "#[cfg(feature = \"native-agentic-semantic-probe\")]\n#[path = \"agentic_semantic_program_probe.rs\"]",
            "#[cfg(feature = \"native-agentic-semantic-probe\")]\n                let bytes = program_probe::normalize_diagnostic(bytes);",
        ] {
            assert!(runtime.contains(call), "gate witness drifted");
            let unguarded = runtime.replace(
                call,
                &call.replace("#[cfg(feature = \"native-agentic-semantic-probe\")]", ""),
            );
            assert!(validate_engine_semantic_runtime_boundary(&unguarded).is_err());
        }
    }

    #[test]
    fn preparation_only_control_stops_after_revalidation_inside_diagnostic_gate() {
        let candidate = include_str!(
            "../../crates/zephium-engine/src/platform/macos/agentic_isolated_fill_candidate.js"
        );
        let probe = include_str!(
            "../../crates/zephium-engine/src/platform/macos/agentic_semantic_program_probe.rs"
        );
        let shipping = include_str!("../../crates/zephium-agentic/assets/semantic-runtime-v1.js");
        assert!(!shipping.contains("commandPreparationOnly"));
        assert!(!shipping.contains("applied_unverified_preparation_only"));
        let engine = include_str!("../../crates/zephium-engine/src/lib.rs");
        assert!(engine.contains("#[cfg(all(feature = \"native-agentic-semantic-probe\", not(debug_assertions)))]\ncompile_error!(\"the native agentic semantic probe is forbidden in optimized builds\");"));
        assert!(candidate.contains("return commandPreparationOnly ? \"unsupported_interaction\" : runSyntheticFill(target, descriptor, request, finish);"));
        let revalidation = candidate.find("if (!preparedCurrent()) return \"applied_unverified_beforeinput_revalidation\";\n      // The consumed").unwrap();
        let stop = candidate
            .find("if (commandPreparationOnly) return \"applied_unverified_preparation_only\";")
            .unwrap();
        let insertion = candidate
            .find("apply(commandInsertText, document,")
            .unwrap();
        assert!(revalidation < stop && stop < insertion);
        assert!(candidate.contains("const commandPreparationOnly = false;"));
        assert!(probe.contains("|| preparation_only_selected()"));
        assert!(probe.contains(
            "preparation_only_selected() && bytes == b\"E2:applied_unverified_preparation_only\""
        ));
        assert!(probe
            .contains("phase=revalidated command_entered=false result=uncertain content=redacted"));
        assert!(probe.contains("return b\"E2:applied_unverified\";"));
        assert!(probe.contains("source.replacen("));
        assert!(probe.contains("\"const commandPreparationOnly = false;\""));
        assert!(probe.contains("\"const commandPreparationOnly = true;\""));
    }

    #[test]
    fn production_semantic_screenshot_requires_bounded_native_streaming() {
        let screenshot = r#"
            takeSnapshotWithConfiguration_completionHandler();
            objc2::exception::catch();
            configuration.setAfterScreenUpdates(true);
            configuration.setSnapshotWidth(Some(&width));
            CGImageDestinationCreateWithDataConsumer();
            CGDataConsumerCallbacks;
            if end > self.limit {}
            bytes.try_reserve_exact(1);
            budget.max_png_bytes();
            cancelled.load(Ordering::Acquire);
            if completed_at > request.deadline() {}
            SemanticScreenshotPaintEvidence::ExactDocumentContentAvailable;
            CFRelease(self.0.as_ptr());
        "#;
        validate_engine_semantic_screenshot_boundary(screenshot)
            .expect("bounded semantic screenshot");
        assert!(validate_engine_semantic_screenshot_boundary(&format!(
            "{screenshot}\nNSMutableData::new();"
        ))
        .is_err());
        assert!(validate_engine_semantic_screenshot_boundary(
            &screenshot.replace("budget.max_png_bytes();", "")
        )
        .is_err());
    }

    #[test]
    fn production_windows_screenshot_requires_a_capped_com_stream() {
        let module = r#"
            #[cfg(feature = "agentic-browser")]
            #[allow(dead_code)]
            mod semantic_screenshot;
        "#;
        let context = r#"
            pub(crate) fn dispatch_screenshot() {
                if self.semantic().is_none_or(|semantic| semantic.document_content_available_for_audit() != Some(true))
                    || (self.work_navigation.is_none()
                        && attest_hidden_owner(&self.view, self.expected_parent, self.viewport).is_err()) {
                    return Err(SemanticScreenshotNativeFailure::NotReady);
                }
                let Some(semantic) = self.semantic().cloned() else {
                    return Err(SemanticScreenshotNativeFailure::NotReady);
                };
                let native = self.view.webview();
                let capture_cancelled = cancelled.clone();
                let public = request.requires_public_page();
                crate::platform::semantic_pixel_privacy::guarded_model_capture(
                    public,
                    Rc::new(move |completion| {
                        if public {
                            semantic.dispatch_public_privacy(completion)
                        } else {
                            semantic.dispatch_privacy(completion)
                        }
                    }),
                    move |completion| {
                        super::semantic_screenshot::capture_viewport(&native);
                    },
                );
            }
        "#;
        let buffer = r#"
            struct BoundedScreenshotBuffer;
            checked_add(source.len());
            if end > self.limit {}
            try_reserve_exact();
            fn bounded_png_dimensions() {}
            budget.max_png_bytes();
            if header.get(12..16) != Some(b"IHDR".as_slice()) {}
            if pixels > budget.max_pixels() {}
        "#;
        let adapter = r#"
            #[windows_core::implement(IStream)]
            impl ISequentialStream_Impl for BoundedCaptureStream_Impl {}
            impl IStream_Impl for BoundedCaptureStream_Impl {}
            buffer.write(source);
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
            core.CapturePreview();
            request.budget().max_png_bytes();
            cancelled.load(Ordering::Acquire);
            if completed_at > request.deadline() {}
            bounded_png_dimensions(&png, request.budget());
            SemanticScreenshotPaintEvidence::ExactDocumentContentAvailable;
        "#;
        validate_engine_windows_semantic_screenshot(module, context, buffer, adapter)
            .expect("bounded Windows semantic screenshot");
        for (required, replacement) in [
            ("!= Some(true)", "== Some(true)"),
            (
                "self.work_navigation.is_none()",
                "self.work_navigation.is_some()",
            ),
            (".is_err()", ".is_ok()"),
            ("return Err(SemanticScreenshotNativeFailure::NotReady);", ""),
            ("guarded_model_capture(", "unguarded_capture("),
            (
                "semantic.dispatch_privacy(completion)",
                "completion(Ok(()))",
            ),
            (
                "pub(crate) fn dispatch_screenshot()",
                "pub(crate) fn semantic_runtime_ready_for_history()",
            ),
        ] {
            assert!(context.contains(required));
            assert!(validate_engine_windows_semantic_screenshot(
                module,
                &context.replace(required, replacement),
                buffer,
                adapter
            )
            .is_err());
        }
        // A correct check in a different method cannot authorize an unchecked
        // screenshot method, even when both methods appear in the same source.
        let history_only =
            context.replace("dispatch_screenshot", "semantic_runtime_ready_for_history");
        let unchecked = format!("pub(crate) fn dispatch_screenshot() {{ super::semantic_screenshot::capture_viewport(&self.view); }}\n{history_only}");
        assert!(
            validate_engine_windows_semantic_screenshot(module, &unchecked, buffer, adapter)
                .is_err()
        );
        assert!(validate_engine_windows_semantic_screenshot(
            module,
            context,
            buffer,
            &format!("{adapter}\nCreateStreamOnHGlobal();")
        )
        .is_err());
        assert!(validate_engine_windows_semantic_screenshot(
            module,
            context,
            &buffer.replace("try_reserve_exact();", ""),
            adapter,
        )
        .is_err());
    }

    #[test]
    fn production_agent_context_boundary_is_closed_and_probe_independent() {
        let engine = r#"
            #[cfg(feature = "agentic-browser")]
            mod agent_context_port;
            pub fn take_agent_browser_port(&self) {}
            fn shutdown(&self) { self.agent_context_port.seal(); }
        "#;
        let host_root = r#"
            #[cfg(feature = "agentic-browser")]
            mod agent_context;
            #[cfg(all(feature = "agentic-browser", target_os = "windows"))]
            mod agent_cookie_source;
            agent_contexts: HashMap<zephium_agentic::ContextId, agent_context::AgentOwnedContext>,
            agent_cookie_transfers: HashMap<zephium_agentic::ContextCookieTransferId, agent_context::AgentPendingCookieTransfer>,
            agent_cookie_quarantined_profiles: HashSet<ProfileId>,
        "#;
        let port = r#"
            use x::MAX_PENDING_NATIVE_CONTEXT_TASKS;
            use x::MAX_PENDING_SEMANTIC_SCREENSHOTS;
            struct AgentScreenshotPhysicalPermit;
            physical_screenshots: usize,
            fn reserve_screenshot() {}
            fn capture_semantic_screenshot() {}
            ContextOperationKind::Recover;
            ContextOperationKind::Close;
            const fn supports_cookie_transfer() -> bool {
                #[cfg(target_os = "windows")]
                { true }
                #[cfg(not(target_os = "windows"))]
                { false }
            }
            admitted_at: std::time::Instant,
            pub(crate) fn cookie(&self) -> Option<(&ContextCookieTransferRequest, std::time::Instant)> {
                match self.request.as_ref() {
                    Some(AgentPendingRequest::Cookie(request)) => Some((request, self.admitted_at)),
                    _ => None,
                }
            }
            pub(crate) struct AgentContextPortSlot;
            catch_unwind();
            fn emit_renderer_lost() {}
            fn closed() { ContextDispatch::Unsupported; }
        "#;
        let host = r#"
            profile_lease: ContextProfileLease,
            install_content_policy_on_view(view.view(), &content_policy);
            NativeResourceClass::AgentContext;
            ContextConstructionProof::MacOsOwnedSelectedProfileExtensionFree;
            pending_navigation: Option<AgentPendingNavigation>,
            pending_recovery: Option<AgentPendingRecovery>,
            pending_screenshot: Option<AgentPendingScreenshot>,
            pending_captures,
            renderer_loss_rejoin_pending: bool,
            binding.view.attest();
            binding.view.prepare_semantic_document_load();
            fn start_owned_agent_semantic_invocation() {}
            fn start_owned_agent_screenshot() {}
            binding.semantic_snapshot_generation != Some(snapshot_generation);
            binding.view.dispatch_screenshot();
            SemanticRuntimeSettlement::try_new();
            double_full_successor(binding.join, requested);
            binding.view.view().reload();
            binding.view.view().load_url("about:blank");
            const AGENT_PAGE_LOAD_COMMIT_TIMEOUT: Duration = Duration::from_secs(30);
            try_with_agent_context_terminal();
            emitter.emit_renderer_lost(prior);
            fn force_shutdown_agent_contexts() {}
        "#;
        let macos = r#"
            with_visible(false).with_focused(false);
            configuration.webExtensionController();
            semantic.attest_configuration(&configuration);
            WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm);
            Retained::as_ptr(&actual_store) == Retained::as_ptr(expected);
            semantic: Option<AgentSemanticRuntimeRegistration>,
            pub(crate) fn dispatch_screenshot() {}
            use crate::platform::agent_navigation::AgentNavigationController;
            with_on_web_content_process_terminate_handler();
            renderer_lost_callback();
        "#;
        let navigation = r#"
            struct AgentNavigationController;
            state.bootstrap_available = false;
            native_id: Option<wry::NavigationId>;
            armed.native_id != Some(event.id);
            terminal_claimed.compare_exchange();
            fn document_finished_for_audit() {}
            fn claim_renderer_loss() {}
            fn arm_recovery() {}
            fn settle_recovery() {}
        "#;
        validate_engine_agent_context_boundary(engine, host_root, port, host, macos, navigation)
            .expect("closed production adapter");
        assert!(validate_engine_agent_context_boundary(
            engine,
            host_root,
            &format!("{port}\nevaluate_script();"),
            host,
            macos,
            navigation,
        )
        .is_err());
        assert!(validate_engine_agent_context_boundary(
            engine,
            host_root,
            &port.replace("admitted_at: std::time::Instant,", ""),
            host,
            macos,
            navigation,
        )
        .is_err());
        assert!(validate_engine_agent_context_boundary(
            engine,
            host_root,
            &port.replace(
                "#[cfg(target_os = \"windows\")]\n                { true }",
                ""
            ),
            host,
            macos,
            navigation,
        )
        .is_err());
        assert!(validate_engine_agent_context_boundary(
            engine,
            host_root,
            port,
            host,
            &macos.replace("semantic.attest_configuration(&configuration);", ""),
            navigation,
        )
        .is_err());
    }

    #[test]
    fn production_agent_context_dispatch_is_panic_contained_and_fail_stopped() {
        let port = include_str!("../../crates/zephium-engine/src/agent_context_port.rs");
        validate_engine_agent_context_panic_boundary(port).expect("agent-context panic boundary");

        for invalid in [
            port.replacen(
                "let accepted = contain_agent_port_panic",
                "let accepted = uncontained_dispatch",
                1,
            ),
            port.replacen(
                "let _ = contain_agent_port_panic(&callback_admission",
                "let _ = uncontained_callback(&callback_admission",
                1,
            ),
            port.replacen(
                "            admission.fail_invariant();\n            None",
                "            None",
                1,
            ),
            port.replacen(
                "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))",
                "match uncontained_operation(operation)",
                1,
            ),
            port.replacen(
                "                state.sealed = true;\n                if let Some(admission) = &state.admission {\n                    admission.seal();\n                }\n                if let Some(factory) = &state.factory {\n                    factory.seal();\n                }\n                drop(state);\n                self.report_fatal_once();",
                "                drop(state);\n                self.report_fatal_once();",
                1,
            ),
            port.replacen(
                "            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {\n                (self.fatal)(\"agent-context port slot invariant failed\");\n            }));",
                "            (self.fatal)(\"agent-context port slot invariant failed\");",
                1,
            ),
        ] {
            assert!(validate_engine_agent_context_panic_boundary(&invalid).is_err());
        }
    }

    #[test]
    fn production_location_observation_is_native_bounded_and_generation_exact() {
        let port = include_str!("../../crates/zephium-engine/src/agent_context_port.rs");
        let host = include_str!("../../crates/zephium-engine/src/host/agent_context.rs");
        let macos = include_str!("../../crates/zephium-engine/src/platform/macos/agent_context.rs");
        let windows =
            include_str!("../../crates/zephium-engine/src/platform/windows/agent_context.rs");
        let navigation =
            include_str!("../../crates/zephium-engine/src/platform/agent_navigation.rs");
        validate_engine_agent_location_observation(port, host, macos, windows, navigation)
            .expect("repository location observation boundary");

        let unbounded = navigation.replace("location_dirty: bool", "location_queue: VecDeque<()>");
        assert!(
            validate_engine_agent_location_observation(port, host, macos, windows, &unbounded)
                .is_err()
        );

        let page_world = format!("{macos}\nfn widened() {{ evaluate_script(\"location.href\"); }}");
        assert!(validate_engine_agent_location_observation(
            port,
            host,
            &page_world,
            windows,
            navigation,
        )
        .is_err());
    }

    #[test]
    fn production_agent_redirects_are_scoped_bounded_and_identity_exact() {
        let domain = include_str!("../../crates/zephium-agentic/src/context_port.rs");
        let host = include_str!("../../crates/zephium-engine/src/host/agent_context.rs");
        let navigation =
            include_str!("../../crates/zephium-engine/src/platform/agent_navigation.rs");
        let fixture = include_str!("../../crates/zephium-agentic/src/fixture_server.rs");
        validate_engine_agent_redirect_contract(domain, host, navigation, fixture)
            .expect("repository redirect contract");

        assert!(validate_engine_agent_redirect_contract(
            &domain.replace(
                "allowed_origins: Vec<SemanticOrigin>",
                "allowed_origins: Vec<String>"
            ),
            host,
            navigation,
            fixture,
        )
        .is_err());
        assert!(validate_engine_agent_redirect_contract(
            domain,
            host,
            &navigation.replace(
                "#[cfg(all(\n        feature = \"native-agentic-semantic-probe\",\n        any(test, target_os = \"windows\")\n    ))]\n    pub(crate) fn redirect_probe_audit",
                "pub(crate) fn redirect_probe_audit",
            ),
            fixture,
        )
        .is_err());
        assert!(validate_engine_agent_redirect_contract(
            &domain.replace(
                "MAX_CONTEXT_NAVIGATION_REDIRECTS: usize = 8",
                "MAX_CONTEXT_NAVIGATION_REDIRECTS: usize = 9",
            ),
            host,
            navigation,
            fixture,
        )
        .is_err());
        assert!(validate_engine_agent_redirect_contract(
            domain,
            host,
            &navigation.replace(
                "armed.native_id != Some(event.id)",
                "armed.native_id == Some(event.id)",
            ),
            fixture,
        )
        .is_err());
        assert!(validate_engine_agent_redirect_contract(
            domain,
            &host.replace("pending.accepts_committed_target(&committed)", "true",),
            navigation,
            fixture,
        )
        .is_err());
        assert!(validate_engine_agent_redirect_contract(
            domain,
            host,
            navigation,
            &fixture.replace("destination: FixtureRoute", "destination: &str"),
        )
        .is_err());
    }

    #[test]
    fn windows_production_agent_context_boundary_is_closed_and_bounded() {
        let module = include_str!("../../crates/zephium-engine/src/platform/windows/mod.rs");
        let platform_module = include_str!("../../crates/zephium-engine/src/platform/mod.rs");
        let adapter =
            include_str!("../../crates/zephium-engine/src/platform/windows/agent_context.rs");
        let suspension =
            include_str!("../../crates/zephium-engine/src/platform/agent_suspension.rs");
        let timeout = include_str!("../../crates/zephium-engine/src/platform/windows/timeout.rs");
        let navigation =
            include_str!("../../crates/zephium-engine/src/platform/agent_navigation.rs");
        let host = include_str!("../../crates/zephium-engine/src/host/agent_context.rs");
        let content_rules = include_str!("../../crates/zephium-engine/src/host/content_rules.rs");
        let port = include_str!("../../crates/zephium-engine/src/agent_context_port.rs");
        let host_root = include_str!("../../crates/zephium-engine/src/host/mod.rs");
        let cookie_source =
            include_str!("../../crates/zephium-engine/src/host/agent_cookie_source.rs");
        validate_engine_windows_agent_context_boundary(module, adapter, timeout, navigation, host)
            .expect("closed Windows production owner");
        for mutation in [
            adapter.replace(
                "if !super::extensions::is_runtime_component(&id)",
                "if false",
            ),
            adapter.replace(
                "if !matches!(&profile, AgentOwnedProfile::Automation { .. })",
                "if false",
            ),
            adapter.replace(
                "with_controller_initialization_hook",
                "with_browser_extension_startup_gate",
            ),
            adapter.replace(
                "|_, core| crate::reputation::configure(core)",
                "|_, _| Ok(())",
            ),
            format!("{adapter}\nsuper::extensions::add();"),
            format!("{adapter}\nwith_browser_extension_startup_gate();"),
        ] {
            assert!(validate_engine_windows_agent_context_boundary(
                module, &mutation, timeout, navigation, host,
            )
            .is_err());
        }
        validate_engine_windows_agent_suspension_boundary(
            platform_module,
            adapter,
            suspension,
            host,
            content_rules,
            port,
        )
        .expect("closed Windows suspension owner");
        validate_engine_windows_agent_cookie_source(host_root, cookie_source)
            .expect("closed ordinary-profile cookie source");
        assert!(validate_engine_windows_agent_context_boundary(
            module,
            &format!("{adapter}\nevaluate_script();"),
            timeout,
            navigation,
            host,
        )
        .is_err());
        assert!(validate_engine_windows_agent_cookie_source(
            host_root,
            &cookie_source.replace(
                "partition.profile() != profile",
                "partition.profile() == profile",
            ),
        )
        .is_err());
        assert!(validate_engine_windows_agent_cookie_source(
            host_root,
            &format!("{cookie_source}\nGetCookies();"),
        )
        .is_err());
        assert!(validate_engine_windows_agent_context_boundary(
            module,
            &adapter.replace(
                "if !matches!(self.profile, AgentOwnedProfile::Automation { .. })",
                "if false",
            ),
            timeout,
            navigation,
            host,
        )
        .is_err());
        assert!(validate_engine_windows_agent_context_boundary(
            module,
            &adapter.replace(
                "super::same_environment(&self.view.environment(), expected_environment)",
                "true",
            ),
            timeout,
            navigation,
            host,
        )
        .is_err());
        assert!(validate_engine_windows_agent_context_boundary(
            module,
            adapter,
            timeout,
            navigation,
            &host.replace("binding.cookie_contaminated = true;", ""),
        )
        .is_err());
        assert!(validate_engine_windows_agent_context_boundary(
            module,
            adapter,
            timeout,
            navigation,
            &host.replace(
                "std::mem::take(&mut self.agent_cookie_transfers)",
                "std::mem::take(&mut self.agent_contexts)",
            ),
        )
        .is_err());
        assert!(validate_engine_windows_agent_context_boundary(
            module,
            adapter,
            &timeout.replace("MAX_PENDING_NATIVE_CONTEXT_TASKS", "usize::MAX"),
            navigation,
            host,
        )
        .is_err());
        assert!(validate_engine_windows_agent_suspension_boundary(
            platform_module,
            adapter,
            &suspension.replace("LATE_NATIVE_COMPLETED", "LATE_NATIVE_DROPPED"),
            host,
            content_rules,
            port,
        )
        .is_err());
        assert!(validate_engine_windows_agent_suspension_boundary(
            platform_module,
            adapter,
            suspension,
            &host.replace("pending.claim.retire();", "drop(pending.claim);"),
            content_rules,
            port,
        )
        .is_err());
    }

    #[test]
    fn windows_production_semantic_protocol_is_closed_and_bounded() {
        let module = include_str!("../../crates/zephium-engine/src/platform/mod.rs");
        let source =
            include_str!("../../crates/zephium-engine/src/platform/agent_semantic_cdp_protocol.rs");
        validate_engine_windows_semantic_protocol(module, source)
            .expect("closed Windows semantic protocol");
        assert!(validate_engine_windows_semantic_protocol(
            module,
            &format!("const ESCAPE: &str = \"Input.dispatchArbitraryEvent\";\n{source}")
        )
        .is_err());
        assert!(validate_engine_windows_semantic_protocol(
            module,
            &format!("{source}\nRuntime.evaluate();"),
        )
        .is_err());
        assert!(validate_engine_windows_semantic_protocol(
            module,
            &source.replace("\"userGesture\": false", "\"userGesture\": true"),
        )
        .is_err());
        assert!(validate_engine_windows_semantic_protocol(
            module,
            &source.replace(
                "empty_control_command(FixedSemanticCdpMethod::RuntimeEnable)",
                "command(FixedSemanticCdpMethod::RuntimeEnable, serde_json::json!({}), MAX_CONTROL_PARAMETERS_BYTES, MAX_CONTROL_RESPONSE_BYTES).expect(\"fixed\")",
            ),
        )
        .is_err());
    }

    #[test]
    fn windows_production_semantic_runtime_is_serialized_and_non_universal() {
        let module = include_str!("../../crates/zephium-engine/src/platform/windows/mod.rs");
        let source =
            include_str!("../../crates/zephium-engine/src/platform/windows/semantic_runtime.rs");
        validate_engine_windows_semantic_runtime(module, source)
            .expect("bounded Windows semantic runtime");
        assert!(validate_engine_windows_semantic_runtime(
            module,
            &format!("{source}\nRuntime.evaluate();"),
        )
        .is_err());
        assert!(validate_engine_windows_semantic_runtime(
            module,
            &source.replace("MAX_CLEANUP_DISABLE_ATTEMPTS: u8 = 1", "usize::MAX"),
        )
        .is_err());
    }

    #[test]
    fn windows_semantic_probe_is_fixed_hidden_and_release_excluded() {
        let module = include_str!("../../crates/zephium-engine/src/platform/windows/mod.rs");
        let source = include_str!(
            "../../crates/zephium-engine/src/platform/windows/agentic_semantic_probe.rs"
        );
        let binary =
            include_str!("../../crates/zephium-engine/src/bin/windows_agentic_semantic_probe.rs");
        let fixture = include_str!("../../crates/zephium-agentic/src/fixture_server.rs");
        validate_windows_semantic_probe(module, source, binary, fixture)
            .expect("closed Windows semantic qualifier");
        assert!(validate_windows_semantic_probe(
            module,
            &source.replacen("#![deny(unsafe_op_in_unsafe_fn)]\n", "", 1),
            binary,
            fixture,
        )
        .is_err());
        assert!(validate_windows_semantic_probe(
            module,
            &source.replacen("#![deny(clippy::undocumented_unsafe_blocks)]\n", "", 1),
            binary,
            fixture,
        )
        .is_err());
        assert!(validate_windows_semantic_probe(
            module,
            &source.replace(
                "deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
                "allow(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
            ),
            binary,
            fixture,
        )
        .is_err());
        assert!(validate_windows_semantic_probe(
            module,
            source,
            &binary.replace(
                "deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
                "allow(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
            ),
            fixture,
        )
        .is_err());
        assert!(validate_windows_semantic_probe(
            module,
            &format!("{source}\nSetForegroundWindow(host);"),
            binary,
            fixture,
        )
        .is_err());
        assert!(validate_windows_semantic_probe(
            module,
            source,
            &binary.replace("persist_noclobber", "persist"),
            fixture,
        )
        .is_err());
        assert!(validate_windows_semantic_probe(
            module,
            &source.replace("view.attest_suspension_state() == Ok(true)", "false"),
            binary,
            fixture,
        )
        .is_err());
        for (retained_join, substitution) in [
            (
                "facts.page_world_bridge_absent = true;",
                "facts.page_world_bridge_absent = false;",
            ),
            (
                "verify_replacement_snapshot(&snapshot)?;",
                "let _ = &snapshot;",
            ),
            (
                "outcome != Err(SemanticRuntimePortFailure::Transport)",
                "false",
            ),
            (
                "facts.renderer_lost_refused = true;",
                "facts.renderer_lost_refused = false;",
            ),
            (
                "facts.suspended_state_attested = true;",
                "facts.suspended_state_attested = false;",
            ),
            (
                "facts.same_document_replacement_observed = true;",
                "facts.same_document_replacement_observed = false;",
            ),
            (
                "facts.same_document_replacement_rejoined = true;",
                "facts.same_document_replacement_rejoined = false;",
            ),
            (
                "facts.stale_location_join_refused = true;",
                "facts.stale_location_join_refused = false;",
            ),
            (
                "facts.post_location_snapshot_verified = true;",
                "facts.post_location_snapshot_verified = false;",
            ),
            (
                "facts.resources_before = Some(sample_resources(",
                "facts.resources_before = None; ignored_sample_resources(",
            ),
            (
                "facts.resources_after = Some(sample_resources(",
                "facts.resources_after = None; ignored_sample_resources(",
            ),
            (
                "webview2_processes: resources_before.processes",
                "webview2_processes: 1",
            ),
            (
                "resident_bytes: resources_after.resident_bytes",
                "resident_bytes: 1",
            ),
            (
                "registry\n        .observe_navigation_replacement(context_id, prior)",
                "registry\n        .join(context_id)",
            ),
            (
                "verify_location_after_snapshot(&after)?;",
                "let _ = &after;",
            ),
            (
                "move || location_callbacks.request_location_check(),",
                "|| {},",
            ),
            (
                "    view.navigation()\n        .finish_location_check(false)\n        .map_err(|_| ProbeError::verify(WindowsSemanticProbeStage::Navigate))",
                "    Ok(())",
            ),
            (
                "semantic_work_drained: facts.semantic_work_drained",
                "semantic_work_drained: true",
            ),
            (
                "runtime_retired: teardown.runtime_retired",
                "runtime_retired: true",
            ),
        ] {
            assert!(
                validate_windows_semantic_probe(
                    module,
                    &source.replace(retained_join, substitution),
                    binary,
                    fixture,
                )
                .is_err(),
                "producer substitution must fail: {retained_join}",
            );
        }
    }

    #[test]
    fn production_owned_context_viewport_is_fixed_and_natively_attested() {
        let context_port = include_str!("../../crates/zephium-agentic/src/context_port.rs");
        let host = include_str!("../../crates/zephium-engine/src/host/agent_context.rs");
        let macos = include_str!("../../crates/zephium-engine/src/platform/macos/agent_context.rs");
        let windows =
            include_str!("../../crates/zephium-engine/src/platform/windows/agent_context.rs");
        validate_owned_context_viewport_contract(context_port, host, macos, windows)
            .expect("fixed native viewport");
        assert!(validate_owned_context_viewport_contract(
            &context_port.replace("width: 1_280", "width: 1"),
            host,
            macos,
            windows,
        )
        .is_err());
        assert!(validate_owned_context_viewport_contract(
            context_port,
            host,
            &macos.replace(
                "native_view.setAutoresizingMask(Mask::ViewNotSizable);",
                "native_view.setAutoresizingMask(Mask::ViewWidthSizable);",
            ),
            windows,
        )
        .is_err());
        for (needle, replacement) in [
            (
                ".with_background_throttling(wry::BackgroundThrottlingPolicy::Throttle)",
                ".with_background_throttling(wry::BackgroundThrottlingPolicy::Disabled)",
            ),
            (
                "scheduling != objc2_web_kit::WKInactiveSchedulingPolicy::Throttle",
                "false",
            ),
        ] {
            assert!(validate_owned_context_viewport_contract(
                context_port,
                host,
                &macos.replace(needle, replacement),
                windows,
            )
            .is_err());
        }
        assert!(validate_owned_context_viewport_contract(
            context_port,
            host,
            macos,
            &windows.replace("controller.Bounds(&mut controller_bounds)", "Ok(())"),
        )
        .is_err());
    }

    #[test]
    fn macos_probe_requires_isolated_runtime_and_preflighted_native_dispatch() {
        let valid =
            include_str!("../../crates/zephium-engine/src/platform/macos/agentic_input_probe.rs");
        validate_macos_probe_source(valid).expect("valid bounded macOS probe");
        assert!(validate_macos_probe_source(&valid.replacen(
            "#![deny(unsafe_op_in_unsafe_fn)]\n",
            "",
            1,
        ))
        .is_err());
        assert!(validate_macos_probe_source(&valid.replacen(
            "#![deny(clippy::undocumented_unsafe_blocks)]\n",
            "",
            1,
        ))
        .is_err());
        assert!(validate_macos_probe_source(&valid.replacen(
            "super::new_ephemeral_data_store()",
            "super::new_data_store()",
            1,
        ))
        .is_err());
        assert!(
            validate_macos_probe_source(&valid.replacen("frame.isMainFrame()", "true", 1,))
                .is_err()
        );
        assert!(validate_macos_probe_source(&valid.replacen(
            "    control.check()?;\n    window.sendEvent(&event);",
            "    window.sendEvent(&event);",
            1,
        ))
        .is_err());
        assert!(validate_macos_probe_source(&valid.replacen(
            "    control.check()?;\n    webview\n        .load_url(&url)",
            "    webview\n        .load_url(&url)",
            1,
        ))
        .is_err());
        assert!(validate_macos_probe_source(&valid.replacen(
            "            control.check()?;\n            webview\n                .set_visible(false)",
            "            webview\n                .set_visible(false)",
            1,
        ))
        .is_err());
        assert!(validate_macos_probe_source(&valid.replacen(
            "    control.check()?;\n    let Some(element) = page.accessibilityHitTest(screen) else",
            "    let Some(element) = page.accessibilityHitTest(screen) else",
            1,
        ))
        .is_err());
        assert!(validate_macos_probe_source(&valid.replacen(
            "    control.check()?;\n    let pressed: bool = {",
            "    let pressed: bool = {",
            1,
        ))
        .is_err());
        assert!(validate_macos_probe_source(&format!(
            "{valid}\nfn global_input() {{ CGEventPost(); }}"
        ))
        .is_err());
        assert!(validate_macos_probe_source(&format!(
            "{valid}\nfn prompt() {{ AXIsProcessTrustedWithOptions(); }}"
        ))
        .is_err());
        assert!(validate_macos_probe_source(&format!(
            "{valid}\nfn widened(page: &Page) {{ page.evaluateJavaScript(\"x\"); }}"
        ))
        .is_err());
    }

    #[test]
    fn windows_probe_requires_closed_scoped_input_and_bounded_cdp() {
        let valid =
            include_str!("../../crates/zephium-engine/src/platform/windows/agentic_input_probe.rs");
        validate_windows_probe_source(valid).expect("valid bounded Windows probe");
        assert!(validate_windows_probe_source(&valid.replacen(
            "#![deny(unsafe_op_in_unsafe_fn)]\n",
            "",
            1
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "#![deny(clippy::undocumented_unsafe_blocks)]\n",
            "",
            1
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replace(
            "deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
            "allow(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "        SendMessageTimeoutW(\n",
            "        SendInput(\n",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replace(
            "owner_thread_id == current_thread_id",
            "owner_thread_id == 0",
        ))
        .is_err());
        assert!(validate_windows_probe_source(
            &valid.replace("MapVirtualKeyExW(", "MapVirtualKeyW(")
        )
        .is_err());
        assert!(validate_windows_probe_source(
            &valid.replace("keyboard_layout == self.keyboard_layout", "true",)
        )
        .is_err());
        assert!(validate_windows_probe_source(&valid.replace(
            "let timeout_ms = message_timeout_ms(deadline)?;",
            "let timeout_ms = 250;",
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "        validate_cdp_response(&response)?;\n        observe_focus()?;",
            "        observe_focus()?;",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "                FixedProbeScript::DomRecipe(case),\n                permit,\n                poll_control,\n                Some(&mut *observe_focus),",
            "                FixedProbeScript::DomRecipe(case),\n                permit,\n                poll_control,\n                None,",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "            &parameters.to_string(),\n            permit,\n            poll_control,\n            Some(&mut *observe_focus),",
            "            &parameters.to_string(),\n            permit,\n            poll_control,\n            None,",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "borrowed_pcwstr_bounded(",
            "unbounded_pcwstr(",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&format!(
            "{valid} fn decode(value: Result<char, ()>) {{ let _ = value.unwrap_or(char::REPLACEMENT_CHARACTER); }}"
        ))
        .is_err());
        assert!(validate_windows_probe_source(&format!(
            "{valid} fn attach() {{ AttachThreadInput(); }}"
        ))
        .is_err());
        assert!(validate_windows_probe_source(&format!(
            "{valid} fn bypass() {{ PostMessageW(); }}"
        ))
        .is_err());
        assert!(validate_windows_probe_source(&format!(
            "{valid} fn capture() {{ SetCapture(); }}"
        ))
        .is_err());
        assert!(validate_windows_probe_source(&format!(
            "{valid} fn composition() {{ SendMouseInput(); }}"
        ))
        .is_err());
        assert!(validate_windows_probe_source(
            &valid.replace("focus_is_owned_by_view(view, self.document_focus)", "false",)
        )
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "        GetGUIThreadInfo(target.owner_thread_id, &mut document_gui)\n            .map_err(|_| AdapterError::FocusPolicy)?;",
            "        let _ = target.owner_thread_id;",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "    let parameters = HSTRING::from(parameters);\n    check_dispatch_control(permit, poll_control, deadline)?;",
            "let parameters = HSTRING::from(parameters);",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "    check_dispatch_control(permit, &mut poll_control, run_deadline)\n        .map_err(|error| adapter_failure(error, ProbeStage::Admit, None, None))?;\n",
            "",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "                check_dispatch_control(permit, &mut poll_control, run_deadline).map_err(\n                    |error| adapter_failure(error, ProbeStage::Navigate, Some(case), Some(backend)),\n                )?;\n",
            "",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "    check_dispatch_control(permit, poll_control, deadline)?;\n    observe_focus()?;\n    if !target.is_current() {\n        return Err(AdapterError::NativeConstruction);\n    }\n    let timeout_ms = message_timeout_ms(deadline)?;",
            "    let timeout_ms = message_timeout_ms(deadline)?;\n    check_dispatch_control(permit, poll_control, deadline)?;\n    observe_focus()?;\n    if !target.is_current() {\n        return Err(AdapterError::NativeConstruction);\n    }",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "            check_dispatch_control(permit, poll_control, deadline)?;\n            view.set_visible(false)",
            "            view.set_visible(false)",
            1,
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replace(
            "super::agentic_probe_resources::sample_webview2_resources(",
            "super::agentic_probe_resources::bypass_resource_sample(",
        ))
        .is_err());
        assert!(validate_windows_probe_source(&valid.replacen(
            "        resources_before: unavailable_resource_sample(),",
            "        resources_before: ResourceEvidence { native_views: 1, queued_actions: 0, helper_processes: None, resident_bytes: None },",
            1,
        ))
        .is_err());
    }

    #[test]
    fn windows_probe_resource_sampler_is_shared_bounded_and_release_excluded() {
        let module = include_str!("../../crates/zephium-engine/src/platform/windows/mod.rs");
        let valid = include_str!(
            "../../crates/zephium-engine/src/platform/windows/agentic_probe_resources.rs"
        )
        .replace("\r\n", "\n");
        let valid = valid.as_str();
        validate_windows_probe_resource_sampler(module, valid).expect("bounded shared sampler");
        assert!(validate_windows_probe_resource_sampler(
            &module.replace(
                "feature = \"native-agentic-semantic-probe\"",
                "feature = \"agentic-browser\"",
            ),
            valid,
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replacen("#![deny(clippy::undocumented_unsafe_blocks)]\n", "", 1),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replacen(
                "#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]\n",
                "",
                1,
            ),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replace(
                "deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
                "allow(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replace(
                "OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id)",
                "OpenProcess(PROCESS_ALL_ACCESS, 0, process_id)",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replace(
                "resident_bytes.checked_add(process_resident_bytes)",
                "Some(process_resident_bytes)"
            ),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replacen(
                "    let Some((rejoined_process_cohort, rejoined_count, rejoined_helper_processes)) =\n        webview2_process_cohort(&environment, check_control, failure)?\n    else {\n        return Ok(None);\n    };",
                "    let (rejoined_process_cohort, rejoined_count, rejoined_helper_processes) = (process_cohort, process_count, helper_processes);",
                1,
            ),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replacen(
                "        let opened_handle = ProbeProcessHandle::open(process_id, failure);\n        check_control()?;\n        let Some(handle) = opened_handle else {",
                "        let opened_handle = ProbeProcessHandle::open(process_id, failure);\n        let Some(handle) = opened_handle else {",
                1,
            ),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replacen(
                "    let processes = unsafe { environment.GetProcessInfos() };\n    check_control()?;\n    let Ok(processes) = processes else {",
                "    let processes = unsafe { environment.GetProcessInfos() };\n    let Ok(processes) = processes else {",
                1,
            ),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replace("unsafe { CloseHandle(handle) != 0 }", "true",),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replace(
                "let kind_result = unsafe { process.Kind(&mut kind) };",
                "let kind_result = Ok(());",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replace("if browser_processes != 1", "if false"),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &valid.replace("*slot = (process_id, kind.0);", "*slot = (process_id, 0);",),
        )
        .is_err());
        assert!(validate_windows_probe_resource_sampler(
            module,
            &format!("{valid}\nCreateToolhelp32Snapshot();"),
        )
        .is_err());
    }

    #[test]
    fn windows_probe_ci_requires_powershell_parse_native_clippy_and_link_gates() {
        let valid = r#"
            - name: Parse the Windows agentic qualification workflow
              if: runner.os == 'Windows'
              shell: pwsh
              run: |
                $tokens = $null
                $parseErrors = $null
                $scriptPath = Join-Path $env:GITHUB_WORKSPACE "scripts\qualification\windows-agentic.ps1"
                [void][System.Management.Automation.Language.Parser]::ParseFile(
                  $scriptPath,
                  [ref]$tokens,
                  [ref]$parseErrors
                )
                if ($parseErrors.Count -ne 0) {
                  throw "physical Windows agentic orchestrator did not parse"
                }
            cargo clippy --locked -p zephium-engine \
              --features native-agentic-input-probe \
              --bin windows-agentic-input-probe
            cargo build --locked -p zephium-engine \
              --features native-agentic-input-probe \
              --bin windows-agentic-input-probe
            cargo clippy --locked -p zephium-engine \
              --features native-agentic-semantic-probe \
              --bin windows-agentic-semantic-probe
            cargo build --locked -p zephium-engine \
              --features native-agentic-semantic-probe \
              --bin windows-agentic-semantic-probe
        "#;
        validate_windows_probe_ci(valid).expect("native audit and link gates");
        assert!(validate_windows_probe_ci(&valid.replacen("ParseFile", "Tokenize", 1)).is_err());
        assert!(validate_windows_probe_ci(&valid.replacen("clippy", "check", 1)).is_err());
        assert!(validate_windows_probe_ci(&valid.replacen("build", "check", 1)).is_err());
        assert!(validate_windows_probe_ci(&valid.replacen(
            "native-agentic-semantic-probe",
            "missing-semantic-clippy-probe",
            1,
        ))
        .is_err());
    }

    #[test]
    fn windows_physical_workflow_is_exact_nonfocused_and_source_bound() {
        let source = include_str!("../../scripts/qualification/windows-agentic.ps1");
        validate_windows_physical_workflow(source).expect("closed physical workflow");
        assert!(validate_windows_physical_workflow(&source.replacen(
            "[switch] $AuthorizedPhysicalWindows",
            "[switch] $UncheckedPhysicalWindows",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "& cargo --offline @Arguments",
            "& cargo @Arguments",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "status --porcelain=v1 --untracked-files=all",
            "status --porcelain=v1 --untracked-files=no",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "[System.IO.FileMode]::CreateNew",
            "[System.IO.FileMode]::OpenOrCreate",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "Get-FileHash -LiteralPath $Path -Algorithm SHA256",
            "Get-Item -LiteralPath $Path",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "if ($currentDebuggerBinaryHash -cne $recordedDebuggerBinaryHash)",
            "if ($currentDebuggerBinaryHash -ceq $recordedDebuggerBinaryHash)",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "if ($semanticProbeBinaryHash -cne $preflightSemanticProbeBinaryHash)",
            "if ($semanticProbeBinaryHash -ceq $preflightSemanticProbeBinaryHash)",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "Get-CargoTargetDirectory -Repository $repository",
            "Join-Path $repository target",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "--ci-hidden-event-flood",
            "--ci-hidden-fixed-documents",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&format!(
            "{source}\n--visible-focused-windows-all --allow-visible-focused"
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "Assert-ExactDirectoryEntries -Directory $evidenceDirectory -ExpectedNames @()",
            "Write-Host unchecked",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "Invoke-Cargo -Arguments @(\"xtask\", \"check-agentic-probe-boundary\")",
            "Write-Host unchecked",
            1,
        ))
        .is_err());
        assert!(validate_windows_physical_workflow(&source.replacen(
            "        Assert-CleanCheckout -Repository $repository\n        $preflightRevision",
            "        $preflightRevision",
            1,
        ))
        .is_err());
        let stamped_before_preflight = source
            .replacen(
                "        Write-SourceStamp -Path (Join-Path $evidenceDirectory $sourceStampName) -Revision $sourceRevision",
                "        Write-Host stamp-moved",
                1,
            )
            .replace(
                "        Invoke-Cargo -Arguments @(\"xtask\", \"check-agentic-probe-boundary\")",
                "        Write-SourceStamp -Path (Join-Path $evidenceDirectory $sourceStampName) -Revision $sourceRevision\n        Invoke-Cargo -Arguments @(\"xtask\", \"check-agentic-probe-boundary\")",
            );
        assert!(validate_windows_physical_workflow(&stamped_before_preflight).is_err());
    }

    #[test]
    fn windows_evidence_paths_reject_every_reparse_point_class() {
        let source = include_str!("../../crates/zephium-agentic/src/probe_evidence_path.rs");
        validate_probe_evidence_path(source).expect("direct Windows evidence paths");
        assert!(
            validate_probe_evidence_path(&source.replace("0x0000_0400", "0x0000_0000")).is_err()
        );
        assert!(validate_probe_evidence_path(&source.replace(
            "windows_attributes_have_no_reparse_point(metadata.file_attributes())",
            "true",
        ))
        .is_err());
        assert!(validate_probe_evidence_path(
            &source.replace("&& metadata_has_no_windows_reparse_point(metadata)", "",)
        )
        .is_err());
    }

    #[test]
    fn windows_physical_runner_retains_exact_modes_and_jsonl_evidence() {
        let binary = r#"
            #![cfg_attr(
                not(test),
                deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
            )]
            WindowsProbeMode::from_argument;
            "--evidence-directory";
            "eval/agentic-browsing/local-results";
            mode.local_result_filename();
            evidence_metadata_is_direct_directory(&metadata);
            NamedTempFile::new_in(directory);
            pending.file.as_file().sync_all();
            persist_noclobber(pending.destination);
            qualify_windows_probe_evidence(mode, &evidence);
            "--allow-visible-focused";
            encode_response_line(&response);
            ProbeReply::RunCompleted(evidence);
        "#;
        let qualification = r#"
            "--ci-hidden-fixed-dom";
            "--ci-hidden-hwnd";
            "--ci-hidden-cdp";
            "--visible-background-windows-all";
            "--visible-focused-windows-all";
            evidence.runtime.adapter_revision.as_str() != "native-input-m1-resources-v4";
            focus_sequence_matches_presentation(actual.focus, mode.presentation());
            focus.before == baseline && focus.during == baseline && focus.after == after;
            actual.focus.target_received_dom_focus != target_focus_event_observed(actual);
            if !no_dispatch_evidence_is_empty(actual) {}
            else if !actual.target.target_verified
                || !has_qualifying_event(actual)
                || (backend_requires_trusted_qualifying_event(backend)
                    && !has_trusted_qualifying_event(actual)) {}
            fn has_qualifying_event(evidence: &CaseEvidence) -> bool {
                let required = qualifying_event_kind(evidence.case);
                evidence.events.iter().any(|event|
                    event.kind == required && event.target == evidence.target.intended)
            }
            fn has_trusted_qualifying_event(evidence: &CaseEvidence) -> bool {
                let required = qualifying_event_kind(evidence.case);
                evidence.events.iter().any(|event| {
                    event.is_trusted
                        && event.kind == required
                        && event.target == evidence.target.intended
                })
            }
            fn target_focus_event_observed(evidence: &CaseEvidence) -> bool {
                evidence.events.iter().any(|event| {
                    event.kind == InputEventKind::Focus
                        && event.target == evidence.target.intended
                })
            }
            const fn backend_requires_trusted_qualifying_event(backend: InputBackend) -> bool {
                matches!(backend, InputBackend::WindowsHwndInput | InputBackend::WindowsCdpInput)
            }
            actual.resources_before.native_views != 1;
            actual.resources_after.native_views != 1;
            actual.resources_before.queued_actions != 0;
            actual.resources_after.queued_actions != 0;
            actual.resources_before.helper_processes.is_none_or(|count| count == 0);
            actual.resources_after.helper_processes.is_none_or(|count| count == 0);
            actual.resources_before.resident_bytes.is_none_or(|bytes| bytes == 0);
            actual.resources_after.resident_bytes.is_none_or(|bytes| bytes == 0);
            maximum_helper_processes;
            maximum_resident_bytes;
            fn no_dispatch_evidence_is_empty(evidence: &CaseEvidence) -> bool { true }
            actual.target.navigation_observed != (case == FixtureCase::Link && !does_not_dispatch);
            GateOutcome::Denied | GateOutcome::Indeterminate;
            if actual.target.popup_requested {
                actual.outcome == CaseOutcome::Verified
            } else {
                actual.outcome == CaseOutcome::Unsupported
            };
        "#;
        validate_windows_probe_binary(binary, qualification).expect("valid physical runner");
        assert!(validate_windows_probe_binary(
            &binary.replace(
                "deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
                "allow(clippy::panic, clippy::unreachable, clippy::unwrap_used)",
            ),
            qualification,
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            &binary.replace("\"--allow-visible-focused\";", ""),
            qualification,
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            &binary.replace("encode_response_line(&response);", "println!(\"passed\");"),
            qualification,
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace(
                "focus_sequence_matches_presentation(actual.focus, mode.presentation());",
                "true;",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace("fn has_qualifying_event", "fn removed_qualifying_event"),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace(
                "event.is_trusted
                        && event.kind == required",
                "event.kind == required",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace(
                "actual.focus.target_received_dom_focus != target_focus_event_observed(actual);",
                "false;",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace("InputEventKind::Focus", "InputEventKind::Blur"),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace(
                "InputBackend::WindowsHwndInput | InputBackend::WindowsCdpInput",
                "InputBackend::FixedDomRecipe",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace(
                "actual.resources_before.helper_processes.is_none_or(|count| count == 0);",
                "false;",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace(
                "actual.resources_after.resident_bytes.is_none_or(|bytes| bytes == 0);",
                "false;",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace(
                "fn no_dispatch_evidence_is_empty(evidence: &CaseEvidence) -> bool { true }",
                "fn no_dispatch_evidence_is_empty(_: &CaseEvidence) -> bool { true }",
            ),
        )
        .is_err());
        assert!(validate_windows_probe_binary(
            binary,
            &qualification.replace(
                "actual.outcome == CaseOutcome::Verified",
                "actual.outcome == CaseOutcome::Unsupported",
            ),
        )
        .is_err());
    }

    #[test]
    fn windows_physical_reviewer_is_closed_and_content_free() {
        let binary = r#"
            const REVIEW_SCHEMA_VERSION: u16 = 2;
            WINDOWS_PHYSICAL_REVIEW_MODES;
            symlink_metadata(&directory);
            evidence_metadata_is_direct_directory(&metadata);
            evidence_metadata_is_direct_file(&metadata);
            decode_response_line(&bytes);
            qualify_windows_probe_evidence(mode, &evidence);
            file.take((MAX_PROTOCOL_OUTPUT_BYTES + 1) as u64);
            if output.len() > MAX_PROTOCOL_OUTPUT_BYTES {}
            stdout.write_all(&output);
            "--write-summary";
            "windows-review-summary-v2.json";
            OpenOptions::new().create_new(true);
            write_new_record(&directory, REVIEW_SUMMARY_FILENAME, &output);
        "#;
        let qualification = r#"
            "windows-hidden-fixed-dom.jsonl";
            "windows-hidden-hwnd.jsonl";
            "windows-hidden-cdp.jsonl";
            "windows-visible-background-all.jsonl";
            pub fn qualify_windows_probe_evidence() {}
            evidence.capabilities.as_slice() != WINDOWS_PROBE_CAPABILITIES;
        "#;
        validate_windows_review_binary(binary, qualification).expect("closed reviewer");
        assert!(validate_windows_review_binary(
            &format!("{binary}\nstdout.write_all(&bytes);"),
            qualification,
        )
        .is_err());
        assert!(validate_windows_review_binary(
            binary,
            &qualification.replace("windows-hidden-cdp.jsonl", "windows-any.jsonl"),
        )
        .is_err());
    }

    #[test]
    fn windows_semantic_reviewer_is_closed_and_content_free() {
        let binary = r#"
            const REVIEW_SCHEMA_VERSION: u16 = 5;
            WINDOWS_SEMANTIC_PHYSICAL_REVIEW_MODES;
            symlink_metadata(directory);
            evidence_metadata_is_direct_directory(&metadata);
            evidence_metadata_is_direct_file(&metadata);
            decode_windows_semantic_probe_response(&bytes);
            qualify_windows_semantic_probe_evidence(mode, &evidence);
            file.take((MAX_WINDOWS_SEMANTIC_PROBE_OUTPUT_BYTES + 1) as u64);
            if output.len() > MAX_WINDOWS_SEMANTIC_PROBE_OUTPUT_BYTES {}
            stdout.write_all(&output);
            "--write-summary";
            "windows-semantic-review-summary-v5.json";
            OpenOptions::new().create_new(true);
            write_new_record(&directory, REVIEW_SUMMARY_FILENAME, &output);
        "#;
        let evidence = r#"
            pub const WINDOWS_SEMANTIC_PROBE_PROTOCOL_VERSION: u16 = 5;
            pub const WINDOWS_SEMANTIC_PHYSICAL_REVIEW_MODES: [WindowsSemanticProbeMode; 7];
            "windows-semantic-fixed-documents.jsonl";
            "windows-semantic-redirect-lifecycle.jsonl";
            "windows-semantic-location-replacement.jsonl";
            "windows-semantic-suspend-resume.jsonl";
            "windows-semantic-event-flood.jsonl";
            "windows-semantic-renderer-loss.jsonl";
            "windows-semantic-debugger-coexistence.jsonl";
            pub fn qualify_windows_semantic_probe_evidence() {}
            evidence.peak_pending_invocations != 1;
            evidence.teardown.retained_native_views != 0;
            evidence.suspend_callback_succeeded;
            evidence.suspended_state_attested;
            evidence.resume_state_attested;
            evidence.post_resume_snapshot_verified;
            evidence.redirect_chain_verified;
            evidence.redirect_chain_hops_observed == 2;
            evidence.redirect_limit_refused;
            evidence.redirect_limit_hops_observed == MAX_CONTEXT_NAVIGATION_REDIRECTS as u8;
            evidence.redirect_recovery_verified;
            evidence.same_document_replacement_observed;
            evidence.same_document_replacement_rejoined;
            evidence.stale_location_join_refused;
            evidence.post_location_snapshot_verified;
            valid_resource_sample(self.resources_before);
            valid_resource_sample(self.resources_after);
            maximum_webview2_processes;
            maximum_resident_bytes;
            "semantic-runtime-m3-lifecycle-m2-redirect-location-resources-v4";
        "#;
        validate_windows_semantic_review_binary(binary, evidence).expect("closed reviewer");
        assert!(validate_windows_semantic_review_binary(
            &format!("{binary}\nstdout.write_all(&bytes);"),
            evidence,
        )
        .is_err());
        assert!(validate_windows_semantic_review_binary(
            binary,
            &evidence.replace("windows-semantic-event-flood.jsonl", "windows-any.jsonl"),
        )
        .is_err());
        assert!(validate_windows_semantic_review_binary(
            binary,
            &evidence.replace(
                "WINDOWS_SEMANTIC_PROBE_PROTOCOL_VERSION: u16 = 5",
                "WINDOWS_SEMANTIC_PROBE_PROTOCOL_VERSION: u16 = 2",
            ),
        )
        .is_err());
        assert!(validate_windows_semantic_review_binary(
            binary,
            &evidence.replace("valid_resource_sample(self.resources_before);", "true;",),
        )
        .is_err());
        assert!(validate_windows_semantic_review_binary(
            binary,
            &evidence.replace("maximum_resident_bytes;", "resident_bytes;"),
        )
        .is_err());
    }
}
