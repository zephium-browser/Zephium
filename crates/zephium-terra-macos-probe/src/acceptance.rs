//! The acceptance suite: every qualification scenario three times, each in
//! its own child process and directory, one closed row per run.
use std::{
    io::Write as _,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const SCENARIOS: [(&str, &str); 6] = [
    ("read_gate", "--live-agent-read-work"),
    ("lego_collection", "--live-agent-collection-work"),
    ("lego_details", "--live-agent-product-details-work"),
    ("trip_plan", "--live-agent-trip-work"),
    ("airbnb_three", "--live-agent-airbnb-work"),
    ("architecture_design", "--live-agent-architecture-work"),
];
/// Run only by name with --live-acceptance-only until the person admits
/// them to the suite.
const ON_REQUEST: [(&str, &str); 5] = [
    ("engine_chart", "--live-agent-engine-chart-work"),
    ("explain_mechanism", "--live-agent-explain-mechanism-work"),
    ("explain_mechanism_rust", "--live-agent-explain-rust-work"),
    ("code_review", "--live-agent-code-review-work"),
    ("concept_comparison", "--live-agent-concept-comparison-work"),
];
const RUNS: usize = 3;
/// Every scenario's own deadline: its 720 s execution plus the 30 s host
/// allowance the probe gives it.
const SCENARIO_DEADLINE: Duration = Duration::from_secs(750);
/// Closed margin past a scenario's deadline before the runner stops it.
const HANG_MARGIN: Duration = Duration::from_secs(120);
/// Runs at once; more contend for the host and turn timing into noise.
const CONCURRENT_RUNS: usize = 3;

/// Closed reasons a run failed its scenario's existing criteria.
#[derive(Clone, Copy, Debug)]
enum FailureReason {
    /// The execution did not end needing review with artifacts.
    Outcome,
    /// The collection or detail comparison lacked its required shape, or
    /// its one answer beside cited findings.
    Collection,
    /// The travel or Airbnb criteria, or the one answer, were unmet.
    Travel,
    /// The making set, its one answer, knowledge marks, reads or time were unmet.
    Design,
    /// A chart of nothing, repeated findings, or too many reads.
    Chart,
    /// The agent loop itself failed.
    RunFailure,
    /// The native host or the probe process failed.
    Host,
    /// The child outlived its scenario's deadline and margin and was stopped.
    Hung,
}

/// Starts a child whose stdout and stderr go straight to files in its own
/// run directory, so no undrained pipe can ever block it.
fn spawn_logged(command: &mut Command, directory: &Path) -> std::io::Result<Child> {
    let stdout = std::fs::File::create(directory.join("run.log"))?;
    let stderr = std::fs::File::create(directory.join("stderr.log"))?;
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
}

/// A child's exit status, or none when it outlived `limit` and was stopped.
fn settled(
    child: &mut Child,
    started: Instant,
    limit: Duration,
) -> std::io::Result<Option<Option<ExitStatus>>> {
    if let Some(status) = child.try_wait()? {
        return Ok(Some(Some(status)));
    }
    if started.elapsed() < limit {
        return Ok(None);
    }
    child.kill()?;
    child.wait()?;
    Ok(Some(None))
}

#[derive(Default)]
struct Row {
    wall_ms: u128,
    reads: u32,
    decision: u32,
    emulation: u32,
    planner: u32,
    read_cost: u64,
    basis: &'static str,
    run_cost: Option<u64>,
    accounting: &'static str,
}

fn field(line: &str, name: &str) -> Option<u64> {
    let start = line.find(&format!("{name}: "))? + name.len() + 2;
    line[start..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn measure(output: &str, wall_ms: u128) -> (Row, Option<FailureReason>, bool) {
    let mut row = Row {
        wall_ms,
        basis: "exact",
        accounting: "unknown",
        ..Row::default()
    };
    let (mut status_ok, mut accepted, mut run_failure, mut host) = (false, None, false, false);
    for line in output.lines() {
        if line.starts_with("agent-work: loop=ReadMeasured") {
            row.reads += 1;
            row.decision += field(line, "decision_calls").unwrap_or(0) as u32;
            row.emulation += field(line, "emulation_calls").unwrap_or(0) as u32;
            row.planner += field(line, "planner_calls").unwrap_or(0) as u32;
            row.read_cost += field(line, "cost_micro_usd").unwrap_or(0);
            row.basis = match (row.basis, line) {
                (_, line) if line.contains("cost_basis: Reserved") => "reserved",
                ("reserved", _) => "reserved",
                (_, line) if line.contains("cost_basis: Priced") => "priced",
                (basis, _) => basis,
            };
        } else if line.starts_with("agent-work: status=") {
            status_ok = line.starts_with("agent-work: status=NeedsReview")
                && !line.contains("artifacts=0;");
            row.run_cost = field(line, "cost_micro_usd");
            row.accounting = if line.contains("accounting: Exact") {
                "exact"
            } else {
                "reserved_ceiling"
            };
        } else if let Some(rest) = line.strip_prefix("agent-acceptance: ") {
            accepted = Some((
                rest.contains("collection=true"),
                rest.contains("travel=true"),
                rest.contains("design=true"),
                rest.contains("chart=true"),
            ));
        } else if line.contains("agent-work: run_failure=") {
            run_failure = true;
        } else if line.contains("host_failure=") {
            host = true;
        }
    }
    let reason = if run_failure {
        Some(FailureReason::RunFailure)
    } else if host {
        Some(FailureReason::Host)
    } else if !status_ok {
        Some(FailureReason::Outcome)
    } else {
        match accepted {
            Some((false, _, _, _)) => Some(FailureReason::Collection),
            Some((_, false, _, _)) => Some(FailureReason::Travel),
            Some((_, _, false, _)) => Some(FailureReason::Design),
            Some((_, _, _, false)) => Some(FailureReason::Chart),
            Some(_) => None,
            None => Some(FailureReason::Host),
        }
    };
    (row, reason, reason.is_none())
}

pub(super) fn run() -> Result<(), super::ProbeFailure> {
    run_queue(
        (0..RUNS)
            .flat_map(|repetition| {
                SCENARIOS
                    .iter()
                    .map(move |scenario| (repetition, *scenario))
            })
            .collect(),
    )
}

/// One run of one named scenario, with the suite's own row.
pub(super) fn run_one(name: &std::ffi::OsStr) -> Result<(), super::ProbeFailure> {
    let scenario = SCENARIOS
        .iter()
        .chain(&ON_REQUEST)
        .find(|(scenario, _)| name == *scenario)
        .ok_or(super::ProbeFailure::Authority)?;
    run_queue([(0, *scenario)].into())
}

fn run_queue(
    mut queue: std::collections::VecDeque<(usize, (&'static str, &'static str))>,
) -> Result<(), super::ProbeFailure> {
    use super::ProbeFailure as Error;
    let executable = std::env::current_exe().map_err(|_| Error::Authority)?;
    let root = std::path::Path::new("target/work-runtime-proof/acceptance");
    std::fs::create_dir_all(root).map_err(|_| Error::Output)?;
    let mut running = Vec::new();
    let mut passes = 0;
    let mut runs = 0;
    while !queue.is_empty() || !running.is_empty() {
        while running.len() < CONCURRENT_RUNS {
            let Some((repetition, (name, flag))) = queue.pop_front() else {
                break;
            };
            let directory = root.join(format!("{name}-{repetition}"));
            std::fs::create_dir_all(&directory).map_err(|_| Error::Output)?;
            let child = spawn_logged(Command::new(&executable).arg(flag), &directory)
                .map_err(|_| Error::Runtime)?;
            running.push((repetition, name, directory, Instant::now(), child));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut index = 0;
        while index < running.len() {
            let (_, _, _, started, child) = &mut running[index];
            let Some(status) = settled(child, *started, SCENARIO_DEADLINE + HANG_MARGIN)
                .map_err(|_| Error::Runtime)?
            else {
                index += 1;
                continue;
            };
            let (repetition, name, directory, started, _) = running.swap_remove(index);
            let wall_ms = started.elapsed().as_millis();
            // The child's own closed-fact lines, written there as it ran.
            let text = std::fs::read(directory.join("run.log")).unwrap_or_default();
            let text = String::from_utf8_lossy(&text);
            let (row, reason, pass) = measure(&text, wall_ms);
            let pass = pass && status.is_some_and(|status| status.success());
            let reason = if status.is_none() {
                Some(FailureReason::Hung)
            } else {
                reason.or((!pass).then_some(FailureReason::Host))
            };
            runs += 1;
            passes += usize::from(pass);
            let _ = writeln!(
                std::io::stdout().lock(),
                "acceptance: scenario={name} run={} result={} wall_ms={} reads={} decision_calls={} emulation_calls={} planner_calls={} read_cost_micro_usd={} read_cost_basis={} run_cost_micro_usd={} run_accounting={} failure={}",
                repetition + 1,
                if pass { "pass" } else { "fail" },
                row.wall_ms,
                row.reads,
                row.decision,
                row.emulation,
                row.planner,
                row.read_cost,
                row.basis,
                row.run_cost.map_or_else(|| "unknown".into(), |cost| cost.to_string()),
                row.accounting,
                reason.map_or_else(|| "none".into(), |reason| format!("{reason:?}")),
            );
        }
    }
    let _ = writeln!(
        std::io::stdout().lock(),
        "acceptance: passes={passes}/{runs}"
    );
    Ok(())
}

/// The lead's acceptance tasks (spec §9), each run once by name with
/// `--live-lead <name>`: the requests in order in one work, and the answer a
/// stand-in person gives to the lead's own questions.
pub(super) struct LeadScenario {
    pub name: &'static str,
    pub requests: &'static [&'static str],
    pub answer: &'static str,
    /// A throwaway folder the person grants, for code work.
    pub folder: bool,
    /// A page check: the start page a scripted helper browses, whose goal
    /// is the one request. Only the page agent calls a model.
    pub site: Option<&'static str>,
}

pub(super) fn site_scenario(start: &str, goal: &str) -> &'static LeadScenario {
    let start: &'static str = Box::leak(start.to_owned().into_boxed_str());
    let goal: &'static str = Box::leak(goal.to_owned().into_boxed_str());
    Box::leak(Box::new(LeadScenario {
        name: "site",
        requests: Box::leak(vec![goal].into_boxed_slice()),
        answer: "Allow",
        folder: false,
        site: Some(start),
    }))
}

/// A request of the person's own, through the whole lead.
/// The person's requests, one per line: each later line is a follow-up in
/// the same work, as the person sends it.
pub(super) fn request_scenario(request: &str, answer: &str) -> &'static LeadScenario {
    // Requests are one per line, or, when any line is `---`, the blocks
    // between such lines, so one request can hold a multi-line message.
    let blocks: Vec<String> = if request.lines().any(|line| line.trim() == "---") {
        request
            .split('\n')
            .collect::<Vec<_>>()
            .split(|line| line.trim() == "---")
            .map(|block| block.join("\n").trim().to_owned())
            .collect()
    } else {
        request.lines().map(|line| line.trim().to_owned()).collect()
    };
    let requests: Vec<&'static str> = blocks
        .into_iter()
        .filter(|block| !block.is_empty())
        .map(|block| &*Box::leak(block.into_boxed_str()))
        .collect();
    let answer: &'static str = Box::leak(answer.to_owned().into_boxed_str());
    Box::leak(Box::new(LeadScenario {
        name: "request",
        requests: Box::leak(requests.into_boxed_slice()),
        answer,
        folder: false,
        site: None,
    }))
}

/// Opt-in local evaluation output. This mode is limited to public requests in
/// a fresh private profile and never exports account/file/context contents.
fn public_eval_path(scenario: &LeadScenario) -> Result<Option<std::path::PathBuf>, &'static str> {
    let Some(path) = std::env::var_os("ZEPHIUM_EVAL_PUBLIC_EXPORT") else {
        return Ok(None);
    };
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute()
        || path.exists()
        || scenario.name != "request"
        || scenario.folder
        || [
            "ZEPHIUM_PROBE_PROFILE",
            "ZEPHIUM_PROBE_SEEDED_PROFILE",
            "ZEPHIUM_PROBE_WORK_MEMORY",
            "ZEPHIUM_PROBE_WORK_MEMORY_CLEAR",
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
    {
        return Err("eval_public_scope");
    }
    Ok(Some(path))
}

fn eval_read_decisions(
    value: Option<&str>,
) -> Result<
    (
        zephium_work_composition::durable_runtime::WorkDecisionPreference,
        &'static str,
    ),
    &'static str,
> {
    use zephium_work_composition::durable_runtime::WorkDecisionPreference as Preference;
    match value {
        None | Some("recommended") => Ok((Preference::Recommended, "recommended")),
        Some("emulation") => Ok((Preference::Emulation, "emulation")),
        Some("interactions") => Ok((Preference::Interactions, "interactions")),
        Some("disabled") => Ok((Preference::Disabled, "disabled")),
        _ => Err("eval_read_decisions"),
    }
}

#[cfg(test)]
mod read_decision_tests {
    use super::*;
    #[test]
    fn ablation_mode_is_explicit_and_never_changes_the_default() {
        use zephium_work_composition::durable_runtime::WorkDecisionPreference as P;
        assert_eq!(
            eval_read_decisions(None),
            Ok((P::Recommended, "recommended"))
        );
        for (value, expected) in [
            ("recommended", P::Recommended),
            ("emulation", P::Emulation),
            ("disabled", P::Disabled),
            ("interactions", P::Interactions),
        ] {
            assert_eq!(eval_read_decisions(Some(value)), Ok((expected, value)));
        }
        assert!(eval_read_decisions(Some("unknown")).is_err());
    }
}

#[derive(Default)]
struct EvalDiagnostics {
    calls: Vec<serde_json::Value>,
    pages: Vec<serde_json::Value>,
    images: Vec<EvalInputImage>,
    image_bytes: usize,
    images_limited: bool,
    page_traces: Vec<serde_json::Value>,
    action_traces: Vec<serde_json::Value>,
    action_traces_limited: bool,
    page_traces_limited: bool,
    truncated: bool,
}
struct EvalInputImage {
    hash: String,
    extension: &'static str,
    bytes: Vec<u8>,
}

fn remember_eval_images(
    local: &mut EvalDiagnostics,
    request: &zephium_core::work::model::WorkModelRequest,
) -> Vec<String> {
    use sha2::{Digest, Sha256};
    use zephium_core::work::model::{WorkModelMessage as M, WorkModelPart as P};
    if request
        .image_fence
        .as_ref()
        .is_none_or(|fence| !fence.while_current(&mut || {}))
    {
        return Vec::new();
    }
    let mut hashes = Vec::new();
    for message in &request.messages {
        let parts = match message {
            M::User(parts) => parts,
            _ => continue,
        };
        for part in parts {
            let P::Image { media_type, bytes } = part else {
                continue;
            };
            let extension = match media_type.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/webp" => "webp",
                _ => {
                    local.images_limited = true;
                    continue;
                }
            };
            let hash = format!("{:x}", Sha256::digest(bytes));
            if !local.images.iter().any(|image| image.hash == hash) {
                if bytes.len() > 1024 * 1024
                    || local.image_bytes.saturating_add(bytes.len()) > 8 * 1024 * 1024
                    || local.images.len() >= 16
                {
                    local.images_limited = true;
                    continue;
                }
                local.image_bytes += bytes.len();
                local.images.push(EvalInputImage {
                    hash: hash.clone(),
                    extension,
                    bytes: bytes.clone(),
                });
            }
            if !hashes.contains(&hash) {
                hashes.push(hash);
            }
        }
    }
    hashes
}

fn export_eval_images(
    path: &Path,
    public: bool,
    local: &EvalDiagnostics,
) -> Result<serde_json::Value, &'static str> {
    if !public {
        return Ok(serde_json::json!({"state":"withheld_private_provenance","images":[]}));
    }
    let mut exported = Vec::new();
    for image in &local.images {
        let calls: Vec<_> = local
            .calls
            .iter()
            .enumerate()
            .filter(|(_, call)| {
                call["state"] == "settled"
                    && call["input_image_hashes"]
                        .as_array()
                        .is_some_and(|hashes| hashes.iter().any(|hash| hash == &image.hash))
            })
            .map(|(index, _)| index)
            .collect();
        if calls.is_empty() {
            continue;
        }
        let name = format!("model-input-{}.{}", image.hash, image.extension);
        let destination = path.parent().ok_or("eval_image_parent")?.join(&name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(destination)
                .map_err(|_| "eval_image_open")?;
            file.write_all(&image.bytes)
                .map_err(|_| "eval_image_write")?;
        }
        #[cfg(not(unix))]
        return Err("eval_image_permissions_unqualified");
        exported.push(serde_json::json!({"file":name,"sha256":image.hash,"bytes":image.bytes.len(),"calls":calls}));
    }
    Ok(
        serde_json::json!({"state":"exported","images":exported,"limited":local.images_limited,
        "scope":"Copies of fenced images already supplied to successful public page-model calls. No new captures and no canvas record-crop collection."}),
    )
}

const EVAL_PAGE_TRACE_BYTES: usize = 96 * 1024;
const EVAL_PAGE_TRACE_CALLS: usize = 12;

fn eval_trace_parts(
    parts: &[zephium_core::work::model::WorkModelPart],
    text: bool,
) -> serde_json::Value {
    use zephium_core::work::model::WorkModelPart as P;
    serde_json::Value::Array(
        parts
            .iter()
            .filter_map(|part| match part {
                P::Text(value) if text => Some(serde_json::json!({"text": value})),
                P::ToolCall(call) => {
                    Some(serde_json::json!({"tool": call.name, "arguments": call.arguments}))
                }
                // Images and provider replay items never enter this text trace.
                _ => None,
            })
            .collect(),
    )
}

fn eval_trace_messages(
    messages: &[zephium_core::work::model::WorkModelMessage],
) -> serde_json::Value {
    use zephium_core::work::model::WorkModelMessage as M;
    serde_json::Value::Array(
        messages
            .iter()
            .map(|message| match message {
                M::User(parts) => {
                    serde_json::json!({"role":"user", "parts":eval_trace_parts(parts, true)})
                }
                M::Assistant(parts) => {
                    serde_json::json!({"role":"assistant", "parts":eval_trace_parts(parts, false)})
                }
                M::ToolResults(results) => {
                    serde_json::json!({"role":"tool_results", "results":results.iter().map(|result|
            serde_json::json!({"is_error":result.is_error,"content":result.content})
        ).collect::<Vec<_>>()})
                }
            })
            .collect(),
    )
}

fn eval_action_targets(tools: &[zephium_core::work::model::WorkModelTool]) -> serde_json::Value {
    let mut targets = serde_json::Map::new();
    if let Some(variants) = tools
        .iter()
        .find(|tool| tool.name == "act")
        .and_then(|tool| tool.schema["properties"]["actions"]["items"]["anyOf"].as_array())
    {
        for variant in variants.iter().take(32) {
            let Some(kind @ ("click" | "fill" | "select" | "press" | "scroll")) =
                variant["properties"]["kind"]["enum"][0].as_str()
            else {
                continue;
            };
            if let Some(references) = variant["properties"]["target"]["enum"].as_array() {
                let refs = targets
                    .entry(kind)
                    .or_insert_with(|| serde_json::json!([]))
                    .as_array_mut()
                    .unwrap();
                for reference in references.iter().take(128).filter(|value| {
                    value.as_str().is_some_and(|text| {
                        text.strip_prefix("@a").is_some_and(|digits| {
                            !digits.is_empty()
                                && digits.len() <= 10
                                && digits.bytes().all(|b| b.is_ascii_digit())
                        })
                    })
                }) {
                    if !refs.contains(reference) && refs.len() < 128 {
                        refs.push(reference.clone());
                    }
                }
            }
        }
    }
    serde_json::Value::Object(targets)
}

fn action_trace(trace: &serde_json::Value) -> Option<serde_json::Value> {
    let outputs = trace.get("output_tools")?.as_array()?;
    if outputs.is_empty() {
        return None;
    }
    let mut observation = None;
    let mut updates = Vec::new();
    for message in trace.get("messages")?.as_array()? {
        if let Some(parts) = message.get("parts").and_then(serde_json::Value::as_array) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                    if text.starts_with("ZSEM3 ") {
                        observation = Some(text.to_owned());
                        updates.clear();
                    }
                }
            }
        }
        if let Some(results) = message.get("results").and_then(serde_json::Value::as_array) {
            for result in results {
                if let Some(value) = result
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
                {
                    if let Some(text) = value
                        .get("observation")
                        .and_then(serde_json::Value::as_str)
                        .filter(|text| text.starts_with("ZSEM3 "))
                    {
                        observation = Some(text.to_owned());
                        updates.clear();
                        continue;
                    }
                }
                updates.push(result.clone());
            }
        }
    }
    let observation = observation?;
    let mut summary = serde_json::json!({"call_index":trace["call_index"],"base_observation":observation,"updates":updates,"proposals":outputs,"allowed_targets":trace["allowed_targets"]});
    if summary.to_string().len() > 24 * 1024 {
        let mut compact = String::new();
        for line in observation.lines() {
            let role = line.split_ascii_whitespace().nth(3).unwrap_or("");
            if (line.starts_with("ZSEM3 ")
                || line.starts_with("F ")
                || matches!(
                    role,
                    "r=button"
                        | "r=combobox"
                        | "r=listbox"
                        | "r=option"
                        | "r=checkbox"
                        | "r=radio"
                        | "r=textbox"
                        | "r=searchbox"
                        | "r=heading"
                        | "r=status"
                ))
                && compact.len() + line.len() + 1 <= 12 * 1024
            {
                compact.push_str(line);
                compact.push('\n');
            }
        }
        if !compact.starts_with("ZSEM3 ") {
            return None;
        }
        summary["base_observation"] = compact.into();
        summary["base_observation_limited"] = true.into();
        summary["updates"] = updates
            .iter()
            .rev()
            .take(2)
            .rev()
            .map(|update| {
                let bytes = update.to_string().len();
                if bytes <= 2048 {
                    update.clone()
                } else {
                    serde_json::json!({"omitted":true,"bytes":bytes})
                }
            })
            .collect::<Vec<_>>()
            .into();
    }
    (summary.to_string().len() <= 24 * 1024).then_some(summary)
}

fn remember_eval_page_trace(local: &mut EvalDiagnostics, trace: serde_json::Value) {
    if let Some(summary) = action_trace(&trace) {
        local.action_traces_limited |= summary["base_observation_limited"] == true;
        if local.action_traces.len() == 64 {
            local.action_traces.remove(0);
            local.action_traces_limited = true;
        }
        local.action_traces.push(summary);
    } else if trace["output_tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool["tool"] == "act"))
    {
        local.action_traces_limited = true;
    }
    if trace.to_string().len() > EVAL_PAGE_TRACE_BYTES {
        local.page_traces_limited = true;
        return;
    }
    if local.page_traces.len() == EVAL_PAGE_TRACE_CALLS {
        // Preserve the setup that caused a later recovery loop as well as its tail.
        local.page_traces.remove(EVAL_PAGE_TRACE_CALLS / 2);
        local.page_traces_limited = true;
    }
    local.page_traces.push(trace);
}

fn export_eval_page_traces(public: bool, local: &EvalDiagnostics) -> serde_json::Value {
    if !public {
        return serde_json::json!({"state":"withheld_private_provenance","turns":[]});
    }
    serde_json::json!({"state":if local.page_traces.is_empty() && !local.page_traces_limited { "not_recorded" } else { "exported" },"turns":local.page_traces,"actions":local.action_traces,"actions_limited":local.action_traces_limited,"limited":local.page_traces_limited,
        "scope":"Opt-in bounded local public page-model text and tool arguments. No system blocks, assistant prose, images, provider replay items or transport credentials; not a native action receipt."})
}

struct EvalModelClient {
    inner: Arc<dyn zephium_core::work::model::WorkModelClient>,
    role: &'static str,
    diagnostics: Arc<Mutex<EvalDiagnostics>>,
    visual_diagnostics: bool,
    trace_pages: bool,
    reject_images: bool,
}
fn eval_image_inputs(messages: &[zephium_core::work::model::WorkModelMessage]) -> (usize, usize) {
    use zephium_core::work::model::{WorkModelMessage as M, WorkModelPart as P};
    messages.iter().fold((0usize, 0usize), |total, message| {
        let parts = match message {
            M::User(parts) | M::Assistant(parts) => parts,
            M::ToolResults(_) => return total,
        };
        parts.iter().fold(total, |(count, size), part| match part {
            P::Image { bytes, .. } => (count.saturating_add(1), size.saturating_add(bytes.len())),
            _ => (count, size),
        })
    })
}
impl zephium_core::work::model::WorkModelClient for EvalModelClient {
    fn call<'a>(
        &'a self,
        request: zephium_core::work::model::WorkModelRequest,
        events: &'a (dyn Fn(zephium_core::work::model::WorkModelEvent) + Send + Sync),
    ) -> zephium_core::work::model::WorkModelFuture<'a> {
        Box::pin(async move {
            if self.reject_images && eval_image_inputs(&request.messages).0 != 0 {
                return Err(zephium_core::work::model::WorkModelError::BadRequest);
            }
            let started = Instant::now();
            let index = {
                let mut local = self.diagnostics.lock().unwrap_or_else(|e| e.into_inner());
                if local.calls.len() >= 4096 {
                    local.truncated = true;
                    None
                } else {
                    let index = local.calls.len();
                    local.calls.push(serde_json::json!({
                        "role": self.role, "model": request.model, "reasoning": request.reasoning,
                        "max_output_tokens": request.max_output_tokens, "native_search": request.native_search,
                        "parallel_tools": request.parallel_tools, "state": "unsettled",
                        "started_ms": STARTED.get_or_init(Instant::now).elapsed().as_millis(),
                    }));
                    if self.visual_diagnostics {
                        let (parts, bytes) = eval_image_inputs(&request.messages);
                        local.calls[index]["input_image_parts"] = serde_json::json!(parts);
                        local.calls[index]["input_image_bytes"] = serde_json::json!(bytes);
                        if self.role == "page" {
                            let hashes = remember_eval_images(&mut local, &request);
                            local.calls[index]["input_image_hashes"] = serde_json::json!(hashes);
                        }
                    }
                    Some(index)
                }
            };
            let trace =
                (self.trace_pages && self.visual_diagnostics && self.role == "page").then(|| {
                    (
                        eval_trace_messages(&request.messages),
                        eval_action_targets(&request.tools),
                    )
                });
            let result = self.inner.call(request, events).await;
            if let Some(index) = index {
                let mut local = self.diagnostics.lock().unwrap_or_else(|e| e.into_inner());
                let row = &mut local.calls[index];
                row["wall_ms"] = serde_json::json!(started.elapsed().as_millis());
                row["finished_ms"] =
                    serde_json::json!(STARTED.get_or_init(Instant::now).elapsed().as_millis());
                match &result {
                    Ok(outcome) => {
                        let usage = outcome.usage;
                        if self.visual_diagnostics {
                            use zephium_core::work::model::WorkModelPart;
                            row["screenshot_tool_requests"] = serde_json::json!(outcome.assistant.iter().filter(|part|
                                matches!(part, WorkModelPart::ToolCall(call) if call.name == "screenshot")
                            ).count());
                        }
                        row["state"] = serde_json::json!("settled");
                        use zephium_core::work::model::WorkModelStop;
                        row["stop"] = serde_json::json!(match outcome.stop {
                            WorkModelStop::EndTurn => "end_turn",
                            WorkModelStop::ToolUse => "tool_use",
                            WorkModelStop::MaxTokens => "max_tokens",
                            WorkModelStop::Refused => "refused",
                        });
                        row["input_tokens"] = serde_json::json!(usage.input_tokens);
                        row["cached_input_tokens"] = serde_json::json!(usage.cached_input_tokens);
                        row["cache_write_input_tokens"] =
                            serde_json::json!(usage.cache_write_input_tokens);
                        row["output_tokens"] = serde_json::json!(usage.output_tokens);
                        row["reasoning_tokens"] = serde_json::json!(usage.reasoning_tokens);
                        row["cost_micro_usd"] = serde_json::json!(usage.cost_micros);
                        row["cost_basis"] = serde_json::json!(if usage.cost_micros.is_some() {
                            "model_client_reported"
                        } else {
                            "unavailable"
                        });
                    }
                    Err(error) => {
                        row["state"] = serde_json::json!("refused");
                        row["failure"] = serde_json::json!(format!("{error:?}"));
                    }
                }
                if let Some((messages, allowed_targets)) = trace {
                    let output = match &result {
                        Ok(outcome) => eval_trace_parts(&outcome.assistant, false),
                        Err(_) => serde_json::Value::Null,
                    };
                    remember_eval_page_trace(
                        &mut local,
                        serde_json::json!({
                            "call_index":index,"messages":messages,"output_tools":output,"allowed_targets":allowed_targets,
                            "settled":result.is_ok(),
                        }),
                    );
                }
            }
            result
        })
    }
}

#[cfg(test)]
mod page_trace_tests {
    use super::*;
    use zephium_core::work::model::*;

    #[test]
    fn action_targets_export_only_bounded_operation_refs() {
        let tool = WorkModelTool {
            name: "act".into(),
            description: "omitted".into(),
            strict: true,
            schema: serde_json::json!({"properties":{"actions":{"items":{"anyOf":[
                {"properties":{"kind":{"enum":["click"]},"target":{"enum":["@a1","@a2","private-sentinel"]}}},
                {"properties":{"kind":{"enum":["unknown"]},"target":{"enum":["@a3"]}}}
            ]}}}}),
        };
        assert_eq!(
            eval_action_targets(&[tool]),
            serde_json::json!({"click":["@a1","@a2"]})
        );
    }

    #[test]
    fn trace_omits_images_replay_and_assistant_prose_and_withholds_private_runs() {
        let messages = vec![
            WorkModelMessage::User(vec![
                WorkModelPart::Text("public synthetic date picker".into()),
                WorkModelPart::Image {
                    media_type: "image/png".into(),
                    bytes: b"omitted-pixels".to_vec(),
                },
                WorkModelPart::Replay(serde_json::json!({"internal":"omitted-replay"})),
            ]),
            WorkModelMessage::Assistant(vec![
                WorkModelPart::Text("omitted-assistant-prose".into()),
                WorkModelPart::ToolCall(WorkModelToolCall {
                    id: "not-exported-id".into(),
                    name: "act".into(),
                    arguments: serde_json::json!({"value":"synthetic-date"}),
                }),
            ]),
        ];
        let mut local = EvalDiagnostics::default();
        remember_eval_page_trace(&mut local, eval_trace_messages(&messages));
        let text = export_eval_page_traces(true, &local).to_string();
        assert!(text.contains("public synthetic date picker"));
        assert!(text.contains("synthetic-date"));
        for withheld in [
            "omitted-pixels",
            "omitted-replay",
            "omitted-assistant-prose",
            "not-exported-id",
        ] {
            assert!(!text.contains(withheld));
        }
        let withheld = export_eval_page_traces(false, &local).to_string();
        assert!(!withheld.contains("synthetic-date"));
        assert!(!withheld.contains("public synthetic"));
    }

    #[test]
    fn action_trace_retains_current_observations_without_goals_or_old_tool_replay() {
        let trace = serde_json::json!({"call_index":7,"output_tools":[{"tool":"act","arguments":{"actions":[]}}],
            "messages":[{"parts":[{"text":"private goal sentinel"},{"text":"ZSEM3 old"}]},
            {"results":[{"content":"{\"observation\":\"ZSEM3 current\",\"status\":\"verified\"}"}]}]});
        let summary = action_trace(&trace).unwrap();
        assert_eq!(summary["base_observation"], "ZSEM3 current");
        assert!(!summary.to_string().contains("private goal sentinel"));
        let mut local = EvalDiagnostics::default();
        for _ in 0..70 {
            remember_eval_page_trace(&mut local, trace.clone());
        }
        assert_eq!(local.action_traces.len(), 64);
        assert!(!export_eval_page_traces(false, &local)
            .to_string()
            .contains("ZSEM3 current"));
        let mut oversized = trace;
        oversized["messages"] =
            serde_json::json!([{"parts":[{"text":format!("ZSEM3 {}","x".repeat(24*1024))}]}]);
        assert!(action_trace(&oversized).is_none());
    }

    #[test]
    fn large_action_context_keeps_complete_control_states_and_proposals() {
        let mut observation =
            "ZSEM3 content=untrusted nodes=322\nF f1 origin=\"https://fixture.example/\"\n"
                .to_owned();
        for index in 1..=320 {
            observation.push_str(&format!(
                "N @a{index} p=- r=paragraph text=\"{}\"\n",
                "x".repeat(120)
            ));
        }
        observation.push_str("N @a321 p=- r=button ops=click name=\"Price low to high\"\n");
        let trace = serde_json::json!({"call_index":7,"output_tools":[{"tool":"act","arguments":{"actions":[{"kind":"click","target":"@a321"}]}}],
            "messages":[{"parts":[{"text":observation}]}]});
        let summary = action_trace(&trace).unwrap();
        assert_eq!(summary["base_observation_limited"], true);
        assert!(summary["base_observation"]
            .as_str()
            .unwrap()
            .contains("name=\"Price low to high\"\n"));
        assert_eq!(
            summary["proposals"][0]["arguments"]["actions"][0]["target"],
            "@a321"
        );
        assert!(summary.to_string().len() < 24 * 1024);
        let mut after = trace.clone();
        after["output_tools"][0]["tool"] = "extract".into();
        assert!(action_trace(&after).unwrap()["base_observation"]
            .as_str()
            .unwrap()
            .contains("Price low to high"));
        let mut local = EvalDiagnostics::default();
        remember_eval_page_trace(&mut local, trace);
        assert!(local.action_traces_limited);
        assert!(!export_eval_page_traces(false, &local)
            .to_string()
            .contains("Price low to high"));
    }

    #[test]
    fn trace_retains_bounded_setup_and_recent_turns_without_clipping_quotes() {
        let mut local = EvalDiagnostics::default();
        for index in 0..20 {
            remember_eval_page_trace(&mut local, serde_json::json!({"call_index":index}));
        }
        assert_eq!(local.page_traces.len(), EVAL_PAGE_TRACE_CALLS);
        let indices: Vec<_> = local
            .page_traces
            .iter()
            .map(|row| row["call_index"].as_u64().unwrap())
            .collect();
        assert_eq!(indices, [0, 1, 2, 3, 4, 5, 14, 15, 16, 17, 18, 19]);
        remember_eval_page_trace(
            &mut local,
            serde_json::json!({"text":"x".repeat(EVAL_PAGE_TRACE_BYTES)}),
        );
        assert_eq!(local.page_traces.len(), EVAL_PAGE_TRACE_CALLS);
        assert_eq!(local.page_traces.last().unwrap()["call_index"], 19);
        assert!(local.page_traces_limited);
    }
}

#[cfg(test)]
mod visual_diagnostic_tests {
    use super::*;
    use zephium_core::work::model::*;
    struct Echo;
    impl WorkModelClient for Echo {
        fn call<'a>(
            &'a self,
            request: WorkModelRequest,
            _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
        ) -> WorkModelFuture<'a> {
            Box::pin(async move {
                assert!(matches!(&request.messages[0], WorkModelMessage::User(parts)
                    if matches!(&parts[1], WorkModelPart::Image { media_type, bytes }
                        if media_type == "image/png" && bytes == b"synthetic-pixels")));
                Ok(WorkModelOutcome {
                    stop: WorkModelStop::ToolUse,
                    usage: WorkModelUsage::default(),
                    assistant: vec![WorkModelPart::ToolCall(WorkModelToolCall {
                        id: "private-call".into(),
                        name: "screenshot".into(),
                        arguments: serde_json::json!({"private": "synthetic-secret"}),
                    })],
                })
            })
        }
    }
    #[tokio::test]
    async fn visual_counts_preserve_model_inputs_and_export_no_content_or_private_mode_counts() {
        for public in [false, true] {
            let diagnostics = Arc::new(Mutex::new(EvalDiagnostics::default()));
            let client = EvalModelClient {
                inner: Arc::new(Echo),
                role: "page",
                diagnostics: diagnostics.clone(),
                visual_diagnostics: public,
                trace_pages: !public,
                reject_images: false,
            };
            let request = WorkModelRequest {
                image_fence: None,
                model: WorkModelRef {
                    provider: WorkModelProvider::OpenAi,
                    wire: WorkModelWire::OpenAiResponses,
                    model: "fixture".into(),
                },
                system: vec![],
                tools: vec![],
                messages: vec![
                    WorkModelMessage::User(vec![
                        WorkModelPart::Text("synthetic-secret".into()),
                        WorkModelPart::Image {
                            media_type: "image/png".into(),
                            bytes: b"synthetic-pixels".to_vec(),
                        },
                    ]),
                    WorkModelMessage::Assistant(vec![WorkModelPart::Image {
                        media_type: "image/png".into(),
                        bytes: vec![0; 3],
                    }]),
                    WorkModelMessage::ToolResults(vec![WorkModelToolResult {
                        call: "private-call".into(),
                        content: "synthetic-secret".into(),
                        is_error: false,
                    }]),
                ],
                max_output_tokens: 1,
                reasoning: None,
                native_search: false,
                parallel_tools: false,
                tool_choice: WorkModelToolChoice::Auto,
                volatile_tail: 0,
                cache_key: None,
            };
            let rejecting = EvalModelClient {
                inner: Arc::new(Echo),
                role: "page",
                diagnostics: Arc::new(Mutex::new(EvalDiagnostics::default())),
                visual_diagnostics: false,
                trace_pages: false,
                reject_images: true,
            };
            assert!(matches!(
                rejecting.call(request.clone(), &|_| {}).await,
                Err(WorkModelError::BadRequest)
            ));
            assert!(rejecting.diagnostics.lock().unwrap().calls.is_empty());
            let outcome = client.call(request, &|_| {}).await.unwrap();
            assert_eq!(outcome.assistant.len(), 1);
            let local = diagnostics.lock().unwrap();
            assert!(
                local.page_traces.is_empty(),
                "private mode cannot enable the text trace"
            );
            assert_eq!(local.calls[0]["stop"], "tool_use");
            let row = &local.calls[0];
            if public {
                assert_eq!(row["input_image_parts"], 2);
                assert_eq!(row["input_image_bytes"], b"synthetic-pixels".len() + 3);
                assert_eq!(row["screenshot_tool_requests"], 1);
            } else {
                for key in [
                    "input_image_parts",
                    "input_image_bytes",
                    "screenshot_tool_requests",
                ] {
                    assert!(row.get(key).is_none());
                }
            }
            let exported = row.to_string();
            for private in [
                "synthetic-secret",
                "synthetic-pixels",
                "private-call",
                "image/png",
            ] {
                assert!(!exported.contains(private));
            }
        }
    }
}

fn public_image_provenance(
    disclosure: Option<zephium_core::work::runtime::WorkPageImageDisclosureV1>,
) -> bool {
    disclosure != Some(zephium_core::work::runtime::WorkPageImageDisclosureV1::SiteConsent)
}

fn public_export_allowed(state: &zephium_core::work::runtime::WorkRuntimeProjection) -> bool {
    use zephium_core::work::{parts::WorkInputKindV1, runtime::*};
    state.executions.iter().all(|execution| {
        execution.agent_grant().is_some_and(|grant| {
            grant.private && grant.folders.is_empty() && grant.accounts.is_empty()
        }) && execution
            .spec
            .context
            .as_ref()
            .is_none_or(|context| !context.requires_review())
            && execution
                .inputs
                .iter()
                .all(|input| input.kind == WorkInputKindV1::Skill)
            && execution.steps.iter().all(|step| {
                step.account.is_none()
                    && public_image_provenance(step.measurements.and_then(|m| m.image_disclosure))
                    && !step
                        .measurements
                        .and_then(|m| m.resolved_handoff)
                        .is_some_and(|kind| kind.includes(WorkPageHandoffKindV1::SignIn))
                    && !step.kind.files()
                    && !matches!(
                        step.kind,
                        WorkStepKindV1::RunCommand { .. } | WorkStepKindV1::EditBlock { .. }
                    )
            })
    })
}

#[cfg(unix)]
fn write_eval_file(path: &Path, value: &serde_json::Value) -> Result<(), &'static str> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| "eval_export_open")?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|_| "eval_export_write")?;
    file.flush().map_err(|_| "eval_export_write")
}
#[cfg(not(unix))]
fn write_eval_file(_path: &Path, _value: &serde_json::Value) -> Result<(), &'static str> {
    Err("eval_export_permissions_unqualified")
}

async fn collect_eval_sources<F, Fut>(
    public: bool,
    links: Vec<zephium_core::work::artifact::WorkEvidenceLink>,
    mut read: F,
) -> serde_json::Value
where
    F: FnMut(zephium_core::work::artifact::WorkEvidenceLink) -> Fut,
    Fut: std::future::Future<
        Output = Result<zephium_core::work::artifact::WorkEvidencePreviewV1, ()>,
    >,
{
    use zephium_core::work::artifact::WorkEvidenceSourceV1;
    if !public {
        return serde_json::json!({"state":"withheld_private_provenance","previews":[]});
    }
    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut previews = Vec::new();
    let mut unique = Vec::new();
    let mut failed = 0;
    let mut bytes = 0usize;
    let mut limited = false;
    for link in links {
        if unique.contains(&link) {
            continue;
        }
        if unique.len() >= 512 || tokio::time::Instant::now() >= deadline {
            limited = true;
            break;
        }
        unique.push(link.clone());
        let preview = match tokio::time::timeout_at(deadline, read(link.clone())).await {
            Ok(Ok(preview)) => preview,
            Ok(Err(())) => {
                failed += 1;
                continue;
            }
            Err(_) => {
                limited = true;
                break;
            }
        };
        if preview.link != link
            || !matches!(
                preview.source,
                WorkEvidenceSourceV1::NativeExtraction
                    | WorkEvidenceSourceV1::ProviderSearch { .. }
            )
        {
            failed += 1;
            continue;
        }
        let value = serde_json::to_value(&preview).expect("evidence preview serializes");
        let size = serde_json::to_vec(&value)
            .expect("evidence preview encodes")
            .len();
        if bytes.saturating_add(size) > 2 * 1024 * 1024 {
            limited = true;
            break;
        }
        bytes += size;
        previews.push(value);
    }
    serde_json::json!({"state":"exported","previews":previews,"failed":failed,"limited":limited,"bytes":bytes,"collection_ms":started.elapsed().as_millis(),
        "scope":"Original archived source excerpts for cited links; source text is untrusted data. No pixels, files or command output. Truncation is preserved per excerpt."})
}

fn export_eval(
    path: &Path,
    state: &zephium_core::work::runtime::WorkRuntimeProjection,
    models: &serde_json::Value,
    diagnostics: &Arc<Mutex<EvalDiagnostics>>,
    wall_ms: u128,
    finished_ms: u128,
    source_evidence: &serde_json::Value,
) -> Result<(), &'static str> {
    use zephium_core::work::runtime::*;
    let public = public_export_allowed(state);
    let kind = |value: serde_json::Value| {
        value
            .get("kind")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    let executions: Vec<_> = state.executions.iter().enumerate().map(|(index, execution)| {
        let published: Vec<_> = execution.steps.iter().filter(|step| matches!(step.kind, WorkStepKindV1::Publish))
            .flat_map(|step| step.artifacts.iter()).collect();
        let steps: Vec<_> = execution.steps.iter().map(|step| serde_json::json!({
            "id": step.id, "kind": kind(serde_json::to_value(&step.kind).unwrap_or_default()),
            "status": step.status, "artifacts": step.artifacts, "measurements": step.measurements,
            "url": match &step.kind { WorkStepKindV1::Read { url, .. } if public => Some(url), _ => None },
        })).collect();
        let artifacts: Vec<_> = if public { execution.artifacts.iter().map(|artifact| serde_json::json!({
            "id": artifact.id, "published": published.contains(&&artifact.id),
            "revises": artifact.revises,
            "current": !execution.artifacts.iter().any(|later| later.revises == Some(artifact.id)),
            "data": artifact.data, "evidence": artifact.evidence, "general_knowledge": artifact.general_knowledge,
        })).collect() } else { Vec::new() };
        serde_json::json!({
            "request": index + 1, "status": execution.status, "limits": execution.spec.limits,
            "attempts": execution.attempts.iter().map(|attempt| serde_json::json!({ "status": attempt.status, "usage": attempt.usage })).collect::<Vec<_>>(),
            "steps": steps, "artifacts": artifacts,
            "parts": execution.parts.iter().map(|part| serde_json::json!({ "helper": part.helper, "state": part.state,
                "need_kind": part.need.as_ref().and_then(|need| serde_json::to_value(need).ok()).map(kind) })).collect::<Vec<_>>(),
        })
    }).collect();
    let local = diagnostics.lock().unwrap_or_else(|e| e.into_inner());
    let model_input_images = export_eval_images(path, public, &local)?;
    write_eval_file(
        path,
        &serde_json::json!({
            "schema_version": 1, "case_id": std::env::var("ZEPHIUM_EVAL_CASE_ID").ok(),
            "public_artifacts": if public { "exported" } else { "withheld_private_provenance" },
            "profile_mode": "fresh_private", "stand_in": "decline_effects", "wall_ms": wall_ms,
            "models": models, "model_calls": local.calls, "model_calls_truncated": local.truncated,
            "page_intervals": local.pages,
            "source_evidence": source_evidence,
            "model_input_images": model_input_images,
            "public_page_turns": export_eval_page_traces(public, &local),
            "page_diagnostics_version": if public { Some(1) } else { None },
            "timeline": {"started_ms": finished_ms.saturating_sub(wall_ms), "finished_ms": finished_ms},
            "model_call_scope": "selected Work model clients; direct public-search transport is not included; do not add page diagnostics to this count",
            "executions": executions,
        }),
    )
}

/// The lead and helper turns of a page check: start one browser part, browse
/// the start page toward the goal with records, then finish.
pub(super) use super::loopback_support::Scripted;
pub(super) const LEAD_SCENARIOS: [LeadScenario; 23] = [
    LeadScenario { name: "blocks-checklist", requests: &["In this checklist, mark Pack passport done and append Book taxi due 2026-11-21. Preserve the other dates and completion flags. Update the existing block."], answer:"", folder:false, site:None },
    LeadScenario { name: "blocks-table", requests: &["In this table, update the row named Item120 so Status is Ready, then append Item200 with Owner Ada and Status Todo. Preserve every existing column and all other rows. Update the existing block."], answer:"", folder:false, site:None },
    LeadScenario { name: "blocks-calendar", requests: &["In my calendar, move Lunch to 2026-11-22 at 13:00 and append Walk on 2026-11-23 at 09:00. Preserve Train unchanged. Update the existing block."], answer:"", folder:false, site:None },
    // A fact the person states once is kept and shapes a later request.
    LeadScenario {
        name: "memory",
        requests: &[
            "I'm vegetarian, I live in Krakow and I prefer quiet places. Remember that.",
            "Suggest three places for dinner tonight",
        ],
        answer: "",
        folder: false,
        site: None,
    },
    // An open request: one finished piece of real work, never the person's
    // own apps.
    LeadScenario {
        name: "best",
        requests: &["Show me the best that you can do"],
        answer: "",
        folder: false,
        site: None,
    },
    // A file saved in a folder the run asks for as it works: the person
    // chooses it in the folder panel, and the same run writes the file.
    LeadScenario {
        name: "save",
        requests: &[
            "Explain how binary search works, in a few sentences and a short Python function",
            "Write that in a file in my Notes folder",
        ],
        answer: "",
        folder: false,
        site: None,
    },
    // A day planned from sources that need no real profile: the person's
    // Zephium tasks, seeded for today; the day's sources are asked once.
    LeadScenario {
        name: "day",
        requests: &["Plan my day"],
        answer: "Use these",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "flight",
        requests: &["Find me a flight WAW→SFO on 5 January 2027"],
        answer: "One adult, economy, one way.",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "month",
        requests: &["A month in San Francisco on Airbnb near South Park, from 1 January 2027"],
        answer: "One adult, up to $6,000 for the month.",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "trip",
        requests: &["Plan my YC batch trip from Warsaw"],
        answer: "The Winter 2027 batch, 5 January to 20 March 2027. One traveller with a Polish passport. Budget up to $9,000 for flights and stay together.",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "architecture",
        requests: &[
            "A modern AI SaaS architecture",
            "Compare AWS, Vercel, Hetzner and Cloudflare for this",
        ],
        answer: "A B2B SaaS with chat over the customer's documents, about 5,000 users in the first year.",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "today",
        requests: &["What do I need to do today? Check my Slack and Gmail."],
        answer: "Allow",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "bug",
        requests: &["The tests fail in my granted folder: fix the bug and show the tests passing"],
        answer: "Go ahead",
        folder: true,
        site: None,
    },
    LeadScenario {
        name: "compilers",
        requests: &["Learn compilers from free university material"],
        answer: "A software engineer, about 6 hours a week, 12 weeks.",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "products",
        requests: &["Compare three LEGO Star Wars sets from lego.com and pick one for a 10-year-old"],
        answer: "Up to $100.",
        folder: false,
        site: None,
    },
    // The request names a small repository the person has not shared yet:
    // the run asks for it in place, then reads it into a project.
    LeadScenario {
        name: "project",
        requests: &["What's in {repo}?"],
        answer: "",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "lego",
        requests: &["Open https://www.lego.com/en-us/themes/architecture, pick three sets, compare price and pieces"],
        answer: "Your pick.",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "compare",
        requests: &["Compare AWS, Vercel, Hetzner, Cloudflare for an AI SaaS"],
        answer: "A B2B SaaS with chat over the customer's documents, about 5,000 users in the first year.",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "explain",
        requests: &["Explain this Rust function:\n\nfn runs<T: PartialEq + Clone>(items: &[T], min: usize) -> Vec<(T, usize)> {\n    let mut out: Vec<(T, usize)> = Vec::new();\n    for item in items {\n        match out.last_mut() {\n            Some((last, n)) if last == item => *n += 1,\n            _ => out.push((item.clone(), 1)),\n        }\n    }\n    out.retain(|(_, n)| *n >= min);\n    out\n}"],
        answer: "",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "question",
        requests: &["Who wrote Dune?"],
        answer: "",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "conversion",
        requests: &["30 EUR in PLN"],
        answer: "",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "pricing",
        requests: &["How do Linear, Height and Plane price? I'm pricing a new issue tracker"],
        answer: "",
        folder: false,
        site: None,
    },
    LeadScenario {
        name: "exam",
        requests: &["Help me prepare for my algorithms final (MIT 6.006)"],
        answer: "The exam is on 16 October 2026; 2 to 3 hours a day.",
        folder: false,
        site: None,
    },
];

/// Today's tasks of a person with a launch this week, in the probe's own
/// store: due today, one overdue, one at a set time.
async fn seed_tasks(
    handle: &zephium_app::Handle,
    profile: zephium_core::ids::ProfileId,
) -> Result<(), &'static str> {
    use zephium_core::resources::*;
    let today = zephium_app::work_personal::local_day();
    let tasks: [(&str, &str, Option<&str>, TaskPriority); 4] = [
        (
            "Finish the launch post draft",
            "Blog post for Thursday's launch; needs the pricing section",
            None,
            TaskPriority::High,
        ),
        (
            "Review Marta's onboarding PR",
            "She is blocked until it is reviewed",
            Some("11:00"),
            TaskPriority::High,
        ),
        (
            "Call the accountant about Q3 VAT",
            "",
            Some("15:30"),
            TaskPriority::Medium,
        ),
        (
            "Book flights for the Berlin offsite",
            "Offsite is 12 to 14 November",
            None,
            TaskPriority::None,
        ),
    ];
    for (index, (title, description, time, priority)) in tasks.into_iter().enumerate() {
        let command = ResourceCommand {
            version: 1,
            request_id: format!("probe-day-task-{index:0>8}"),
            intent: ResourceIntent::Create {
                draft: ResourceDraft {
                    title: title.into(),
                    pinned: false,
                    content: ResourceContent::Task {
                        details: TaskDetails {
                            priority,
                            ..Default::default()
                        },
                        description: description.into(),
                        completed: false,
                        due_date: Some(today.clone()),
                        due_time: time.map(str::to_owned),
                        status: TaskStatus::Open,
                        assignee: TaskActor::User,
                        origin: TaskActor::User,
                        context: None,
                        sort_key: None,
                        work: None,
                    },
                    related: vec![],
                },
            },
        };
        let receiver = handle.resource_call(
            profile,
            ResourceCall::Mutate {
                command: Box::new(command),
            },
        );
        let reply =
            tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(10)))
                .await
                .map_err(|_| "seed_tasks")?
                .map_err(|_| "seed_tasks")?;
        if !matches!(reply.response, ResourceResponse::Applied { .. }) {
            return Err("seed_tasks");
        }
    }
    Ok(())
}

/// A small throwaway repository under the home folder: a web app with a
/// manifest, sources, a README and one uncommitted change.
fn project_folder() -> Result<std::path::PathBuf, &'static str> {
    let home = std::env::var_os("HOME").ok_or("home")?;
    let folder = std::path::PathBuf::from(home)
        .join("Library/Caches/app.zephium.probe")
        .join(format!("tidepool-{}", std::process::id()));
    let write = |path: &str, text: &str| {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().ok_or("project_folder")?)
            .map_err(|_| "project_folder")?;
        std::fs::write(path, text).map_err(|_| "project_folder")
    };
    write(
        "package.json",
        r#"{"name":"tidepool","description":"A tide-table web app for surfers, with forecasts from NOAA.","scripts":{"dev":"vite dev","build":"vite build","test":"vitest run"},"dependencies":{"@sveltejs/kit":"^2.9.0","svelte":"^5.2.0"},"devDependencies":{"typescript":"^5.7.2","vite":"^6.0.5","vitest":"^3.0.0","tailwindcss":"^4.0.0"}}"#,
    )?;
    write(
        "pnpm-lock.yaml",
        "lockfileVersion: '9.0'
",
    )?;
    write(
        "README.md",
        "# Tidepool

Tidepool shows the next tides for a surf spot. Forecasts come from NOAA.
",
    )?;
    write(
        "src/routes/+page.svelte",
        "<h1>Tides</h1>
",
    )?;
    write(
        "src/routes/spot/[id]/+page.svelte",
        "<h1>Spot</h1>
",
    )?;
    write(
        "src/lib/noaa.ts",
        "export const base = 'https://api.tidesandcurrents.noaa.gov';
",
    )?;
    write(
        "src/lib/tides.test.ts",
        "import { test } from 'vitest';
test('tides', () => {});
",
    )?;
    write("static/favicon.png", "")?;
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&folder)
            .env("GIT_AUTHOR_NAME", "Probe")
            .env("GIT_AUTHOR_EMAIL", "probe@example.com")
            .env("GIT_COMMITTER_NAME", "Probe")
            .env("GIT_COMMITTER_EMAIL", "probe@example.com")
            .output()
            .map(|_| ())
            .map_err(|_| "project_git")
    };
    git(&["init", "-q", "-b", "main"])?;
    git(&["add", "."])?;
    git(&["commit", "-q", "-m", "First tides"])?;
    write(
        "src/lib/noaa.ts",
        "export const base = 'https://api.tidesandcurrents.noaa.gov/v2';
",
    )?;
    Ok(folder)
}

/// A throwaway repository under the home folder with one failing test.
/// An empty folder the person picks when the run asks for one.
fn save_folder() -> Result<std::path::PathBuf, &'static str> {
    let home = std::env::var_os("HOME").ok_or("home")?;
    let folder = std::path::PathBuf::from(home)
        .join("Library/Caches/app.zephium.probe")
        .join(format!("lead-save-{}", std::process::id()));
    std::fs::create_dir_all(&folder).map_err(|_| "save_folder")?;
    Ok(folder)
}

fn bug_folder() -> Result<std::path::PathBuf, &'static str> {
    let home = std::env::var_os("HOME").ok_or("home")?;
    let folder = std::path::PathBuf::from(home)
        .join("Library/Caches/app.zephium.probe")
        .join(format!("lead-bug-{}", std::process::id()));
    std::fs::create_dir_all(&folder).map_err(|_| "bug_folder")?;
    std::fs::write(
        folder.join("pricing.py"),
        "def nightly_total(nightly, nights, cleaning_fee=0):\n    \"\"\"Total for a stay: every night plus one cleaning fee.\"\"\"\n    return nightly * (nights - 1) + cleaning_fee\n",
    )
    .map_err(|_| "bug_folder")?;
    std::fs::write(
        folder.join("test_pricing.py"),
        "import unittest\nfrom pricing import nightly_total\n\n\nclass NightlyTotal(unittest.TestCase):\n    def test_counts_every_night(self):\n        self.assertEqual(nightly_total(100, 3), 300)\n\n    def test_adds_the_cleaning_fee_once(self):\n        self.assertEqual(nightly_total(100, 2, 50), 250)\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
    )
    .map_err(|_| "bug_folder")?;
    Ok(folder)
}

static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn say(line: std::fmt::Arguments<'_>) {
    let at = STARTED.get_or_init(Instant::now).elapsed().as_millis();
    let _ = writeln!(std::io::stdout().lock(), "[{at:>7}ms] {line}");
}

fn public_page_diagnostic_enabled(
    public_eval: bool,
    account_probe: bool,
    value: Option<&str>,
) -> bool {
    public_eval && !account_probe && value == Some("1")
}

fn public_page_diagnostic(
    _: zephium_core::work::WorkAttemptId,
    snapshot: zephium_app::RetainedWorkSnapshot,
) {
    let disposition = snapshot.record.map(|record| record.disposition());
    let human_reason = snapshot
        .record
        .and_then(|record| record.human_handoff())
        .map(|handoff| handoff.reason());
    let human_wait =
        disposition.map(|value| value == zephium_agentic::AgentWorkDisposition::WaitingForHuman);
    say(format_args!(
        "public-page-phase: phase={:?} disposition={disposition:?} artifact_present={} artifact_read_present={} human_wait={human_wait:?} human_reason={human_reason:?}",
        snapshot.phase,
        snapshot.artifact.is_some(),
        snapshot.artifact_read.is_some(),
    ));
}

/// Shows the person the card's content in the terminal and waits for their
/// own yes or no.
async fn person_approves(confirm: &zephium_core::work::runtime::WorkConfirmV1) -> bool {
    use std::io::Write as _;
    let mut card = format!(
        "\n=== {} ({:?} on {}) ===\n{}\n",
        confirm.headline, confirm.category, confirm.site, confirm.action
    );
    for fact in &confirm.facts {
        card.push_str(&format!("{}: {}\n", fact.label, fact.value));
    }
    if let Some(text) = &confirm.text {
        card.push_str(&format!("---\n{text}\n---\n"));
    }
    card.push_str("Type yes to approve, anything else to decline: ");
    // The terminal itself, past whatever the run's output is piped into.
    match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        Ok(mut tty) => drop(tty.write_all(card.as_bytes())),
        Err(_) => drop(std::io::stderr().lock().write_all(card.as_bytes())),
    }
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("yes")
    })
    .await
    .unwrap_or(false)
}

/// Stands in for the person: answers the lead's questions with the
/// scenario's answer, lets sites in for this run, approves file changes
/// and commands in the granted folder, and declines every held step that
/// would commit something on a site.
async fn stand_in(
    handle: zephium_app::Handle,
    profile: zephium_core::ids::ProfileId,
    work: zephium_core::work::WorkId,
    answer: &'static str,
    pick: Option<std::path::PathBuf>,
    public_eval: bool,
) {
    use zephium_core::work::{runtime::*, *};
    use zephium_ipc::work::*;
    let mut kept_going = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let Ok(request) = handle.work_projection(profile, work) else {
            continue;
        };
        let WorkReplyV1::Projection { projection } = request.response(profile).await.reply else {
            continue;
        };
        if !public_eval && super::work_local_fixture::active().is_none() {
            // Kept as it goes, so a run the host stops can still be read.
            let _ = std::fs::create_dir_all("target/work-runtime-proof/lead");
            let _ = std::fs::write(
                "target/work-runtime-proof/lead/latest.json",
                serde_json::to_vec_pretty(&projection).unwrap_or_default(),
            );
        }
        let Some(execution) = projection
            .executions
            .iter()
            .rev()
            .find(|e| !e.status.terminal())
        else {
            continue;
        };
        let Some(step) = execution
            .steps
            .iter()
            .find(|step| step.status == WorkStepStatus::Running && needs_person(&step.kind))
        else {
            continue;
        };
        let intent = if let Some(fixture) = super::work_local_fixture::active() {
            match &step.kind {
                WorkStepKindV1::Ask{purpose:Some(WorkAskPurposeV1::Question),..}=>WorkRuntimeIntent::AnswerStep{execution:execution.id,step:step.id,answer:"Follow TASK.md in the granted synthetic folder; unavailable capabilities must be reported, never replaced with a shell.".into()},
                WorkStepKindV1::Ask{options,..}=>WorkRuntimeIntent::AnswerStep{execution:execution.id,step:step.id,answer:options.iter().find(|o|matches!(o.as_str(),"Not now"|"Decline"|"Stop"|"No")).cloned().unwrap_or_else(||"Not now".into())},
                _=>{let approve=fixture.approval(step);fixture.record_approval(step,approve);WorkRuntimeIntent::ApproveStep{execution:execution.id,step:step.id,approve,for_run:false}}
            }
        } else {
            match &step.kind {
                WorkStepKindV1::Ask { options, .. } => {
                    let reply = if public_eval {
                        match &step.kind {
                            WorkStepKindV1::Ask {
                                purpose: Some(WorkAskPurposeV1::Question),
                                ..
                            } => answer.to_owned(),
                            _ => options
                                .iter()
                                .find(|option| {
                                    matches!(option.as_str(), "Not now" | "Decline" | "Stop" | "No")
                                })
                                .cloned()
                                .unwrap_or_else(|| "Not now".into()),
                        }
                    } else if options.iter().any(|o| o == "Keep going") {
                        kept_going += 1;
                        if kept_going <= 2 {
                            "Keep going"
                        } else {
                            "Stop"
                        }
                        .to_owned()
                    } else if let Some(pick) = pick
                        .as_ref()
                        .filter(|_| options.first().map(String::as_str) == Some("Choose folder…"))
                    {
                        // The folder panel's choice goes back as its path.
                        pick.to_string_lossy().into_owned()
                    } else if let Some(allow) = options.iter().find(|o| o.starts_with("Allow")) {
                        allow.clone()
                    } else if !answer.is_empty() && answer != "Allow" {
                        answer.to_owned()
                    } else {
                        options
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "Go ahead".into())
                    };
                    say(format_args!(
                        "lead-person: answered question options={}",
                        options.len()
                    ));
                    WorkRuntimeIntent::AnswerStep {
                        execution: execution.id,
                        step: step.id,
                        answer: reply,
                    }
                }
                WorkStepKindV1::Confirm { confirm } => {
                    // A cookie banner is never a held step; one showing up here
                    // is a classifier miss, reported and declined like any other.
                    let action = confirm.action.to_lowercase();
                    let consent = ["reject", "accept", "cookie", "odrzuć", "necessary"]
                        .iter()
                        .any(|word| action.contains(word));
                    // A person at the terminal decides, as they would on the card.
                    let approve = !public_eval
                        && !consent
                        && std::env::var_os("ZEPHIUM_PROBE_PERSON").is_some()
                        && person_approves(confirm).await;
                    say(format_args!(
                        "lead-person: held step category={:?} approved={approve} consent_miss={consent}",
                        confirm.category
                    ));
                    WorkRuntimeIntent::ApproveStep {
                        execution: execution.id,
                        step: step.id,
                        approve,
                        for_run: false,
                    }
                }
                _ => {
                    say(format_args!(
                        "lead-person: proposed change approved={}",
                        !public_eval
                    ));
                    WorkRuntimeIntent::ApproveStep {
                        execution: execution.id,
                        step: step.id,
                        approve: !public_eval,
                        for_run: false,
                    }
                }
            }
        };
        if let Ok(request) = handle.work_command(
            profile,
            WorkCommandV1 {
                version: 1,
                work,
                expected_revision: projection.work.revision,
                command: WorkCommandId::generate(),
                intent,
            },
        ) {
            let _ = request.response(profile).await;
        }
    }
}

/// One closed row per page a run opened: its host, outcome, closed note,
/// cost and what it gave (records, and records with a picture).
fn page_rows(execution: &zephium_core::work::runtime::WorkExecutionFact) {
    use zephium_core::work::{artifact::WorkArtifactDataV1 as Data, runtime::*};
    for step in &execution.steps {
        let WorkStepKindV1::Read { url, goal, .. } = &step.kind else {
            continue;
        };
        let host = url
            .split_once("://")
            .map_or("", |(_, rest)| rest.split(['/', '?']).next().unwrap_or(""));
        let (mut records, mut pictured, mut priced) = (0, 0, 0);
        for artifact in execution
            .artifacts
            .iter()
            .filter(|artifact| step.artifacts.contains(&artifact.id))
        {
            match &artifact.data {
                Data::ComparisonMatrix { subjects, .. } | Data::Findings { subjects, .. } => {
                    records += subjects.len();
                    pictured += subjects
                        .iter()
                        .filter(|subject| !subject.image_candidates.is_empty())
                        .count();
                }
                _ => {}
            }
            if let Data::ComparisonMatrix { cells, .. } = &artifact.data {
                priced += cells
                    .iter()
                    .filter(|row| {
                        row.iter().any(|cell| {
                            matches!(
                                &cell.value,
                                zephium_core::work::artifact::WorkCellValue::Money { .. }
                            ) || matches!(&cell.value,
                                    zephium_core::work::artifact::WorkCellValue::Text { text }
                                        if text.chars().any(|c| "$€£¥".contains(c))
                                            || ["zł", "PLN", "USD", "EUR", "GBP"]
                                                .iter()
                                                .any(|code| text.contains(code)))
                        })
                    })
                    .count();
            }
        }
        say(format_args!(
            "lead-page: host={host} task={} status={:?} note={:?} wall_ms={} planner_calls={} actions={} tokens={} cost_micro_usd={} records={records} pictured={pictured} priced={priced}",
            goal.is_some(),
            step.status,
            step.note,
            step.measurements.as_ref().map_or(0, |m| m.wall_millis),
            step.measurements.as_ref().map_or(0, |m| m.planner_calls),
            step.measurements.as_ref().map_or(0, |m| m.native_actions),
            step.measurements.as_ref().map_or(0, |m| m.model_tokens),
            step.measurements.as_ref().map_or(0, |m| m.cost_micro_usd),
        ));
    }
}

/// One closed summary row for a lead run's execution.
pub(super) use super::loopback_support::{part_rows, run_row};
/// Probe-only public reachability check. These bytes are not stored or rendered;
/// canvas image coverage requires an actual record-media reference.
pub(super) async fn media_row(execution: &zephium_core::work::runtime::WorkExecutionFact) {
    use zephium_agentic::public_asset::{fetch_public_image, PublicAssetError};
    use zephium_core::work::artifact::WorkArtifactDataV1 as Data;
    let mut urls: Vec<String> = Vec::new();
    for step in &execution.steps {
        if !matches!(
            step.kind,
            zephium_core::work::runtime::WorkStepKindV1::Publish
        ) {
            continue;
        }
        for artifact in execution
            .artifacts
            .iter()
            .filter(|artifact| step.artifacts.contains(&artifact.id))
        {
            match &artifact.data {
                Data::Picks { items, .. } => {
                    urls.extend(items.iter().flat_map(|i| i.image_candidates.clone()))
                }
                Data::ComparisonMatrix { subjects, .. } | Data::Findings { subjects, .. } => {
                    urls.extend(subjects.iter().flat_map(|s| s.image_candidates.clone()))
                }
                _ => {}
            }
        }
    }
    urls.sort();
    urls.dedup();
    let candidates = urls.len();
    let (mut admitted, mut too_large, mut other, mut scaled) = (0, 0, 0, 0);
    for url in urls.into_iter().take(16) {
        match fetch_public_image(&url).await {
            Ok(bytes) => {
                admitted += 1;
                // Admission scales these to display size before storing them.
                if bytes.len() > zephium_core::resources::MAX_MEDIA_FETCHED_IMAGE_BYTES as usize {
                    scaled += 1;
                }
            }
            Err(PublicAssetError::TooLarge) => too_large += 1,
            Err(_) => other += 1,
        }
    }
    say(format_args!(
        "lead-media: candidates={candidates} admitted={admitted} refused={} too_large={too_large} scaled={scaled} purpose=public_reachability_only",
        too_large + other
    ));
}

async fn enrich_media_for_mode<F, Fut>(account: bool, local: bool, enrich: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if !account && !local {
        enrich().await;
    }
}

fn needs_person(kind: &zephium_core::work::runtime::WorkStepKindV1) -> bool {
    use zephium_core::work::runtime::WorkStepKindV1 as K;
    match kind {
        K::Ask { answer, .. } => answer.is_none(),
        K::Confirm { confirm } => confirm.decision.is_none(),
        kind => kind.proposes_write() && kind.file_decision().is_none(),
    }
}

fn public_eval_model(public: bool, value: Option<&str>) -> Result<&'static str, &'static str> {
    if !public {
        return Ok("openai/gpt-6-luna");
    }
    match value {
        None | Some("gpt-6-luna") => Ok("openai/gpt-6-luna"),
        Some("gpt-6.1-sol") => Ok("openai/gpt-6.1-sol"),
        Some("deepseek-flash") => Ok("deepseek/deepseek-flash"),
        _ => Err("eval_model_choice"),
    }
}

#[cfg(test)]
#[test]
fn evaluation_model_choices_cannot_change_private_qualification_or_supply_an_endpoint() {
    for choice in [
        None,
        Some("deepseek-flash"),
        Some("gpt-6.1-sol"),
        Some("https://other.test/model"),
    ] {
        assert_eq!(
            public_eval_model(false, choice).unwrap(),
            "openai/gpt-6-luna"
        );
    }
    assert_eq!(public_eval_model(true, None).unwrap(), "openai/gpt-6-luna");
    assert_eq!(
        public_eval_model(true, Some("deepseek-flash")).unwrap(),
        "deepseek/deepseek-flash"
    );
    assert!(public_eval_model(true, Some("https://other.test/model")).is_err());
}

/// One lead scenario in a live host: every request in turn, one closed
/// row per request, and the projection kept for reading its objects.
pub(super) async fn lead_workflow(
    handle: &zephium_app::Handle,
    composition: &zephium_work_composition::MacosWorkComposition,
    profile: zephium_core::ids::ProfileId,
    space: zephium_core::ids::SpaceId,
    binding: zephium_app::AgentWorkProfileBinding,
    mut keys: Vec<zephium_agentic::AgentProviderCredential>,
    scenario: &'static LeadScenario,
) -> Result<super::work_durable::WorkflowResult, &'static str> {
    use zephium_agentic::{
        AgentProviderTransport, AgentProviderTransportConfig, OpenAiPublicSearch,
        OpenAiPublicSearchConfig,
    };
    use zephium_app::work_lead::{LeadModel, WorkLeadModels, WorkLeadService};
    use zephium_core::work::{model::WorkModelRole, runtime::*, search::*, *};
    use zephium_ipc::work::*;
    let export = public_eval_path(scenario)?;
    let public_eval = export.is_some();
    let account_probe = super::work_account::active().is_some();
    let local_fixture = super::work_local_fixture::active();
    let heldout_blocks = super::work_heldout_blocks::active();
    let local_probe = local_fixture.is_some() || heldout_blocks.is_some();
    if local_probe
        && (account_probe || public_eval || scenario.folder || scenario.name != "request")
    {
        return Err("local_scope");
    }
    if account_probe && (public_eval || scenario.folder || scenario.name != "request") {
        return Err("account_scope");
    }
    let read_decision_env = if public_eval {
        std::env::var("ZEPHIUM_EVAL_READ_DECISIONS")
            .map(Some)
            .or_else(|error| match error {
                std::env::VarError::NotPresent => Ok(None),
                _ => Err("eval_read_decisions"),
            })?
    } else {
        None
    };
    let (read_decisions, read_decision_name) = eval_read_decisions(read_decision_env.as_deref())?;
    let eval_model_env = if public_eval {
        match std::env::var("ZEPHIUM_EVAL_MODEL") {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => return Err("eval_model_choice"),
        }
    } else {
        None
    };
    let eval_model = public_eval_model(public_eval, eval_model_env.as_deref())?;
    let block_probe = scenario.name.starts_with("blocks-");
    let _ = STARTED.get_or_init(Instant::now);
    let eval_started = Instant::now();
    let local_diagnostics = Arc::new(Mutex::new(EvalDiagnostics::default()));
    if public_eval || block_probe || account_probe || local_probe {
        let _ = zephium_app::work_models::view(profile).await;
        for role in [
            WorkModelRole::Lead,
            WorkModelRole::Page,
            WorkModelRole::Light,
        ] {
            let choice = if role == WorkModelRole::Lead && eval_model == "deepseek/deepseek-flash" {
                "openai/gpt-6-luna"
            } else {
                eval_model
            };
            zephium_app::work_models::choose(profile, role, Some(choice))
                .map_err(|_| "eval_model_choice")?;
        }
    }
    let resolve = |role| async move {
        zephium_app::work_models::resolve_entry(profile, role)
            .await
            .ok()
            .map(|(entry, client)| LeadModel { entry, client })
    };
    // A named page model, as a person picks one in Settings → AI.
    if let Ok(id) = std::env::var("ZEPHIUM_PROBE_PAGE_MODEL").and_then(|id| {
        if public_eval || block_probe || account_probe || local_probe {
            Err(std::env::VarError::NotPresent)
        } else {
            Ok(id)
        }
    }) {
        let _ = zephium_app::work_models::view(profile).await;
        zephium_app::work_models::choose(profile, WorkModelRole::Page, Some(&id))
            .map_err(|_| "page_model_choice")?;
    }
    let mut lead = resolve(WorkModelRole::Lead).await.ok_or("lead_model")?;
    let mut page = resolve(WorkModelRole::Page)
        .await
        .unwrap_or_else(|| lead.clone());
    let mut light = resolve(WorkModelRole::Light)
        .await
        .unwrap_or_else(|| page.clone());
    // Qualify Flash as the lead on public tasks without changing catalog defaults.
    if public_eval && eval_model == "deepseek/deepseek-flash" {
        lead = page.clone();
    }
    if account_probe {
        for model in [&mut lead, &mut page, &mut light] {
            model.entry.supports.vision = false;
        }
    }
    let model_setup = serde_json::json!({ "lead": lead.entry, "page": page.entry, "light": light.entry, "public_search_model": PUBLIC_SEARCH_MODEL, "read_triage": read_decision_name });
    if public_eval || block_probe || account_probe || local_probe {
        for (role, model) in [
            ("lead", &mut lead),
            ("page", &mut page),
            ("light", &mut light),
        ] {
            if model.entry.id != eval_model || PUBLIC_SEARCH_MODEL != "gpt-6-luna" {
                return Err("eval_model_policy");
            }
            model.client = Arc::new(EvalModelClient {
                inner: model.client.clone(),
                role,
                diagnostics: local_diagnostics.clone(),
                visual_diagnostics: public_eval,
                trace_pages: public_eval
                    && std::env::var("ZEPHIUM_EVAL_PUBLIC_PAGE_TRACE").as_deref() == Ok("1"),
                reject_images: account_probe,
            });
        }
    }
    say(format_args!(
        "lead-run: models lead={} page={} light={}",
        lead.entry.id, page.entry.id, light.entry.id
    ));
    // The page agent runs on the chosen page model, as in the app; a scripted
    // lead still browses with the real page and light models.
    let chosen_page =
        zephium_agentic::AgentProviderModelClient::try_new(&page.entry, page.client.clone())
            .or_else(|_| {
                zephium_agentic::AgentProviderModelClient::try_new(&lead.entry, lead.client.clone())
            })
            .map_err(|_| "page_model")?;
    let chosen_light =
        zephium_agentic::AgentProviderModelClient::try_new(&light.entry, light.client.clone()).ok();
    let models = match scenario.site {
        Some(start) => {
            let scripted = LeadModel {
                entry: lead.entry.clone(),
                client: Arc::new(Scripted { start }),
            };
            WorkLeadModels {
                lead: scripted.clone(),
                page: scripted.clone(),
                light: scripted,
            }
        }
        None => WorkLeadModels { lead, page, light },
    };
    // Work's memory on a live site: Never keeps the start site out of the
    // person's session, so its page opens in Work's own store.
    let memory_host = scenario
        .site
        .filter(|_| std::env::var_os("ZEPHIUM_PROBE_WORK_MEMORY").is_some())
        .and_then(|start| zephium_app::work_sites::site_of(start.split(' ').next()?));
    if let Some(host) = &memory_host {
        zephium_app::work_sites::set_standing(
            handle,
            profile,
            host.clone(),
            Some(zephium_core::work::sites::WorkSiteAccessV1::Never),
        )
        .await
        .map_err(|_| "memory_standing")?;
        say(format_args!(
            "work-memory: profile={profile} stage=before stores(own,work)={:?}",
            super::work_site::stores_hold(profile, host)
        ));
    }
    if !local_probe {
        super::work_site::seed_profile(profile).await?;
    }
    let folder = if let Some(fixture) = local_fixture {
        Some(fixture.root.clone())
    } else if scenario.folder {
        Some(bug_folder()?)
    } else {
        None
    };
    let repo = if scenario.name == "project" {
        Some(project_folder()?)
    } else {
        None
    };
    let named = |request: &str| match &repo {
        Some(repo) => request.replace("{repo}", &repo.to_string_lossy()),
        None => request.to_owned(),
    };
    let created = handle
        .work_authoring_command(
            profile,
            WorkAuthoringCommandV1 {
                version: 1,
                command: WorkCommandId::generate(),
                intent: WorkAuthoringIntent::Create {
                    objective: named(scenario.requests[0]),
                },
            },
        )
        .map_err(|_| "create_admission")?
        .response(profile)
        .await;
    let WorkReplyV1::AuthoringApplied { receipt: created } = created.reply else {
        return Err("create_persistence");
    };
    let work = created.work;
    let transport = AgentProviderTransport::try_new(AgentProviderTransportConfig::STANDARD)
        .map_err(|_| "transport")?;
    let fallback = OpenAiPublicSearch::try_new(
        transport,
        keys.pop().ok_or("search_key")?,
        OpenAiPublicSearchConfig::try_new(
            zephium_agent_model_catalog::try_public_search_provider_exact_call_config(
                PUBLIC_SEARCH_MODEL,
                4096,
            )
            .map_err(|_| "search_model")?,
        )
        .map_err(|_| "search_config")?,
    )
    .map_err(|_| "search_provider")?;
    // As the app searches: the engine's results page first, the provider's
    // search behind it. ZEPHIUM_PROBE_SEARCH_ENGINE picks the engine.
    let engine = std::env::var("ZEPHIUM_PROBE_SEARCH_ENGINE")
        .ok()
        .and_then(|id| zephium_core::search::SearchEngine::from_id(&id))
        .unwrap_or_default();
    say(format_args!("lead-run: search engine={}", engine.id()));
    let search = zephium_agentic::BrowserSearch::try_new(engine, "")
        .and_then(|browser| {
            browser.with_fallback(
                Box::new(fallback),
                WorkSearchFallbackV1 {
                    provider: WorkSearchProvider::OpenAi,
                    model: PUBLIC_SEARCH_MODEL.into(),
                },
            )
        })
        .map_err(|_| "search_provider")?;
    if scenario.name == "day" {
        seed_tasks(handle, profile).await?;
    }
    let block_fixture = super::work_blocks::seed(handle, profile, space, scenario.name).await?;
    if let Some(fixture) = heldout_blocks {
        fixture.seed(handle, profile).await?;
    }
    let pick = if scenario.name == "save" {
        Some(save_folder()?)
    } else {
        None
    };
    let person = (!account_probe).then(|| {
        tokio::spawn(stand_in(
            handle.clone(),
            profile,
            work,
            scenario.answer,
            pick.clone(),
            public_eval || block_probe || local_probe,
        ))
    });
    let keys = Arc::new(Mutex::new(keys));
    let callback = handle.callback_handle();
    let service = WorkLeadService::new(handle.clone()).with_diagnostic(|event| {
        say(format_args!("lead-run: event={event:?}"));
    });
    let no_search = super::work_account::NoSearch;
    let active_search: &dyn WorkPublicSearchProvider = if account_probe || local_probe {
        &no_search
    } else {
        &search
    };
    let mut expected = created.applied_revision;
    let mut state = None;
    let mut failure = None;
    for (index, request) in scenario.requests.iter().enumerate() {
        if super::work_account::active().is_some_and(|account| account.stopped()) {
            break;
        }
        if index > 0 {
            let edited = handle
                .work_authoring_command(
                    profile,
                    WorkAuthoringCommandV1 {
                        version: 1,
                        command: WorkCommandId::generate(),
                        intent: WorkAuthoringIntent::Edit {
                            work,
                            expected_revision: expected,
                            edit: WorkUserEdit::SetObjective {
                                objective: named(request),
                            },
                        },
                    },
                )
                .map_err(|_| "follow_up_admission")?
                .response(profile)
                .await;
            let WorkReplyV1::AuthoringApplied { receipt } = edited.reply else {
                return Err("follow_up_persistence");
            };
            expected = receipt.applied_revision;
        }
        let started = Instant::now();
        let running = service.run(
            profile,
            WorkCommandV1 {
                version: 1,
                work,
                expected_revision: expected,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::BeginAgent {
                    grant: WorkAgentGrantV1 {
                        no_usage_charge: false,
                        provider: WorkSearchProvider::OpenAi,
                        model: PUBLIC_SEARCH_MODEL.into(),
                        max_turns: 10,
                        max_steps: 32,
                        browse_hops: 4,
                        folders: folder
                            .iter()
                            .map(|f| f.to_string_lossy().into_owned())
                            .collect(),
                        accounts: vec![],
                        private: public_eval || block_probe || account_probe || local_probe,
                        lead: None,
                        skill: None,
                        search_fallback: None,
                        draft: None,
                    },
                    limits: WorkExecutionLimits {
                        model_tokens: 1_000_000,
                        cost_micro_usd: 3_000_000,
                        operations: 256,
                        timeout_seconds: 1_800,
                        max_workers: 4,
                    },
                },
            },
            block_fixture
                .as_ref()
                .map(super::work_blocks::Fixture::selection),
            models.clone(),
            active_search,
            |probe, request| {
                let key = keys.lock().ok().and_then(|mut keys| keys.pop());
                let callback = &callback;
                let chosen_page = chosen_page.clone();
                let chosen_light = chosen_light.clone();
                let page_diagnostics = local_diagnostics.clone();
                async move {
                    if local_probe {
                        return Err(WorkError::Unavailable);
                    }
                    if super::work_account::active()
                        .is_some_and(|account| !account.permits(&request))
                    {
                        return Err(WorkError::Unavailable);
                    }
                    let key = match key {
                        Some(key) => key,
                        None => tokio::task::spawn_blocking(
                            zephium_agentic::load_macos_probe_openai_credential,
                        )
                        .await
                        .map_err(|_| WorkError::Unavailable)?
                        .map_err(|_| WorkError::Unavailable)?,
                    };
                    let mut settings = super::work_durable::browser_settings(binding, key);
                    settings.access =
                        zephium_agent_controller::AgentBrowserModelAccess::Chosen(chosen_page);
                    settings.decision_model = chosen_light;
                    if account_probe {
                        settings.retain_public_responses = false;
                    }
                    if public_eval {
                        settings.decisions = read_decisions;
                    }
                    if public_eval || block_probe || account_probe || local_probe {
                        settings.stage_diagnostic = None;
                        settings.model_diagnostic = None;
                        settings.resource_diagnostic = None;
                        settings.diagnostic = None;
                    }
                    if public_eval {
                        settings.resource_diagnostic = Some(|cause| {
                            if let Some(cause) = cause {
                                let _ = writeln!(
                                    std::io::stdout().lock(),
                                    "page-resource: cause={cause:?}; content=redacted"
                                );
                            }
                        });
                    }
                    if account_probe {
                        settings.stage_diagnostic = Some(super::work_account::stage);
                        settings.model_diagnostic = Some(super::work_account::model_event);
                    }
                    if public_page_diagnostic_enabled(
                        public_eval,
                        account_probe,
                        std::env::var("ZEPHIUM_EVAL_PAGE_PHASE_DIAGNOSTIC")
                            .ok()
                            .as_deref(),
                    ) {
                        settings.diagnostic = Some(public_page_diagnostic);
                    }
                    let page_index = public_eval.then(|| {
                        let mut diagnostics = page_diagnostics.lock().unwrap_or_else(|e| e.into_inner());
                        if diagnostics.pages.len() >= 4096 { diagnostics.truncated = true; return None; }
                        let index = diagnostics.pages.len();
                        diagnostics.pages.push(serde_json::json!({"started_ms": STARTED.get_or_init(Instant::now).elapsed().as_millis()}));
                        Some(index)
                    }).flatten();
                    if public_eval {
                        settings.model_diagnostic = Some(|event| say(format_args!("page-model: event={event:?}")));
                    }
                    let result = composition.run_agent_step(callback, &probe, request, settings).await;
                    if let Some(index) = page_index {
                        let mut diagnostics = page_diagnostics.lock().unwrap_or_else(|e| e.into_inner());
                        diagnostics.pages[index]["finished_ms"] = serde_json::json!(STARTED.get_or_init(Instant::now).elapsed().as_millis());
                    }
                    result
                }
            },
            |_| {},
        );
        let result = if let Some(account) = super::work_account::active() {
            tokio::select! {
                result = running => result,
                _ = account.person(handle, composition, profile, work) => Err(WorkError::Unavailable),
            }
        } else {
            tokio::select! {
                result = running => result,
                () = super::work_account::present_human_pages(composition, profile, work) => Err(WorkError::Unavailable),
            }
        };
        let projection = match result {
            Ok(projection) => projection,
            Err(error) => {
                say(format_args!(
                    "lead-run: scenario={} request={} run_failure={error:?}",
                    scenario.name,
                    index + 1
                ));
                failure = Some("lead_run");
                break;
            }
        };
        expected = projection.work.revision;
        let execution = projection.executions.last().ok_or("lead_execution")?;
        run_row(
            scenario.name,
            index + 1,
            execution,
            started.elapsed().as_millis(),
        );
        if let Some(account) = super::work_account::active() {
            account.report(execution);
        }
        if !public_eval && !block_probe && !account_probe && !local_probe {
            page_rows(execution);
            part_rows(execution);
        }
        enrich_media_for_mode(account_probe, local_probe, || media_row(execution)).await;
        if !matches!(
            execution.status,
            WorkExecutionStatus::Completed | WorkExecutionStatus::NeedsReview
        ) {
            failure = failure.or(Some("lead_outcome"));
        }
        state = Some(projection);
    }
    if let Some(person) = person {
        person.abort();
    }
    if let Some(pick) = pick {
        let written = std::fs::read_dir(&pick)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.metadata().is_ok_and(|meta| meta.len() > 0))
                    .count()
            })
            .unwrap_or(0);
        say(format_args!(
            "lead-run: scenario=save files_written={written}"
        ));
        let _ = std::fs::remove_dir_all(&pick);
        if written == 0 {
            failure = failure.or(Some("save_nothing_written"));
        }
    }
    if let Some(folder) = folder.filter(|_| !local_probe) {
        let fixed = std::fs::read_to_string(folder.join("pricing.py")).unwrap_or_default();
        say(format_args!(
            "lead-run: scenario=bug fixed={}",
            fixed.contains("nightly * nights")
        ));
        let _ = std::fs::remove_dir_all(&folder);
    }
    if let Some(host) = &memory_host {
        say(format_args!(
            "work-memory: profile={profile} stage=after stores(own,work)={:?}",
            super::work_site::stores_hold(profile, host)
        ));
        if std::env::var_os("ZEPHIUM_PROBE_WORK_MEMORY_CLEAR").is_some() {
            say(format_args!(
                "work-memory: profile={profile} stage=cleared empty={:?} stores(own,work)={:?}",
                super::work_site::clear_work_memory(profile),
                super::work_site::stores_hold(profile, host)
            ));
        }
    }
    let state = state.ok_or("lead_no_run")?;
    if let Some(fixture) = &block_fixture {
        if let Err(error) = super::work_blocks::verify(handle, profile, fixture, &state).await {
            failure = failure.or(Some(error));
        }
    }
    let fixture_diagnostics = || {
        let diagnostics = local_diagnostics.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::json!({
            "selected_client_calls": diagnostics.calls,
            "truncated": diagnostics.truncated,
            "scope": "selected Work model clients; no browser/search dispatch permitted",
            "attempt_usage": state.executions.last().map(|e| e.attempts.iter().map(|a| a.usage).collect::<Vec<_>>()),
            "step_usage": state.executions.last().map(|e| e.steps.iter().map(|s| s.usage).collect::<Vec<_>>()),
        })
    };
    if let Some(fixture) = local_fixture {
        if !fixture.export(
            &state,
            &model_setup,
            fixture_diagnostics(),
            eval_started.elapsed().as_millis(),
        )? {
            failure = failure.or(Some("local_outcome"));
        }
    } else if let Some(fixture) = heldout_blocks {
        if !fixture
            .export(
                handle,
                profile,
                &state,
                &model_setup,
                fixture_diagnostics(),
                eval_started.elapsed().as_millis(),
            )
            .await?
        {
            failure = failure.or(Some("heldout_blocks_outcome"));
        }
    } else if let Some(path) = export {
        let wall_ms = eval_started.elapsed().as_millis();
        let finished_ms = STARTED.get_or_init(Instant::now).elapsed().as_millis();
        let links = state
            .executions
            .iter()
            .flat_map(|execution| execution.artifacts.iter())
            .flat_map(|artifact| artifact.evidence.iter().cloned())
            .collect();
        let source_evidence =
            collect_eval_sources(public_export_allowed(&state), links, |link| async move {
                let response = handle
                    .work_evidence(profile, work, link)
                    .map_err(|_| ())?
                    .response(profile)
                    .await;
                match response.reply {
                    WorkReplyV1::Evidence { evidence } => Ok(*evidence),
                    _ => Err(()),
                }
            })
            .await;
        export_eval(
            &path,
            &state,
            &model_setup,
            &local_diagnostics,
            wall_ms,
            finished_ms,
            &source_evidence,
        )?;
    } else if !block_probe && !account_probe {
        let directory = std::path::Path::new("target/work-runtime-proof/lead");
        let _ = std::fs::create_dir_all(directory);
        let _ = std::fs::write(
            directory.join(format!("{}.json", scenario.name)),
            serde_json::to_vec_pretty(&state).unwrap_or_default(),
        );
    }
    Ok(super::work_durable::WorkflowResult { state, failure })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn evidence_export_never_reads_private_mode_and_deduplicates_public_links() {
        use zephium_core::work::artifact::*;
        let rows: Vec<WorkEvidencePreviewV1> = serde_json::from_str(include_str!(
            "../../zephium-ipc/fixtures/work-research-evidence-v1.json"
        ))
        .unwrap();
        let preview = rows[0].clone();
        let called = std::sync::atomic::AtomicUsize::new(0);
        let private = collect_eval_sources(false, vec![preview.link.clone()], |_| async {
            called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(preview.clone())
        })
        .await;
        assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(private["state"], "withheld_private_provenance");
        let public = collect_eval_sources(
            true,
            vec![preview.link.clone(), preview.link.clone()],
            |_| async {
                called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(preview.clone())
            },
        )
        .await;
        assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(public["previews"].as_array().unwrap().len(), 1);
        assert_eq!(public["previews"][0]["text"], preview.text);
        assert_eq!(public["previews"][0]["truncated"], preview.truncated);
    }

    #[tokio::test]
    async fn evidence_export_rejects_wrong_links_local_sources_and_excess_text() {
        use zephium_core::work::artifact::*;
        let rows: Vec<WorkEvidencePreviewV1> = serde_json::from_str(include_str!(
            "../../zephium-ipc/fixtures/work-research-evidence-v1.json"
        ))
        .unwrap();
        let original = rows[0].clone();
        let mut wrong = original.clone();
        wrong.link.source_id = wrong.link.source_id.saturating_add(1);
        let mut local = original.clone();
        local.source = WorkEvidenceSourceV1::File {
            path: "/private/never-export".into(),
            name: "private".into(),
            file_kind: zephium_core::work::runtime::WorkFileKindV1::Text,
        };
        local.text = "PRIVATE FIXTURE TEXT".into();
        for refused in [wrong, local] {
            let result = collect_eval_sources(true, vec![original.link.clone()], |_| async {
                Ok(refused.clone())
            })
            .await;
            assert_eq!(result["failed"], 1);
            assert!(result["previews"].as_array().unwrap().is_empty());
            assert!(!result.to_string().contains("PRIVATE FIXTURE TEXT"));
        }
        let mut oversized = original.clone();
        oversized.text = "x".repeat(2 * 1024 * 1024);
        let result = collect_eval_sources(true, vec![original.link.clone()], |_| async {
            Ok(oversized.clone())
        })
        .await;
        assert_eq!(result["limited"], true);
        assert!(result["previews"].as_array().unwrap().is_empty());
    }

    #[test]
    fn image_evidence_is_local_public_only_and_requires_a_settled_call() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("evidence.json");
        let mut diagnostics = EvalDiagnostics::default();
        diagnostics.images.push(EvalInputImage {
            hash: "fixture".into(),
            extension: "png",
            bytes: vec![1, 2, 3],
        });
        diagnostics
            .calls
            .push(serde_json::json!({"state":"settled","input_image_hashes":["fixture"]}));
        assert_eq!(
            export_eval_images(&path, false, &diagnostics).unwrap()["state"],
            "withheld_private_provenance"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        diagnostics.calls[0]["state"] = serde_json::json!("refused");
        assert!(
            export_eval_images(&path, true, &diagnostics).unwrap()["images"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        diagnostics.calls[0]["state"] = serde_json::json!("settled");
        let value = export_eval_images(&path, true, &diagnostics).unwrap();
        assert_eq!(value["images"].as_array().unwrap().len(), 1);
        assert_eq!(
            std::fs::read(directory.path().join("model-input-fixture.png")).unwrap(),
            [1, 2, 3]
        );
    }

    #[test]
    fn public_export_refuses_site_consented_possible_image_disclosure() {
        use zephium_core::work::runtime::WorkPageImageDisclosureV1;
        assert!(public_image_provenance(None));
        assert!(public_image_provenance(Some(
            WorkPageImageDisclosureV1::PublicIsolated
        )));
        assert!(!public_image_provenance(Some(
            WorkPageImageDisclosureV1::SiteConsent
        )));
    }

    #[tokio::test]
    async fn local_and_account_modes_never_invoke_media_candidate_fetch() {
        let candidate = "https://example.com/inert-candidate.png";
        for (account, local) in [(false, true), (true, false), (true, true)] {
            let invoked = std::sync::atomic::AtomicBool::new(false);
            enrich_media_for_mode(account, local, || async {
                assert!(!candidate.is_empty());
                invoked.store(true, std::sync::atomic::Ordering::SeqCst);
            })
            .await;
            assert!(!invoked.load(std::sync::atomic::Ordering::SeqCst));
        }
    }

    #[test]
    fn public_page_diagnostic_is_explicit_and_never_for_accounts() {
        assert!(public_page_diagnostic_enabled(true, false, Some("1")));
        for value in [None, Some(""), Some("true"), Some("1")] {
            assert!(!public_page_diagnostic_enabled(false, false, value));
            assert!(!public_page_diagnostic_enabled(true, true, value));
        }
        assert!(!public_page_diagnostic_enabled(true, false, None));
    }

    #[test]
    fn a_child_writing_more_than_a_mebibyte_completes_and_a_hung_one_is_stopped() {
        let directory = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let mut child = spawn_logged(
            Command::new("/bin/sh").args([
                "-c",
                "head -c 3000000 /dev/zero | tr '\\0' x; echo done >&2",
            ]),
            directory.path(),
        )
        .unwrap();
        let status = loop {
            if let Some(status) = settled(&mut child, started, Duration::from_secs(60)).unwrap() {
                break status;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.is_some_and(|status| status.success()));
        assert_eq!(
            std::fs::metadata(directory.path().join("run.log"))
                .unwrap()
                .len(),
            3_000_000
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("stderr.log")).unwrap(),
            "done\n"
        );
        let started = Instant::now();
        let mut child =
            spawn_logged(Command::new("/bin/sleep").arg("60"), directory.path()).unwrap();
        let status = loop {
            if let Some(status) = settled(&mut child, started, Duration::from_millis(200)).unwrap()
            {
                break status;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.is_none());
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
