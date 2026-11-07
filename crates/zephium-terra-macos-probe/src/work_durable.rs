//! Opt-in public qualification: model plan, exact approval, native browser,
//! source-backed artifact, resource closure, then a new Store incarnation.
use std::{
    io::Write as _,
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};
use zephium_agentic::{
    load_macos_probe_openai_credential, AgentProviderTransport, AgentProviderTransportConfig,
    OpenAiPublicSearch, OpenAiPublicSearchConfig, OpenAiWorkAgent, OpenAiWorkPlanner,
    OpenAiWorkSynthesizer, WorkPlanningConfig,
};
use zephium_app::{
    work_agent::{WorkAgentProviders, WorkAgentService},
    work_execution::WorkExecutionService,
    work_planning::WorkPlanningService,
    work_runtime::WorkRuntimeService,
};
use zephium_core::{
    ids::{ProfileId, SpaceId},
    ports::store::Store,
    profiles::ProfileKind,
    session::{PersistedProfile, PersistedSpace, SessionState},
    work::{port::*, runtime::*, *},
};
use zephium_ipc::work::*;
use zephium_work_composition::MacosWorkComposition;

const OBJECTIVE: &str = "Find SQLite's official explanation of why WAL mode does not work when clients on different machines share a database over a network filesystem. Produce one concise source-backed note as a single plan responsibility. Use only public documentation at sqlite.org or www.sqlite.org. No account, writes, installations, or external communication are needed. Every factual output needs source-mapped human review.";
const COORDINATED_OBJECTIVE: &str = "Explain SQLite's official reason that WAL mode does not work when clients on different machines share a database over a network filesystem. Use exactly two plan responsibilities: a delegated public-documentation research worker with one source-backed findings output, then a primary agent that depends on those findings and produces one concise source-backed explanation. Both outputs require source_mapped_needs_review. Use only sqlite.org or www.sqlite.org. No accounts, writes, installations or external communication are needed.";

// Provider TLS runs on these threads; match the app's agent worker stack.
const PROVIDER_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;
const AGENT_COLLECTION_OBJECTIVE: &str = "Read https://www.lego.com/en-us/themes/architecture in the browser and collect three distinct Architecture sets with their displayed prices and useful distinguishing details. Return a cited comparison with displayed price text, distinguishing details, product links and images from the actual page, using structured collection. Do not buy, sign in, change locale, or use search snippets as a substitute for inspecting the actual catalog. Omit details that the page does not establish.";
const AGENT_DETAILS_OBJECTIVE: &str = "Open https://www.lego.com/en-us/themes/architecture, choose three distinct Architecture sets, and visit each of their observed product links. On each product page inspect the displayed price and product specifications, especially piece count and dimensions when shown. Return a cited structured comparison with product links, images and distinguishing details. The catalog alone is insufficient: inspect all three product pages. Do not buy, sign in, change locale, or use search snippets as a substitute. Leave unsupported details unknown.";
const AGENT_MONEY_OBJECTIVE: &str = "Read https://demo.vercel.store/product/acme-geometric-circles-t-shirt in the browser and collect the Acme Circles T-Shirt with its explicitly displayed price, currency code and product image. Return only the target product with its observed amount and currency. Use one responsibility with one source-mapped output. Do not buy, sign in or change the cart. Do not substitute search snippets for the page.";
const AGENT_READ_OBJECTIVE: &str = "From SQLite's official WAL documentation page, list every situation in which WAL mode does not work or has drawbacks, as cited findings with the page itself as the source. Read the actual page rather than relying on search snippets; use only sqlite.org.";
const AGENT_GOVERNMENT_OBJECTIVE: &str = "Read https://travel.state.gov/ in the browser and report the passport and travel advisory services shown there, citing the actual page. Use one page read. Do not substitute search results, another page or prior knowledge. If verification prevents reading, report that honestly and stop; do not interact with verification controls, sign in or submit forms.";
const AGENT_ENGINE_CHART_OBJECTIVE: &str = "Compare SQLite, DuckDB and RocksDB for a local-first desktop app with millions of records and frequent full-text search, then make a chart of their performance and resource use.";
const AGENT_ARCHITECTURE_OBJECTIVE: &str = "Create me a full modern AI SaaS architecture and in general system design, technologies, stack, how much it will cost, etc.";
/// A pasted function to review: the window loop reads one element past the
/// end, and `snapshot` is a clone nothing uses.
const CODE_REVIEW_FUNCTION: &str = r#"pub fn rolling_report(readings: &[Reading], window: usize) -> Report {
    let mut report = Report::default();
    if readings.is_empty() || window == 0 {
        return report;
    }
    let snapshot = readings.to_vec();
    let mut total = 0.0;
    let mut peak = f64::MIN;
    let mut low = f64::MAX;
    let mut spikes = Vec::new();
    for reading in readings {
        total += reading.value;
        if reading.value > peak {
            peak = reading.value;
        }
        if reading.value < low {
            low = reading.value;
        }
    }
    report.mean = total / readings.len() as f64;
    report.peak = peak;
    report.low = low;
    let mut window_sum = 0.0;
    for (index, reading) in readings.iter().enumerate().take(window) {
        window_sum += reading.value;
        report.averages.push(Average {
            at: index,
            value: window_sum / (index + 1) as f64,
        });
    }
    for i in window..=readings.len() {
        window_sum += readings[i].value - readings[i - window].value;
        let average = window_sum / window as f64;
        report.averages.push(Average { at: i, value: average });
        if readings[i].value > average * 1.5 {
            spikes.push(Spike {
                at: i,
                value: readings[i].value,
                average,
            });
        }
    }
    let mut run = 0;
    for pair in readings.windows(2) {
        if pair[1].value > pair[0].value {
            run += 1;
            report.longest_rise = report.longest_rise.max(run);
        } else {
            run = 0;
        }
    }
    report.spikes = spikes;
    report.flat = (peak - low).abs() < f64::EPSILON;
    report.label = match report.spikes.len() {
        0 => "steady".to_string(),
        1..=3 => "noisy".to_string(),
        _ => "unstable".to_string(),
    };
    report
}"#;
/// The line of the off-by-one, found by its text in whatever excerpt the
/// agent publishes.
const CODE_REVIEW_BUG: &str = "window..=readings.len()";
const AGENT_EXPLAIN_MECHANISM_OBJECTIVE: &str =
    "Explain how virtual memory works, with the prerequisites first.";
/// The same scenario on a mechanism of a programming language.
const AGENT_EXPLAIN_RUST_OBJECTIVE: &str =
    "Explain how ownership and borrowing work in Rust, with the prerequisites first.";
static EXPLAIN_IN_RUST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const AGENT_CONCEPT_COMPARISON_OBJECTIVE: &str =
    "Compare Rust ownership with tracing garbage collection.";
/// Objects the loop refused as malformed during the run.
static MALFORMED_REFUSALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Explanation and review answer from knowledge: a tighter wall time.
const EXPLANATION_DEADLINE: Duration = Duration::from_secs(120);
/// A making request answers from knowledge: a bounded wall time for the loop.
const ARCHITECTURE_DEADLINE: Duration = Duration::from_secs(180);
/// Wall time of the last agent loop, in milliseconds.
static AGENT_ELAPSED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const AGENT_OBJECTIVE: &str = "Compare Svelte Flow and React Flow as the canvas library for a desktop app: bundle size, license, and how actively each is maintained in 2026. Place the two libraries as subjects with cited findings, and finish with a short comparison.";

pub(super) use super::loopback_support::WorkflowResult;
#[derive(Clone, Copy, Eq, PartialEq)]
enum Mode {
    Public,
    Coordinated,
    CancelCoordinated,
    ProductIntegration,
    /// The routine agent loop: turns, searches, native reads, published objects.
    Agent,
    /// The same loop on an objective that needs a native page read.
    AgentRead,
    AgentGovernment,
    AgentHumanGovernment,
    /// The loop on a granted folder: a file read cited as a source and an
    /// edit applied after the person's approval.
    AgentFiles,
    AgentScroll,
    AgentDisclosure,
    AgentCollection,
    AgentDetails,
    AgentTrip,
    AgentAirbnb,
    AgentListing,
    /// One read of the page named on the command line.
    AgentPage,
    AgentMoney,
    MoneyNode,
    /// A making request: a design answered from knowledge as a set.
    AgentArchitecture,
    /// A research comparison asking for a chart of numbers sources rarely give.
    AgentEngineChart,
    /// How something works, answered as a set in learning order.
    AgentExplainMechanism,
    /// A pasted function reviewed as a code excerpt with line notes.
    AgentCodeReview,
    /// Two ideas compared from knowledge in one matrix.
    AgentConceptComparison,
    /// Signed-in origin grants against two loopback sites; no public site.
    LoopbackSite,
    /// One of the lead's acceptance tasks, by name.
    Lead,
}

static LEAD_SCENARIO: std::sync::OnceLock<&'static super::acceptance::LeadScenario> =
    std::sync::OnceLock::new();

/// One lead acceptance task in a live host, by its name.
pub(super) fn run_lead(name: &std::ffi::OsStr) -> Result<(), super::ProbeFailure> {
    let scenario = super::acceptance::LEAD_SCENARIOS
        .iter()
        .find(|scenario| name == scenario.name)
        .ok_or(super::ProbeFailure::Authority)?;
    if scenario.name.starts_with("blocks-")
        && [
            "ZEPHIUM_PROBE_PROFILE",
            "ZEPHIUM_PROBE_SEEDED_PROFILE",
            "ZEPHIUM_PROBE_WORK_MEMORY",
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
    {
        return Err(super::ProbeFailure::Authority);
    }
    let _ = LEAD_SCENARIO.set(scenario);
    run_mode(Mode::Lead)
}

/// The person's own requests (one per line, later lines follow up in the
/// same work) through the whole lead, as the app runs
/// it, with the stand-in's answer to any question it asks.
pub(super) fn run_lead_request(
    request: &std::ffi::OsStr,
    answer: &std::ffi::OsStr,
) -> Result<(), super::ProbeFailure> {
    let (Some(request), Some(answer)) = (request.to_str(), answer.to_str()) else {
        return Err(super::ProbeFailure::Authority);
    };
    if request.is_empty() || request.len() > 2400 || answer.len() > 600 {
        return Err(super::ProbeFailure::Authority);
    }
    let _ = LEAD_SCENARIO.set(super::acceptance::request_scenario(request, answer));
    run_mode(Mode::Lead)
}

pub(super) fn run_local_fixture(
    case: &std::ffi::OsStr,
    export: &std::ffi::OsStr,
) -> Result<(), super::ProbeFailure> {
    let case = case.to_str().ok_or(super::ProbeFailure::Authority)?;
    let objective = if case.starts_with("blocks-") {
        super::work_heldout_blocks::configure(case, std::path::Path::new(export))
    } else {
        super::work_local_fixture::configure(case, std::path::Path::new(export))
    }
    .map_err(|_| super::ProbeFailure::Authority)?;
    LEAD_SCENARIO
        .set(super::acceptance::request_scenario(
            &objective,
            "Follow the frozen fixture request; report unavailable capabilities honestly.",
        ))
        .map_err(|_| super::ProbeFailure::Authority)?;
    let result = run_mode(Mode::Lead);
    if let Some(fixture) = super::work_local_fixture::active() {
        fixture.cleanup();
    }
    if let Some(fixture) = super::work_heldout_blocks::active() {
        fixture.cleanup();
    }
    result
}

pub(super) fn run_account_read(start: &std::ffi::OsStr) -> Result<(), super::ProbeFailure> {
    run_account(start, false)
}

pub(super) fn run_account_send(start: &std::ffi::OsStr) -> Result<(), super::ProbeFailure> {
    run_account(start, true)
}

fn run_account(start: &std::ffi::OsStr, send: bool) -> Result<(), super::ProbeFailure> {
    let url = start.to_str().ok_or(super::ProbeFailure::Authority)?;
    let request = if send {
        super::work_account::configure_send(url)
    } else {
        super::work_account::configure(url)
    }
    .map_err(|_| super::ProbeFailure::Authority)?;
    LEAD_SCENARIO
        .set(super::acceptance::request_scenario(&request, ""))
        .map_err(|_| super::ProbeFailure::Authority)?;
    run_mode(Mode::Lead)
}

/// One page task on a named start page through the lead's Browser helper,
/// with scripted lead and helper turns: only the page agent calls a model.
pub(super) fn run_site(
    start: &std::ffi::OsStr,
    goal: &std::ffi::OsStr,
) -> Result<(), super::ProbeFailure> {
    let (Some(start), Some(goal)) = (start.to_str(), goal.to_str()) else {
        return Err(super::ProbeFailure::Authority);
    };
    if !start.split(' ').all(|start| start.starts_with("https://"))
        || start.len() > 1024
        || goal.is_empty()
        || goal.len() > 600
    {
        return Err(super::ProbeFailure::Authority);
    }
    let scenario = super::acceptance::site_scenario(start, goal);
    let _ = LEAD_SCENARIO.set(scenario);
    run_mode(Mode::Lead)
}

pub(super) fn run_loopback_site() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::LoopbackSite)
}

pub(super) fn run() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::Public)
}

pub(super) fn run_coordinated() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::Coordinated)
}

pub(super) fn run_product() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::ProductIntegration)
}

pub(super) fn run_cancelled() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::CancelCoordinated)
}

pub(super) fn run_agent() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::Agent)
}

pub(super) fn run_money_node() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::MoneyNode)
}

pub(super) fn run_agent_architecture() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentArchitecture)
}

pub(super) fn run_agent_engine_chart() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentEngineChart)
}

pub(super) fn run_agent_explain_mechanism() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentExplainMechanism)
}

pub(super) fn run_agent_code_review() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentCodeReview)
}

pub(super) fn run_agent_explain_rust() -> Result<(), super::ProbeFailure> {
    EXPLAIN_IN_RUST.store(true, std::sync::atomic::Ordering::Relaxed);
    run_mode(Mode::AgentExplainMechanism)
}

pub(super) fn run_agent_concept_comparison() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentConceptComparison)
}

pub(super) fn run_agent_money() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentMoney)
}

pub(super) fn run_agent_collection() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentCollection)
}

pub(super) fn run_agent_trip() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentTrip)
}

pub(super) fn run_agent_airbnb() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentAirbnb)
}

pub(super) fn run_agent_listing() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentListing)
}

static PAGE_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static PAGE_OBJECTIVE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// One agent read of a named public page; its frames are kept locally.
pub(super) fn run_agent_page(url: &std::ffi::OsStr) -> Result<(), super::ProbeFailure> {
    let url = url
        .to_str()
        .filter(|url| {
            (url.starts_with("https://") || url.starts_with("http://")) && url.len() <= 512
        })
        .ok_or(super::ProbeFailure::Authority)?;
    let _ = PAGE_URL.set(url.to_owned());
    let _ = PAGE_OBJECTIVE.set(format!("Read {url} in one browser read and report the places to stay it shows with their displayed prices, as cited findings from that page. Do not search, follow links, book, sign in or interact with verification controls. If the page shows no places to stay, report that honestly."));
    run_mode(Mode::AgentPage)
}

/// One agent read of a named listing page in the shape the Airbnb scenario
/// asks of each listing; its frames are kept locally.
pub(super) fn run_agent_listing_page(url: &std::ffi::OsStr) -> Result<(), super::ProbeFailure> {
    let url = url
        .to_str()
        .filter(|url| url.starts_with("https://") && url.len() <= 512)
        .ok_or(super::ProbeFailure::Authority)?;
    let _ = PAGE_URL.set(url.to_owned());
    let _ = PAGE_OBJECTIVE.set(format!("Read {url} in one browser read and collect this one listing: its name, displayed price, rating, stay type or monthly terms, location or neighborhood, and picture, each as an optional verbatim column. Dates are unspecified, so leave any value the page does not show unknown. Do not search, follow links, book, sign in or interact with verification controls."));
    run_mode(Mode::AgentPage)
}

/// Keeps each settled page's last frame under target and prints only its size.
fn keep_frames(observed: &Mutex<Option<zephium_app::work_runtime::WorkAttemptObserver>>) {
    static KEPT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let directory = std::path::Path::new("target/work-runtime-proof/frames");
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    let pages = observed
        .lock()
        .ok()
        .and_then(|observer| observer.as_ref().map(|observer| observer.pages()))
        .unwrap_or_default();
    for page in pages {
        let Some(frame) = page.frame.filter(|_| !page.live) else {
            continue;
        };
        let index = KEPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::write(
            directory.join(format!("page-{index}.png")),
            frame.png.as_slice(),
        );
        let _ = writeln!(
            std::io::stdout().lock(),
            "agent-work: frame index={index} width={} height={} png_bytes={}",
            frame.width,
            frame.height,
            frame.png.len()
        );
    }
}

pub(super) fn run_agent_details() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentDetails)
}

pub(super) fn run_agent_disclosure() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentDisclosure)
}

pub(super) fn run_agent_scroll() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentScroll)
}

pub(super) fn run_agent_read() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentRead)
}

pub(super) fn run_agent_government() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentGovernment)
}

pub(super) fn run_agent_human_government() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentHumanGovernment)
}

pub(super) fn run_agent_files() -> Result<(), super::ProbeFailure> {
    run_mode(Mode::AgentFiles)
}

/// Sends one retained public turn request again, four at a time, and keeps
/// each text with whether the turn wire admitted it. An effort (none, low,
/// medium) replaces the retained request's reasoning effort.
pub(super) fn replay_agent_turn(
    path: &std::ffi::OsStr,
    count: &std::ffi::OsStr,
    effort: Option<&std::ffi::OsStr>,
) -> Result<(), super::ProbeFailure> {
    use super::ProbeFailure as Error;
    let mut body: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| Error::Authority)?)
            .map_err(|_| Error::Authority)?;
    if let Some(effort) = effort {
        let effort = effort
            .to_str()
            .filter(|effort| matches!(*effort, "none" | "low" | "medium"))
            .ok_or(Error::Authority)?;
        body["reasoning"]["effort"] = serde_json::json!(effort);
    }
    let count: usize = count
        .to_str()
        .and_then(|count| count.parse().ok())
        .filter(|count| (1..=64).contains(count))
        .ok_or(Error::Authority)?;
    std::fs::create_dir_all("target/work-runtime-proof/replay").map_err(|_| Error::Output)?;
    let workers = (0..4)
        .map(|worker| {
            let body = body.clone();
            std::thread::Builder::new()
                .stack_size(PROVIDER_THREAD_STACK_BYTES)
                .spawn(move || -> Result<Vec<(usize, bool)>, Error> {
                    let key = load_macos_probe_openai_credential().map_err(|_| Error::Keychain)?;
                    let transport =
                        AgentProviderTransport::try_new(AgentProviderTransportConfig::STANDARD)
                            .map_err(|_| Error::Runtime)?;
                    let agent = OpenAiWorkAgent::try_new(
                        transport,
                        key,
                        WorkPlanningConfig::try_new(
                            zephium_agent_model_catalog::try_gpt6_luna_provider_exact_call_config(8192)
                                .map_err(|_| Error::Runtime)?,
                            32_768,
                            300_000,
                        )
                        .map_err(|_| Error::Runtime)?,
                    )
                    .map_err(|_| Error::Runtime)?
                    .with_public_response_retention();
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| Error::Runtime)?;
                    let mut results = Vec::new();
                    for index in (worker..count).step_by(4) {
                        let started = Instant::now();
                        let text = runtime.block_on(agent.replay_retained_turn(body.clone()));
                        let _ = writeln!(
                            std::io::stdout().lock(),
                            "replay-agent-turn: index={index} elapsed_ms={}",
                            started.elapsed().as_millis()
                        );
                        let (decoded, faults) = text
                            .as_deref()
                            .map(zephium_agentic::agent_turn_wire_faults)
                            .unwrap_or_default();
                        for fault in &faults {
                            let _ = writeln!(
                                std::io::stdout().lock(),
                                "replay-agent-turn: index={index} wire_error path={} expected={} dropped={}",
                                fault.path,
                                fault.expected,
                                fault.dropped
                            );
                        }
                        if let Ok(text) = &text {
                            let _ = std::fs::write(
                                format!("target/work-runtime-proof/replay/{index}-{decoded}.json"),
                                text,
                            );
                        }
                        results.push((index, text.is_ok() && decoded));
                    }
                    Ok(results)
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::Runtime)?;
    let mut refused = 0;
    for worker in workers {
        for (index, decoded) in worker.join().map_err(|_| Error::Runtime)?? {
            refused += usize::from(!decoded);
            let _ = writeln!(
                std::io::stdout().lock(),
                "replay-agent-turn: index={index} decoded={decoded}"
            );
        }
    }
    let _ = writeln!(
        std::io::stdout().lock(),
        "replay-agent-turn: runs={count} refused={refused}"
    );
    Ok(())
}

fn run_mode(mode: Mode) -> Result<(), super::ProbeFailure> {
    use super::ProbeFailure as Error;
    zephium_app::work_trace::install(Arc::new(|line: std::fmt::Arguments<'_>| {
        if super::work_account::active().is_none()
            && std::env::var_os("ZEPHIUM_EVAL_PUBLIC_EXPORT").is_none()
            && !LEAD_SCENARIO
                .get()
                .is_some_and(|scenario| scenario.name.starts_with("blocks-"))
        {
            let _ = writeln!(std::io::stdout().lock(), "trace: {line}");
        }
    }));
    zephium_app::work_trace::install_record_media(Arc::new(|event| {
        // Account qualification never exports even closed media measurements.
        // Public qualification uses only this typed channel, not free-text traces.
        if super::work_account::active().is_none() {
            let _ = writeln!(std::io::stdout().lock(), "record-media: {event:?}");
        }
    }));
    zephium_engine::configure_macos_public_media_diagnostic(
        std::env::var("ZEPHIUM_PROBE_MEDIA_DIAGNOSTIC").as_deref() == Ok("1")
            && std::env::var_os("ZEPHIUM_EVAL_PUBLIC_EXPORT").is_some(),
        super::work_account::active().is_some(),
    );
    let coordinated = !matches!(mode, Mode::Public | Mode::MoneyNode);
    use std::os::unix::fs::PermissionsExt;
    let data = tempfile::Builder::new()
        .prefix("zephium-durable-work-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .map_err(|_| Error::Runtime)?;
    let store =
        Arc::new(zephium_store::SqliteStore::open(data.path()).map_err(|_| Error::Runtime)?);
    // Match desktop initialization from this probe's actual private Store.
    // Account qualification remains hard text-only and keeps this switch closed.
    initialize_probe_screenshots(
        super::work_account::active().is_some(),
        || store.app_setting_readback("work.screenshots"),
        zephium_app::work_screenshots::initialize,
    );
    // A named profile lets two probe processes share Work's memory.
    let profile = std::env::var("ZEPHIUM_PROBE_PROFILE")
        .ok()
        .and_then(|id| ProfileId::parse(&id))
        .unwrap_or_else(ProfileId::generate);
    let space = SpaceId::generate();
    store.save_session(SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "Durable Work qualification".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: space,
            profile,
            name: "Public research".into(),
            look: Default::default(),
        }],
        active_space: Some(space),
        ..SessionState::default()
    });
    if !store.flush() {
        return Err(Error::Runtime);
    }
    let blocker = super::work_application::seeded_blocker(data.path())?;
    // Credentials never enter Work, model context, diagnostics or serialized reports.
    let planning_key = load_macos_probe_openai_credential().map_err(|_| Error::Keychain)?;
    let browser_keys = (0..if matches!(
        mode,
        Mode::Agent
            | Mode::AgentRead
            | Mode::AgentGovernment
            | Mode::AgentHumanGovernment
            | Mode::AgentScroll
            | Mode::AgentDisclosure
            | Mode::AgentCollection
            | Mode::AgentDetails
            | Mode::AgentTrip
            | Mode::AgentAirbnb
            | Mode::AgentListing
            | Mode::AgentPage
            | Mode::AgentMoney
            | Mode::AgentArchitecture
            | Mode::AgentEngineChart
            | Mode::AgentExplainMechanism
            | Mode::AgentCodeReview
            | Mode::AgentConceptComparison
            | Mode::Lead
    ) {
        6
    } else {
        4
    })
        .map(|_| load_macos_probe_openai_credential())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::Keychain)?;
    let owner_store = store.clone();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let relay = Arc::new(Mutex::new(None::<zephium_app::CallbackHandle>));
    let events = relay.clone();
    let execution_timeout = Duration::from_secs(match mode {
        Mode::LoopbackSite => 840,
        Mode::Agent
        | Mode::AgentRead
        | Mode::AgentGovernment
        | Mode::AgentHumanGovernment
        | Mode::AgentScroll
        | Mode::AgentDisclosure
        | Mode::AgentCollection
        | Mode::AgentDetails
        | Mode::AgentTrip
        | Mode::AgentAirbnb
        | Mode::AgentListing
        | Mode::AgentPage
        | Mode::AgentMoney
        | Mode::AgentArchitecture
        | Mode::AgentEngineChart
        | Mode::AgentExplainMechanism
        | Mode::AgentCodeReview
        | Mode::AgentConceptComparison => 720,
        // Each follow-up of a scenario gets a request's own time.
        Mode::Lead => (860
            * LEAD_SCENARIO
                .get()
                .map_or(1, |scenario| scenario.requests.len().max(1)) as u64)
            .min(3500),
        Mode::Public => 160,
        _ => 240,
    });
    let run = zephium_engine::run_macos_work_application_with_input_probe(
        profile,
        if super::work_account::active().is_some()
            || matches!(mode, Mode::AgentHumanGovernment)
            || (mode == Mode::LoopbackSite && super::work_site::needs_human_presentation())
            // A person sits at the window to sign in or pass a check.
            || (mode == Mode::Lead && std::env::var_os("ZEPHIUM_PROBE_PERSON").is_some())
        {
            zephium_engine::MacosWorkProbeInput::Human
        } else {
            zephium_engine::MacosWorkProbeInput::LifecycleOnly
        },
        execution_timeout + Duration::from_secs(30),
        move |event| {
            if let Ok(relay) = events.lock() {
                if let Some(handle) = relay.as_ref() {
                    handle.dispatch(zephium_app::Command::Engine(event));
                }
            }
        },
        move |engine| {
            let shell = zephium_app::spawn_suspended(
                engine.clone(),
                owner_store.clone(),
                blocker,
                Box::new(|_| {}),
                Arc::new(super::work_application::NoChrome),
                Box::new(|_| {}),
            )
            .map_err(|_| "durable_shell")?;
            *relay.lock().map_err(|_| "durable_events")? = Some(shell.callback_handle());
            if mode == Mode::LoopbackSite {
                super::work_site::install_presence(engine.clone());
            } else {
                super::work_site::install_app_presence(engine.clone());
            }
            let composition = MacosWorkComposition::new(engine, owner_store);
            if !shell.admit_startup() {
                return Err("durable_startup");
            }
            // The headless host has no chrome to send the ordinary bootstrap
            // command. Admission alone starts the actor, not the session.
            if !shell.dispatch(zephium_app::Command::Bootstrap) {
                return Err("durable_bootstrap");
            }
            let worker_handle = shell.clone();
            let worker = std::thread::Builder::new()
                .name("durable-work-qualification".into())
                .stack_size(PROVIDER_THREAD_STACK_BYTES)
                .spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| "durable_tokio")
                        .and_then(|runtime| {
                            runtime.block_on(async {
                                tokio::time::timeout(
                                    execution_timeout,
                                    workflow(
                                        &worker_handle,
                                        &composition,
                                        profile,
                                        space,
                                        planning_key,
                                        browser_keys,
                                        mode,
                                    ),
                                )
                                .await
                                .map_err(|_| "durable_deadline")?
                            })
                        });
                    let _ = result_tx.send(result);
                })
                .map_err(|_| "durable_worker")?;
            let mut worker = Some(worker);
            let mut shutdown = None;
            Ok(Box::new(move |native_failed| {
                if native_failed && shutdown.is_none() {
                    let request =
                        shell.shutdown_with_deadline(Instant::now() + Duration::from_secs(8));
                    shutdown = std::thread::Builder::new()
                        .name("durable-work-shutdown".into())
                        .spawn(move || request.recv_until_deadline())
                        .ok();
                }
                if worker.as_ref().is_some_and(|join| join.is_finished()) {
                    let joined = worker.take().is_some_and(|join| join.join().is_ok());
                    if !joined {
                        return Some(Err("durable_worker_panic"));
                    }
                    if shutdown.is_none() {
                        let request =
                            shell.shutdown_with_deadline(Instant::now() + Duration::from_secs(8));
                        shutdown = std::thread::Builder::new()
                            .name("durable-work-shutdown".into())
                            .spawn(move || request.recv_until_deadline())
                            .ok();
                        if shutdown.is_none() {
                            return Some(Err("durable_shutdown_thread"));
                        }
                    }
                }
                if shutdown.as_ref().is_some_and(|join| join.is_finished()) {
                    let clean = shutdown.take().is_some_and(|join| {
                        matches!(join.join(), Ok(Ok(zephium_app::ShutdownOutcome::Clean)))
                    });
                    return Some(if clean && !native_failed {
                        Ok(())
                    } else {
                        Err("durable_shutdown")
                    });
                }
                None
            }))
        },
    );
    run.map_err(|reason| {
        let _ = writeln!(
            std::io::stdout().lock(),
            "durable-work: host_failure={reason}; content=redacted"
        );
        Error::Runtime
    })?;
    let WorkflowResult { state, failure } = result_rx
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| Error::Runtime)?
        .map_err(|reason| {
            let _ = writeln!(
                std::io::stdout().lock(),
                "durable-work: failure={reason}; content=redacted"
            );
            Error::Runtime
        })?;
    if mode == Mode::LoopbackSite {
        let _ = state;
        return match failure {
            None => Ok(()),
            Some(failure) => {
                let _ = writeln!(std::io::stdout().lock(), "loopback-site: failure={failure}");
                Err(Error::Verification)
            }
        };
    }
    drop(store);
    let reopened = zephium_store::SqliteStore::open(data.path()).map_err(|_| Error::Runtime)?;
    let (tx, rx) = mpsc::sync_channel(1);
    reopened
        .work_document(
            profile,
            WorkRequest::RuntimeRead { id: state.work.id },
            Box::new(move |result| {
                let _ = tx.send(result);
            }),
        )
        .map_err(|_| Error::Runtime)?;
    let WorkReply::Runtime(restored) = rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| Error::Runtime)?
        .map_err(|_| Error::Runtime)?
    else {
        return Err(Error::Runtime);
    };
    if *restored != state {
        return Err(Error::Runtime);
    }
    if mode == Mode::Lead {
        let clean = reopened.shutdown_until(Instant::now() + Duration::from_secs(5))
            == zephium_core::ports::store::StoreShutdownOutcome::Clean;
        return if clean && failure.is_none() {
            Ok(())
        } else {
            Err(Error::Verification)
        };
    }
    if mode == Mode::CancelCoordinated {
        let execution = state.executions.first().ok_or(Error::Runtime)?;
        if failure != Some("child_execution")
            || execution.status != WorkExecutionStatus::Cancelled
            || execution.attempts.len() != 2
            || !execution.artifacts.is_empty()
            || execution.attempts.iter().any(|attempt| {
                !matches!(
                    attempt.status,
                    WorkAttemptStatus::Failed | WorkAttemptStatus::Cancelled
                ) || attempt.usage.is_none()
            })
            || !execution
                .attempts
                .iter()
                .any(|attempt| attempt.status == WorkAttemptStatus::Cancelled)
        {
            return Err(Error::Runtime);
        }
        if reopened.shutdown_until(Instant::now() + Duration::from_secs(5))
            != zephium_core::ports::store::StoreShutdownOutcome::Clean
        {
            return Err(Error::Runtime);
        }
        std::fs::write(
            "target/work-runtime-proof/coordinated-cancelled.json",
            serde_json::to_vec_pretty(&serde_json::json!({
                "cancelled_after_native_model_admission": true,
                "host_shutdown_clean": true,
                "reopened": true,
                "projection": state,
            }))
            .map_err(|_| Error::Output)?,
        )
        .map_err(|_| Error::Output)?;
        writeln!(std::io::stdout().lock(), "durable-work: cancellation=true; original_child_settled=true; primary_synthesis=false; resource_closed=true; reopened=true; content=redacted").map_err(|_| Error::Output)?;
        return Ok(());
    }
    if let Some(failure) = failure {
        std::fs::create_dir_all("target/work-runtime-proof").map_err(|_| Error::Output)?;
        // Reopening failed facts is useful evidence, but never a successful
        // research report. Native host shutdown has already acknowledged its
        // original owners; unknown attempt outcomes remain unknown in Store.
        if reopened.shutdown_until(Instant::now() + Duration::from_secs(5))
            != zephium_core::ports::store::StoreShutdownOutcome::Clean
        {
            return Err(Error::Runtime);
        }
        std::fs::write(
            "target/work-runtime-proof/coordinated-failure.json",
            serde_json::to_vec_pretty(&serde_json::json!({
                "failure": failure,
                "host_shutdown_clean": true,
                "reopened": true,
                "projection": state,
            }))
            .map_err(|_| Error::Output)?,
        )
        .map_err(|_| Error::Output)?;
        let _ = writeln!(std::io::stdout().lock(), "durable-work: failure={failure}; host_shutdown_clean=true; reopened=true; content=redacted");
        return Err(Error::Runtime);
    }
    let expected_status = if mode == Mode::ProductIntegration {
        WorkExecutionStatus::Completed
    } else {
        WorkExecutionStatus::NeedsReview
    };
    if matches!(
        mode,
        Mode::Agent
            | Mode::AgentRead
            | Mode::AgentGovernment
            | Mode::AgentHumanGovernment
            | Mode::AgentScroll
            | Mode::AgentDisclosure
            | Mode::AgentCollection
            | Mode::AgentDetails
            | Mode::AgentTrip
            | Mode::AgentAirbnb
            | Mode::AgentListing
            | Mode::AgentPage
            | Mode::AgentMoney
            | Mode::AgentArchitecture
            | Mode::AgentEngineChart
            | Mode::AgentExplainMechanism
            | Mode::AgentCodeReview
            | Mode::AgentConceptComparison
    ) {
        let execution = &state.executions[0];
        let counts = |kind: &str| {
            execution
                .steps
                .iter()
                .filter(|step| {
                    serde_json::to_value(&step.kind)
                        .ok()
                        .and_then(|value| value["kind"].as_str().map(|k| k == kind))
                        .unwrap_or(false)
                })
                .count()
        };
        let _ = writeln!(
            std::io::stdout().lock(),
            "agent-work: status={:?}; steps={}; turns={}; searches={}; reads={}; discoveries={}; publishes={}; asks={}; finished={}; artifacts={}; sources={}; usage={:?}; content=redacted",
            execution.status,
            execution.steps.len(),
            counts("turn"),
            counts("search"),
            counts("read"),
            counts("discover"),
            counts("publish"),
            counts("ask"),
            counts("finish"),
            execution.artifacts.len(),
            execution.provider_evidence.len(),
            execution.attempts.first().and_then(|attempt| attempt.usage),
        );
        if mode == Mode::AgentPage {
            // Closed relations only: whether the requested page was read as
            // asked or at where it redirected, and which read the records cite.
            let reads: Vec<(&str, WorkStepStatus, bool)> = execution
                .steps
                .iter()
                .filter_map(|step| match &step.kind {
                    WorkStepKindV1::Read { url, .. } => {
                        Some((url.as_str(), step.status, !step.artifacts.is_empty()))
                    }
                    _ => None,
                })
                .collect();
            let requested = PAGE_URL
                .get()
                .map(|url| url.replacen("http://", "https://", 1))
                .unwrap_or_default();
            let cited: Vec<&str> = reads
                .iter()
                .filter(|(_, _, records)| *records)
                .map(|(url, _, _)| *url)
                .collect();
            let _ = writeln!(
                std::io::stdout().lock(),
                "agent-work: page_reads={} redirected_reads={} cited_reads={} cited_requested={} cited_last_read={} content=redacted",
                reads.len(),
                reads
                    .iter()
                    .filter(|(_, status, _)| *status == WorkStepStatus::Failed)
                    .count(),
                cited.len(),
                cited.iter().any(|url| *url == requested),
                reads.last().is_some_and(|(url, status, _)| {
                    *status == WorkStepStatus::Succeeded && cited.contains(url)
                }),
            );
        }
    }
    if state.executions[0].status != expected_status || state.executions[0].artifacts.is_empty() {
        return Err(Error::Runtime);
    }
    let collection_accepted = !matches!(mode, Mode::AgentCollection | Mode::AgentDetails) || state.executions[0].artifacts.iter().any(|artifact| {
        (mode == Mode::AgentDetails || state.executions[0].steps.iter().any(|step| step.artifacts.contains(&artifact.id) && matches!(&step.kind, WorkStepKindV1::Read { collection: Some(_), .. } | WorkStepKindV1::Discover { collection: Some(_), .. })))
            && matches!(&artifact.data, zephium_core::work::artifact::WorkArtifactDataV1::ComparisonMatrix { subjects, cells, .. }
                if subjects.len() == 3 && subjects.iter().all(|subject| subject.homepage.is_some() && !subject.image_candidates.is_empty()) && cells.len() == 3 && cells.iter().all(|row| row.iter().any(|cell|
                    matches!(&cell.value, zephium_core::work::artifact::WorkCellValue::Text { text } if text.contains('$')) && !cell.evidence.is_empty())))
            && !artifact.evidence.is_empty()
    });
    let collection_accepted = collection_accepted
        && (!matches!(mode, Mode::AgentCollection | Mode::AgentDetails)
            || research_accepted(&state.executions[0]))
        && (mode != Mode::AgentDetails || {
            let mut pages = std::collections::BTreeSet::new();
            for step in &state.executions[0].steps {
                if let WorkStepKindV1::Read { url, .. } = &step.kind {
                    if step.status == WorkStepStatus::Succeeded
                        && state.executions[0].artifacts.iter().any(|artifact| {
                            step.artifacts.contains(&artifact.id)
                                && has_product_specification(&artifact.data)
                        })
                        && url.starts_with("https://www.lego.com/en-us/product/")
                    {
                        pages.insert(url.as_str());
                    }
                }
            }
            pages.len() >= 3
        });
    let money_accepted = !matches!(mode, Mode::AgentMoney | Mode::MoneyNode) || state.executions[0].artifacts.iter().any(|artifact| {
        artifact.title == "Observed product prices" && matches!(&artifact.data, zephium_core::work::artifact::WorkArtifactDataV1::ComparisonMatrix { subjects, cells, .. } if subjects.len() == 1 && subjects[0].name == "Acme Circles T-Shirt" && !subjects[0].image_candidates.is_empty() && subjects.len() == cells.len() && cells.iter().all(|row| row.first().is_some_and(|cell| matches!(&cell.value, zephium_core::work::artifact::WorkCellValue::Money { currency, observed_at: None, .. } if currency == "USD") && !cell.evidence.is_empty())))
    });
    let airbnb_reads: std::collections::BTreeSet<_> = state.executions[0]
        .steps
        .iter()
        .filter_map(|step| match &step.kind {
            WorkStepKindV1::Read { url, .. } if step.status == WorkStepStatus::Succeeded => {
                airbnb_page(url)
            }
            _ => None,
        })
        .collect();
    let airbnb_listing_reads = airbnb_reads
        .iter()
        .filter(|page| matches!(page, AirbnbPage::Listing(_)))
        .count();
    let travel_accepted = match mode {
        Mode::AgentTrip => !airbnb_reads.is_empty() && research_accepted(&state.executions[0]),
        Mode::AgentListing => airbnb_listing_reads == 1,
        Mode::AgentAirbnb => {
            airbnb_listing_reads >= 3
                && state.executions[0].artifacts.iter().any(|artifact| {
                    let zephium_core::work::artifact::WorkArtifactDataV1::ComparisonMatrix {
                        subjects,
                        ..
                    } = &artifact.data
                    else {
                        return false;
                    };
                    let pages: std::collections::BTreeSet<_> = subjects
                        .iter()
                        .filter_map(|subject| subject.homepage.as_deref().and_then(airbnb_page))
                        .filter(|page| {
                            matches!(page, AirbnbPage::Listing(_)) && airbnb_reads.contains(page)
                        })
                        .collect();
                    subjects.len() == 3 && pages.len() == 3 && !artifact.evidence.is_empty()
                })
                && research_accepted(&state.executions[0])
        }
        _ => true,
    };
    let design_accepted = match mode {
        Mode::AgentArchitecture => architecture_accepted(&state.executions[0]),
        Mode::AgentExplainMechanism => mechanism_accepted(&state.executions[0]),
        Mode::AgentCodeReview => code_review_accepted(&state.executions[0]),
        Mode::AgentConceptComparison => concept_comparison_accepted(&state.executions[0]),
        _ => true,
    };
    let chart_accepted =
        mode != Mode::AgentEngineChart || engine_chart_accepted(&state.executions[0]);
    if mode == Mode::AgentListing {
        // The read asked for ?adults=1; only the page's canonical address,
        // not the admitted one, cites the listing without that query.
        let canonical = "https://www.airbnb.com/rooms/23813739";
        let cited = state.executions[0].artifacts.iter().any(|artifact| {
            matches!(&artifact.data, zephium_core::work::artifact::WorkArtifactDataV1::ComparisonMatrix { subjects, cells, .. }
                if subjects.iter().any(|subject| subject.homepage.as_deref() == Some(canonical))
                    || cells.iter().flatten().any(|cell| matches!(&cell.value,
                        zephium_core::work::artifact::WorkCellValue::Text { text } if text == canonical)))
        });
        let _ = writeln!(
            std::io::stdout().lock(),
            "listing_address canonical_cited={cited}"
        );
    }
    if matches!(
        mode,
        Mode::AgentTrip | Mode::AgentAirbnb | Mode::AgentListing
    ) {
        let _ = writeln!(
            std::io::stdout().lock(),
            "travel_qualification airbnb_reads={} listing_reads={} accepted={travel_accepted}",
            airbnb_reads.len(),
            airbnb_listing_reads
        );
    }
    let mut evidence = Vec::new();
    for link in state.executions[0]
        .artifacts
        .iter()
        .flat_map(|artifact| &artifact.evidence)
    {
        let (tx, rx) = mpsc::sync_channel(1);
        reopened
            .work_document(
                profile,
                WorkRequest::ReadEvidence {
                    id: state.work.id,
                    link: link.clone(),
                },
                Box::new(move |result| {
                    let _ = tx.send(result);
                }),
            )
            .map_err(|_| Error::Runtime)?;
        let WorkReply::Evidence(preview) = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| Error::Runtime)?
            .map_err(|_| Error::Runtime)?
        else {
            return Err(Error::Runtime);
        };
        evidence.push(preview);
    }
    let money_accepted = money_accepted && (!matches!(mode, Mode::AgentMoney | Mode::MoneyNode) || state.executions[0].artifacts.iter().filter(|artifact| artifact.title == "Observed product prices").all(|artifact| artifact.evidence.iter().all(|link| evidence.iter().any(|preview| preview.link == *link && preview.origin == "https://demo.vercel.store/" && !preview.truncated && matches!(preview.source, zephium_core::work::artifact::WorkEvidenceSourceV1::NativeExtraction)))));
    let output = std::path::Path::new("target/work-runtime-proof");
    std::fs::create_dir_all(output).map_err(|_| Error::Output)?;
    std::fs::write(
        output.join(if mode == Mode::MoneyNode {
            if money_accepted {
                "money-node-run.json"
            } else {
                "money-node-incomplete.json"
            }
        } else if mode == Mode::AgentMoney {
            if money_accepted {
                "agent-money-run.json"
            } else {
                "agent-money-incomplete.json"
            }
        } else if mode == Mode::AgentArchitecture {
            if design_accepted {
                "agent-architecture-run.json"
            } else {
                "agent-architecture-incomplete.json"
            }
        } else if mode == Mode::AgentExplainMechanism {
            if design_accepted {
                "agent-explain-mechanism-run.json"
            } else {
                "agent-explain-mechanism-incomplete.json"
            }
        } else if mode == Mode::AgentCodeReview {
            if design_accepted {
                "agent-code-review-run.json"
            } else {
                "agent-code-review-incomplete.json"
            }
        } else if mode == Mode::AgentConceptComparison {
            if design_accepted {
                "agent-concept-comparison-run.json"
            } else {
                "agent-concept-comparison-incomplete.json"
            }
        } else if mode == Mode::AgentEngineChart {
            if chart_accepted {
                "agent-engine-chart-run.json"
            } else {
                "agent-engine-chart-incomplete.json"
            }
        } else if mode == Mode::AgentTrip {
            "agent-trip-run.json"
        } else if mode == Mode::AgentAirbnb {
            "agent-airbnb-run.json"
        } else if mode == Mode::AgentListing {
            "agent-airbnb-listing-run.json"
        } else if mode == Mode::AgentDetails {
            if collection_accepted {
                "agent-details-run.json"
            } else {
                "agent-details-incomplete.json"
            }
        } else if !collection_accepted {
            "agent-collection-incomplete.json"
        } else if mode == Mode::AgentCollection {
            "agent-collection-run.json"
        } else if mode == Mode::AgentDisclosure {
            "agent-disclosure-run.json"
        } else if mode == Mode::AgentScroll {
            "agent-scroll-run.json"
        } else if matches!(mode, Mode::AgentGovernment | Mode::AgentHumanGovernment) {
            "agent-government-run.json"
        } else if mode == Mode::AgentRead {
            "agent-read-run.json"
        } else if mode == Mode::Agent {
            "agent-run.json"
        } else if mode == Mode::ProductIntegration {
            "product-integration.json"
        } else if coordinated {
            "coordinated-research.json"
        } else {
            "public-research.json"
        }),
        serde_json::to_vec_pretty(&serde_json::json!({
            "fixed_collection_assignment": mode == Mode::AgentMoney,
            "money_accepted": money_accepted,
            "collection_accepted": collection_accepted,
            "travel_accepted": travel_accepted,
            "design_accepted": design_accepted,
            "chart_accepted": chart_accepted,
            "projection": state,
            "historical_evidence": evidence,
        }))
        .map_err(|_| Error::Output)?,
    )
    .map_err(|_| Error::Output)?;
    if reopened.shutdown_until(Instant::now() + Duration::from_secs(5))
        != zephium_core::ports::store::StoreShutdownOutcome::Clean
    {
        return Err(Error::Runtime);
    }
    let _ = writeln!(
        std::io::stdout().lock(),
        "agent-acceptance: collection={collection_accepted} money={money_accepted} travel={travel_accepted} design={design_accepted} chart={chart_accepted}"
    );
    if !collection_accepted
        || !money_accepted
        || !travel_accepted
        || !design_accepted
        || !chart_accepted
    {
        return Err(Error::Runtime);
    }
    writeln!(std::io::stdout().lock(), "durable-work: fixed_collection_assignment={}; native_browser=true; artifacts={}; resource_closed=true; reopened=true; semantic_status={:?}; content=redacted", mode == Mode::AgentMoney, state.executions[0].artifacts.len(), state.executions[0].status).map_err(|_| Error::Output)?;
    Ok(())
}

async fn workflow(
    handle: &zephium_app::Handle,
    composition: &MacosWorkComposition,
    profile: ProfileId,
    space: SpaceId,
    planning_key: zephium_agentic::AgentProviderCredential,
    mut browser_keys: Vec<zephium_agentic::AgentProviderCredential>,
    mode: Mode,
) -> Result<WorkflowResult, &'static str> {
    let coordinated = !matches!(mode, Mode::Public | Mode::MoneyNode);
    let binding = loop {
        let selected = handle.work_profile_binding();
        let answer = loop {
            if let Some(answer) = selected.try_recv() {
                break answer;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        match answer {
            zephium_app::AgentWorkProfileReadiness::Ready(binding)
                if binding.profile() == profile =>
            {
                break binding
            }
            zephium_app::AgentWorkProfileReadiness::PolicyPending(_)
            | zephium_app::AgentWorkProfileReadiness::PolicyMissing => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            zephium_app::AgentWorkProfileReadiness::ProfileMissing => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            zephium_app::AgentWorkProfileReadiness::PolicyFailed => {
                return Err("profile_policy_failed")
            }
            _ => return Err("profile_not_ready"),
        }
    };
    if mode == Mode::LoopbackSite {
        let _ = planning_key;
        return super::work_site::workflow(handle, composition, profile, binding, browser_keys)
            .await;
    }
    if mode == Mode::Lead {
        let _ = planning_key;
        let scenario = LEAD_SCENARIO.get().ok_or("lead_scenario")?;
        return super::acceptance::lead_workflow(
            handle,
            composition,
            profile,
            space,
            binding,
            browser_keys,
            scenario,
        )
        .await;
    }
    if matches!(
        mode,
        Mode::Agent
            | Mode::AgentRead
            | Mode::AgentGovernment
            | Mode::AgentHumanGovernment
            | Mode::AgentScroll
            | Mode::AgentDisclosure
            | Mode::AgentCollection
            | Mode::AgentDetails
            | Mode::AgentTrip
            | Mode::AgentAirbnb
            | Mode::AgentListing
            | Mode::AgentPage
            | Mode::AgentMoney
            | Mode::AgentArchitecture
            | Mode::AgentEngineChart
            | Mode::AgentExplainMechanism
            | Mode::AgentCodeReview
            | Mode::AgentConceptComparison
            | Mode::AgentFiles
    ) {
        return agent_workflow(
            handle,
            composition,
            profile,
            binding,
            planning_key,
            browser_keys,
            mode,
        )
        .await;
    }
    let created = handle
        .work_authoring_command(
            profile,
            WorkAuthoringCommandV1 {
                version: 1,
                command: WorkCommandId::generate(),
                intent: WorkAuthoringIntent::Create {
                    objective: if mode == Mode::MoneyNode {
                        AGENT_MONEY_OBJECTIVE
                    } else if coordinated {
                        COORDINATED_OBJECTIVE
                    } else {
                        OBJECTIVE
                    }
                    .into(),
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
        .map_err(|_| "planning_transport")?;
    let synthesis = if coordinated {
        Some(
            OpenAiWorkSynthesizer::try_new(
                transport.clone(),
                browser_keys.pop().ok_or("synthesis_key")?,
                WorkPlanningConfig::try_new(
                    zephium_agent_model_catalog::try_gpt6_luna_provider_exact_call_config(4096)
                        .map_err(|_| "synthesis_model")?,
                    8192,
                    10_000,
                )
                .map_err(|_| "synthesis_limits")?,
            )
            .map_err(|_| "synthesis_provider")?
            .with_public_response_retention(),
        )
    } else {
        None
    };
    let planner = OpenAiWorkPlanner::try_new(
        transport,
        planning_key,
        WorkPlanningConfig::try_new(
            zephium_agent_model_catalog::try_gpt6_luna_provider_exact_call_config(4096)
                .map_err(|_| "model_config")?,
            8192,
            100_000,
        )
        .map_err(|_| "planning_limits")?,
    )
    .map_err(|_| "planner")?;
    let result = WorkPlanningService::new(handle.clone(), Arc::new(planner))
        .plan_request(
            profile,
            WorkPlanRequestV1 {
                version: 1,
                work,
                expected_revision: created.applied_revision,
                context: None,
            },
        )
        .await;
    let WorkPlanningOutcomeV1::Settled { response } = result.outcome else {
        return Err("planning");
    };
    let WorkReplyV1::Snapshot { snapshot: planned } = response.reply else {
        return Err("plan_persistence");
    };
    let plan = planned.plan.as_ref().ok_or("clarification_required")?;
    std::fs::create_dir_all("target/work-runtime-proof").map_err(|_| "plan_report")?;
    std::fs::write(
        if coordinated {
            "target/work-runtime-proof/coordinated-plan.json"
        } else {
            "target/work-runtime-proof/public-plan.json"
        },
        serde_json::to_vec_pretty(&planned).map_err(|_| "plan_report")?,
    )
    .map_err(|_| "plan_report")?;
    if plan.draft.nodes.len() > browser_keys.len() {
        return Err("qualification_node_limit");
    }
    if mode == Mode::MoneyNode
        && (plan.draft.nodes.len() != 1 || plan.draft.nodes[0].outputs.len() != 1)
    {
        return Err("money_node_shape");
    }
    let primary = if coordinated {
        if plan.draft.nodes.len() != 2
            || plan.draft.nodes.iter().any(|n| {
                n.outputs.len() != 1
                    || n.outputs[0].review != WorkOutputReview::SourceMappedNeedsReview
            })
        {
            return Err("qualification_primary_shape");
        }
        Some(
            plan.draft
                .nodes
                .iter()
                .find(|n| n.dependencies.len() == 1)
                .ok_or("qualification_primary_dependency")?
                .id,
        )
    } else {
        None
    };
    let limits = WorkExecutionLimits {
        model_tokens: 400_000,
        cost_micro_usd: 500_000,
        operations: 48,
        timeout_seconds: 120,
        max_workers: 1,
    };
    let limits = if coordinated {
        WorkExecutionLimits {
            model_tokens: 800_000,
            cost_micro_usd: 1_000_000,
            operations: 96,
            timeout_seconds: 180,
            max_workers: 2,
        }
    } else {
        limits
    };
    let driver = WorkExecutionService::new(handle.clone());
    let preview = driver
        .prepare_public_approval(
            profile,
            WorkApprovalRequestV1 {
                version: 1,
                work,
                expected_revision: planned.revision,
                limits,
                primary,
                scope: if mode == Mode::MoneyNode {
                    WorkBrowseScope {
                        start_url:
                            "https://demo.vercel.store/product/acme-geometric-circles-t-shirt"
                                .into(),
                        routes: vec![WorkBrowseRoute {
                            origin: "https://demo.vercel.store".into(),
                            path_prefix: "/product/".into(),
                        }],
                        max_hops: 1,
                    }
                } else {
                    WorkBrowseScope {
                        start_url: "https://sqlite.org/docs.html".into(),
                        routes: ["https://sqlite.org", "https://www.sqlite.org"]
                            .into_iter()
                            .map(|origin| WorkBrowseRoute {
                                origin: origin.into(),
                                path_prefix: "/".into(),
                            })
                            .collect(),
                        max_hops: 8,
                    }
                },
            },
        )
        .await
        .map_err(|error| {
            let _ = writeln!(
                std::io::stdout().lock(),
                "durable-work: approval_preview={error:?}; content=redacted"
            );
            "approval_preview"
        })?;
    let WorkReplyV1::ApprovalDraft { spec, .. } = preview.reply else {
        return Err("approval_preview_reply");
    };
    // Explicit qualification approval of this exact model-produced revision,
    // limited to the public scope and budget requested above. No remote writes.
    let request = handle
        .work_command(
            profile,
            zephium_ipc::work::WorkCommandV1 {
                version: 1,
                work,
                expected_revision: planned.revision,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::Approve { spec },
            },
        )
        .map_err(|_| "approval_admission")?;
    let WorkReply::RuntimeCommand {
        projection: approved,
        receipt,
    } = request.await.map_err(|_| "approval_persistence")?.reply
    else {
        return Err("approval_reply");
    };
    let mut state = *approved;
    let mut completed = Vec::new();
    let mut browser_keys = browser_keys.into_iter();
    if matches!(mode, Mode::Coordinated | Mode::ProductIntegration) {
        state = driver
            .execute_request(
                profile,
                WorkStartRequestV1 {
                    version: 1,
                    work,
                    expected_revision: state.work.revision,
                    execution: receipt.execution,
                },
                synthesis.as_ref().ok_or("primary_model")?,
                |attempt| {
                    let key = browser_keys.next();
                    async move {
                        let key = key.ok_or(WorkError::Capacity)?;
                        composition
                            .execute_public_node_owned(
                                &handle.callback_handle(),
                                attempt,
                                browser_settings(binding, key),
                            )
                            .await
                    }
                },
                |_| {},
            )
            .await
            .map_err(|_| "product_execution")?;
        std::fs::write(
            "target/work-runtime-proof/coordinated-attempt.json",
            serde_json::to_vec_pretty(&state).map_err(|_| "primary_report")?,
        )
        .map_err(|_| "primary_report")?;
        if state.executions[0].status != WorkExecutionStatus::NeedsReview
            || state.executions[0].artifacts.len() != 2
        {
            return Ok(WorkflowResult {
                state,
                failure: Some("product_execution"),
            });
        }
        if mode == Mode::ProductIntegration {
            state = review_product_results(handle, profile, state).await?;
        }
        writeln!(std::io::stdout().lock(), "durable-work: product_driver=true; original_parent=true; original_child=true; structured_handoff=true; primary_synthesis=true; content=redacted").map_err(|_| "primary_progress")?;
        return Ok(WorkflowResult {
            state,
            failure: None,
        });
    }
    if let Some(primary) = primary {
        let attempt = WorkRuntimeService::new(handle.clone())
            .begin_node(
                profile,
                work,
                state.work.revision,
                receipt.execution,
                primary,
            )
            .await
            .map_err(|_| "primary_admission")?;
        let mut coordinator = attempt
            .coordinate()
            .await
            .map_err(|_| "primary_ownership")?;
        let child = plan
            .draft
            .nodes
            .iter()
            .find(|n| n.id != primary)
            .ok_or("child_node")?
            .id;
        let credential = browser_keys.next().ok_or("child_key")?;
        let child_result = coordinator
            .execute_child(child, |attempt| async {
                let observer = attempt.observer();
                let callback = handle.callback_handle();
                let operation = composition.execute_public_node_owned(
                    &callback,
                    attempt,
                    browser_settings(binding, credential),
                );
                if mode != Mode::CancelCoordinated {
                    return operation.await;
                }
                tokio::pin!(operation);
                loop {
                    tokio::select! {
                        result = &mut operation => return result,
                        _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                    }
                    if observer.latest().is_some_and(|signal| {
                        signal.activity == zephium_ipc::work::WorkActivityV1::Planning
                    }) {
                        let WorkReply::Runtime(current) =
                            handle.work_projection(profile, work)?.await?.reply
                        else {
                            return Err(WorkError::Invalid);
                        };
                        handle
                            .work_command(
                                profile,
                                zephium_ipc::work::WorkCommandV1 {
                                    version: 1,
                                    work,
                                    expected_revision: current.work.revision,
                                    command: WorkCommandId::generate(),
                                    intent: WorkRuntimeIntent::Cancel {
                                        execution: receipt.execution,
                                        intervention: None,
                                    },
                                },
                            )?
                            .await?;
                        // Keep polling the original adapter to settle native,
                        // provider and Store ownership after durable stop intent.
                        return operation.await;
                    }
                }
            })
            .await
            .map_err(|error| {
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "durable-work: child_error={error:?}; content=redacted"
                );
                "child_execution"
            });
        let state = coordinator
            .finish(synthesis.as_ref().ok_or("primary_model")?)
            .await
            .map_err(|_| "primary_synthesis")?
            .into_projection();
        // Keep failed qualification facts too. This snapshot does not claim
        // native closure or Store reopen; only the final report below does.
        // Finish consumes the poisoned original coordinator without another
        // model call when a child fails, retaining any unknown child charge.
        std::fs::write(
            "target/work-runtime-proof/coordinated-attempt.json",
            serde_json::to_vec_pretty(&state).map_err(|_| "primary_report")?,
        )
        .map_err(|_| "primary_report")?;
        if let Err(failure) = child_result {
            return Ok(WorkflowResult {
                state,
                failure: Some(failure),
            });
        }
        if state.executions[0].attempts.len() != 2 || state.executions[0].artifacts.len() != 2 {
            return Ok(WorkflowResult {
                state,
                failure: Some("primary_publication"),
            });
        }
        writeln!(std::io::stdout().lock(), "durable-work: original_parent=true; original_child=true; structured_handoff=true; primary_synthesis=true; content=redacted").map_err(|_| "primary_progress")?;
        return Ok(WorkflowResult {
            state,
            failure: None,
        });
    }
    // Bounded sequential qualification dispatch of the model's exact DAG.
    // This does not claim primary/child model delegation or a production scheduler.
    while completed.len() < plan.draft.nodes.len() {
        let node = plan
            .draft
            .nodes
            .iter()
            .find(|node| {
                !completed.contains(&node.id)
                    && node.dependencies.iter().all(|id| completed.contains(id))
            })
            .ok_or("dependency_readiness")?
            .id;
        let attempt = WorkRuntimeService::new(handle.clone())
            .begin_node(profile, work, state.work.revision, receipt.execution, node)
            .await
            .map_err(|_| "attempt_admission")?;
        writeln!(std::io::stdout().lock(), "durable-work: model_plan=true; exact_approval=true; starting_native=true; content=redacted").map_err(|_| "progress_output")?;
        let settings = browser_settings(
            binding,
            browser_keys.next().ok_or("qualification_key_limit")?,
        );
        state = if mode == Mode::MoneyNode {
            composition
                .execute_collection_node_owned(
                    &handle.callback_handle(),
                    attempt,
                    settings,
                    money_schema().map_err(|_| "money_schema")?,
                )
                .await
                .map(|settlement| settlement.into_projection())
        } else {
            composition
                .execute_public_node(&handle.callback_handle(), attempt, settings)
                .await
        }
        .map_err(|error| {
            let _ = writeln!(
                std::io::stdout().lock(),
                "durable-work: execution_error={error:?}; content=redacted"
            );
            "native_execution"
        })?;
        if !state.executions[0]
            .attempts
            .iter()
            .any(|attempt| attempt.node == node && attempt.status == WorkAttemptStatus::Succeeded)
        {
            return Err("node_not_successful");
        }
        completed.push(node);
    }
    Ok(WorkflowResult {
        state,
        failure: None,
    })
}

async fn review_product_results(
    handle: &zephium_app::Handle,
    profile: ProfileId,
    mut state: WorkRuntimeProjection,
) -> Result<WorkRuntimeProjection, &'static str> {
    let original = state.executions[0].artifacts.clone();
    let execution = state.executions[0].id;
    let work = state.work.id;
    let apply = |command| async move {
        let reply = handle
            .work_command(profile, command)
            .map_err(|_| "review_admission")?
            .response(profile)
            .await;
        let WorkReplyV1::ExecutionApplied { projection, .. } = reply.reply else {
            return Err("review_persistence");
        };
        Ok::<_, &'static str>(*projection)
    };
    for artifact in &original {
        let command = WorkCommandV1 {
            version: 1,
            work,
            expected_revision: state.work.revision,
            command: WorkCommandId::generate(),
            intent: WorkRuntimeIntent::ReviewArtifact {
                execution,
                artifact: artifact.id,
                decision: WorkArtifactDecision::Accepted,
            },
        };
        state = apply(command.clone()).await?;
        if apply(command).await? != state {
            return Err("review_replay");
        }
    }
    if state.executions[0].status != WorkExecutionStatus::Completed {
        return Err("review_completion");
    }
    let primary = original.last().ok_or("review_artifact")?;
    let WorkArtifactDataV1::Document { paragraphs, .. } = &primary.data else {
        return Err("review_document");
    };
    let mut paragraphs = paragraphs.clone();
    paragraphs.push("Review note: preserve the cited sources and any stated evidence limitations when using this result.".into());
    let previous_revision = state.work.revision;
    state = apply(WorkCommandV1 {
        version: 1,
        work,
        expected_revision: previous_revision,
        command: WorkCommandId::generate(),
        intent: WorkRuntimeIntent::EditArtifact {
            execution,
            artifact: primary.id,
            data: WorkArtifactDataV1::Document {
                paragraphs,
                formatted: None,
            },
            evidence: primary.evidence.clone(),
        },
    })
    .await?;
    if state.executions[0].status != WorkExecutionStatus::NeedsReview {
        return Err("edit_review_reset");
    }
    let review = WorkRuntimeIntent::ReviewArtifact {
        execution,
        artifact: primary.id,
        decision: WorkArtifactDecision::Accepted,
    };
    let stale = handle
        .work_command(
            profile,
            WorkCommandV1 {
                version: 1,
                work,
                expected_revision: previous_revision,
                command: WorkCommandId::generate(),
                intent: review.clone(),
            },
        )
        .map_err(|_| "stale_review_admission")?
        .response(profile)
        .await;
    if !matches!(
        stale.reply,
        WorkReplyV1::Error {
            error: WorkFailureV1::Conflict
        }
    ) {
        return Err("stale_review_not_refused");
    }
    state = apply(WorkCommandV1 {
        version: 1,
        work,
        expected_revision: state.work.revision,
        command: WorkCommandId::generate(),
        intent: review,
    })
    .await?;
    if state.executions[0].status != WorkExecutionStatus::Completed
        || state.executions[0].artifacts != original
    {
        return Err("review_original_changed");
    }
    writeln!(std::io::stdout().lock(), "durable-work: product_commands=true; accepted=true; edited=true; original_artifact_immutable=true; stale_review_refused=true; replay_idempotent=true; content=redacted").map_err(|_| "review_report")?;
    Ok(state)
}

/// A research comparison that asks for numbers the sources rarely give: at
/// most one findings object per turn, no chart of all-zero or unknown points,
/// either a knowledge-marked chart with a basis or a finding that comparable
/// numbers are unavailable, at most eight reads, and no claim repeated across
/// findings objects.
fn engine_chart_accepted(execution: &WorkExecutionFact) -> bool {
    use zephium_core::work::artifact::WorkArtifactDataV1 as Data;
    let kind_of = |id: &WorkArtifactId| {
        execution
            .artifacts
            .iter()
            .find(|artifact| artifact.id == *id)
            .map(|artifact| zephium_core::work::agent::artifact_kind(&artifact.data))
    };
    let mut per_turn = std::collections::BTreeMap::<u8, usize>::new();
    for step in &execution.steps {
        if matches!(step.kind, WorkStepKindV1::Publish) {
            *per_turn.entry(step.turn).or_default() += step
                .artifacts
                .iter()
                .filter(|id| kind_of(id) == Some("findings"))
                .count();
        }
    }
    let findings_max = per_turn.values().copied().max().unwrap_or(0);
    let empty_charts = execution
        .artifacts
        .iter()
        .filter(|artifact| {
            matches!(&artifact.data, Data::Chart { series, .. }
            if series.iter().flat_map(|series| &series.points).all(|point| {
                let value = point.value.trim();
                !value.bytes().any(|b| b.is_ascii_digit())
                    || value.parse::<f64>().is_ok_and(|n| n == 0.0)
            }))
        })
        .count();
    let knowledge_chart = execution.artifacts.iter().any(|artifact| {
        matches!(&artifact.data, Data::Chart { basis: Some(_), general_knowledge, .. }
            if *general_knowledge || artifact.general_knowledge)
    });
    let collapse = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let claims: Vec<(usize, String)> = execution
        .artifacts
        .iter()
        .enumerate()
        .flat_map(|(index, artifact)| match &artifact.data {
            Data::Findings { items, .. } => items
                .iter()
                .map(|item| (index, collapse(&item.claim)))
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    let answer = AnswerFacts::of(execution);
    let answer_text: Vec<String> = execution
        .artifacts
        .iter()
        .filter_map(|artifact| match &artifact.data {
            Data::Answer { markdown } => Some(collapse(markdown)),
            _ => None,
        })
        .collect();
    let unavailable_finding = claims
        .iter()
        .map(|(_, claim)| claim)
        .chain(&answer_text)
        .any(|claim| {
            let claim = claim.to_lowercase();
            [
                "no comparable",
                "not comparable",
                "unavailable",
                "lack",
                "no published",
                "do not give",
                "don't give",
                "no measured",
                "not report",
            ]
            .iter()
            .any(|phrase| claim.contains(phrase))
                && ["number", "benchmark", "figure", "metric", "measure", "data"]
                    .iter()
                    .any(|noun| claim.contains(noun))
        });
    let duplicate_claims = claims
        .iter()
        .enumerate()
        .filter(|(at, (index, claim))| {
            claims[..*at]
                .iter()
                .any(|(earlier, text)| earlier != index && text == claim)
        })
        .count();
    let reads = execution
        .steps
        .iter()
        .filter(|step| matches!(step.kind, WorkStepKindV1::Read { .. }))
        .count();
    let elapsed_ms = AGENT_ELAPSED_MS.load(std::sync::atomic::Ordering::Relaxed);
    let mut kinds: Vec<_> = execution
        .artifacts
        .iter()
        .map(|artifact| zephium_core::work::agent::artifact_kind(&artifact.data))
        .collect();
    kinds.sort_unstable();
    let accepted = findings_max <= 1
        && answer.accepted()
        && empty_charts == 0
        && (knowledge_chart || unavailable_finding)
        && reads <= 8
        && duplicate_claims == 0;
    let _ = writeln!(
        std::io::stdout().lock(),
        "chart_qualification {answer} findings_max_per_turn={findings_max} empty_charts={empty_charts} knowledge_chart={knowledge_chart} unavailable_finding={unavailable_finding} duplicate_claims={duplicate_claims} reads={reads} elapsed_ms={elapsed_ms} kinds={} accepted={accepted}",
        kinds.join(","),
    );
    accepted
}

/// A making request answered from knowledge: exactly one answer beside a
/// diagram of at least six nodes, five edges and one vendor host and a table,
/// each marked knowledge; at most two reads, none failed, and the loop under
/// its deadline. Findings and a checklist are counted, not required.
fn architecture_accepted(execution: &WorkExecutionFact) -> bool {
    use zephium_core::work::artifact::WorkArtifactDataV1 as Data;
    let known = |test: &dyn Fn(&Data) -> bool| {
        execution
            .artifacts
            .iter()
            .any(|artifact| artifact.general_knowledge && test(&artifact.data))
    };
    let diagram = known(&|data| {
        matches!(data, Data::Diagram { nodes, edges, .. }
            if nodes.len() >= 6 && edges.len() >= 5 && nodes.iter().any(|node| node.vendor.is_some()))
    });
    let table = known(&|data| matches!(data, Data::Table { .. }));
    let findings = known(&|data| matches!(data, Data::Findings { .. }));
    let checklist = known(&|data| matches!(data, Data::Checklist { .. }));
    let answer = AnswerFacts::of(execution);
    let answer_known = known(&|data| matches!(data, Data::Answer { .. }));
    let reads: Vec<_> = execution
        .steps
        .iter()
        .filter(|step| matches!(step.kind, WorkStepKindV1::Read { .. }))
        .collect();
    let failed_reads = reads
        .iter()
        .filter(|step| step.status != WorkStepStatus::Succeeded)
        .count();
    let elapsed_ms = AGENT_ELAPSED_MS.load(std::sync::atomic::Ordering::Relaxed);
    let marked = execution
        .artifacts
        .iter()
        .filter(|artifact| artifact.general_knowledge)
        .count();
    let mut kinds: Vec<_> = execution
        .artifacts
        .iter()
        .map(|artifact| zephium_core::work::agent::artifact_kind(&artifact.data))
        .collect();
    kinds.sort_unstable();
    let accepted = diagram
        && table
        && answer.accepted()
        && answer_known
        && reads.len() <= 2
        && failed_reads == 0
        && u128::from(elapsed_ms) < ARCHITECTURE_DEADLINE.as_millis();
    let _ = writeln!(
        std::io::stdout().lock(),
        "design_qualification {answer} diagram={diagram} table={table} findings={findings} checklist={checklist} knowledge={marked}/{} reads={} failed_reads={failed_reads} elapsed_ms={elapsed_ms} kinds={} accepted={accepted}",
        execution.artifacts.len(),
        reads.len(),
        kinds.join(","),
    );
    accepted
}

/// The one answer a finished request publishes, measured without its text:
/// how many there are, and the largest one's size and shape.
struct AnswerFacts {
    count: usize,
    first: bool,
    bytes: usize,
    words: usize,
    lines: usize,
    sections: usize,
    fences: usize,
}
impl AnswerFacts {
    fn of(execution: &WorkExecutionFact) -> Self {
        use zephium_core::work::artifact::WorkArtifactDataV1 as Data;
        let answers: Vec<_> = execution
            .artifacts
            .iter()
            .filter_map(|artifact| match &artifact.data {
                Data::Answer { markdown } => Some((&artifact.id, markdown.as_str())),
                _ => None,
            })
            .collect();
        let largest = answers
            .iter()
            .map(|(_, markdown)| *markdown)
            .max_by_key(|markdown| markdown.len())
            .unwrap_or_default();
        let first = answers.iter().any(|(id, _)| {
            execution.steps.iter().any(|step| {
                matches!(step.kind, WorkStepKindV1::Publish) && step.artifacts.first() == Some(*id)
            })
        });
        let fence = |line: &&str| line.trim_start().starts_with("```");
        Self {
            count: answers.len(),
            first,
            bytes: largest.len(),
            words: largest.split_whitespace().count(),
            lines: largest.lines().count(),
            sections: largest
                .lines()
                .filter(|line| line.starts_with("## ") || line.starts_with("### "))
                .count(),
            fences: largest.lines().filter(fence).count() / 2,
        }
    }
    /// Exactly one answer, within the size limit.
    fn accepted(&self) -> bool {
        use zephium_core::work::artifact::{MAX_ANSWER_BYTES, MAX_ANSWER_LINES};
        self.count == 1 && self.bytes <= MAX_ANSWER_BYTES && self.lines <= MAX_ANSWER_LINES
    }
}
impl std::fmt::Display for AnswerFacts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "answers={} answer_first={} answer_bytes={} answer_words={} answer_lines={} answer_sections={} answer_fences={}",
            self.count, self.first, self.bytes, self.words, self.lines, self.sections, self.fences
        )
    }
}

/// Research publishes an answer beside findings that each cite a source.
fn research_accepted(execution: &WorkExecutionFact) -> bool {
    use zephium_core::work::artifact::WorkArtifactDataV1 as Data;
    let answer = AnswerFacts::of(execution);
    let findings: Vec<_> = execution
        .artifacts
        .iter()
        .filter(|artifact| matches!(artifact.data, Data::Findings { .. }))
        .collect();
    let cited = findings
        .iter()
        .all(|artifact| !artifact.evidence.is_empty());
    let accepted = answer.accepted() && cited;
    let _ = writeln!(
        std::io::stdout().lock(),
        "research_qualification {answer} findings={} findings_cited={cited} kinds={} accepted={accepted}",
        findings.len(),
        sorted_kinds(execution),
    );
    accepted
}

fn reads_and_elapsed(execution: &WorkExecutionFact) -> (usize, u64) {
    let reads = execution
        .steps
        .iter()
        .filter(|step| matches!(step.kind, WorkStepKindV1::Read { .. }))
        .count();
    (
        reads,
        AGENT_ELAPSED_MS.load(std::sync::atomic::Ordering::Relaxed),
    )
}

fn sorted_kinds(execution: &WorkExecutionFact) -> String {
    let mut kinds: Vec<_> = execution
        .artifacts
        .iter()
        .map(|artifact| zephium_core::work::agent::artifact_kind(&artifact.data))
        .collect();
    kinds.sort_unstable();
    kinds.join(",")
}

/// How something works: exactly one answer beside a knowledge-marked diagram
/// of at least six nodes, the example in a named language, at most one read,
/// and the loop under its deadline. Findings and a brief are counted only.
fn mechanism_accepted(execution: &WorkExecutionFact) -> bool {
    use zephium_core::work::artifact::WorkArtifactDataV1 as Data;
    let data = || execution.artifacts.iter();
    let diagram = data().any(|artifact| {
        artifact.general_knowledge
            && matches!(&artifact.data, Data::Diagram { nodes, .. } if nodes.len() >= 6)
    });
    let findings = data()
        .filter_map(|artifact| match &artifact.data {
            Data::Findings { items, .. } => Some(items.len()),
            _ => None,
        })
        .sum::<usize>();
    let brief = data().any(|artifact| matches!(artifact.data, Data::Document { .. }));
    let terms = data().any(|artifact| matches!(artifact.data, Data::Table { .. }));
    let language = named_language(explain_objective());
    let example = language.is_none_or(|language| {
        data().any(|artifact| {
            matches!(&artifact.data, Data::Code { language: named, text, notes }
                if named == language && notes.len() >= 3 && text.lines().count() <= 30)
        })
    });
    let answer = AnswerFacts::of(execution);
    let (reads, elapsed_ms) = reads_and_elapsed(execution);
    let accepted = diagram
        && answer.accepted()
        && example
        && reads <= 1
        && u128::from(elapsed_ms) < EXPLANATION_DEADLINE.as_millis();
    let _ = writeln!(
        std::io::stdout().lock(),
        "mechanism_qualification {answer} diagram={diagram} findings_items={findings} brief={brief} terms={terms} language={} example={example} reads={reads} elapsed_ms={elapsed_ms} kinds={} accepted={accepted}",
        language.unwrap_or("none"),
        sorted_kinds(execution),
    );
    accepted
}

fn explain_objective() -> &'static str {
    if EXPLAIN_IN_RUST.load(std::sync::atomic::Ordering::Relaxed) {
        AGENT_EXPLAIN_RUST_OBJECTIVE
    } else {
        AGENT_EXPLAIN_MECHANISM_OBJECTIVE
    }
}

/// The code language a programming language named in the objective maps to.
fn named_language(objective: &str) -> Option<&'static str> {
    const NAMES: [(&str, &str); 9] = [
        ("rust", "rust"),
        ("python", "python"),
        ("typescript", "typescript"),
        ("javascript", "javascript"),
        ("golang", "go"),
        ("java", "java"),
        ("kotlin", "kotlin"),
        ("swift", "swift"),
        ("c++", "cpp"),
    ];
    objective
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | '?' | '!' | ':' | ';'))
        .find_map(|word| {
            let word = word.to_ascii_lowercase();
            NAMES
                .iter()
                .find(|(name, _)| *name == word)
                .map(|(_, language)| *language)
        })
}

/// Two ideas compared from knowledge: exactly one answer beside exactly one
/// knowledge-marked matrix of two subjects and at least four criteria with
/// every cell filled, no reads, no object refused as malformed, and the loop
/// under its deadline.
fn concept_comparison_accepted(execution: &WorkExecutionFact) -> bool {
    use zephium_core::work::artifact::{WorkArtifactDataV1 as Data, WorkCellValue};
    let matrices: Vec<_> = execution
        .artifacts
        .iter()
        .filter_map(|artifact| match &artifact.data {
            Data::ComparisonMatrix {
                subjects,
                criteria,
                cells,
                ..
            } => Some((
                artifact.general_knowledge,
                subjects.len(),
                criteria.len(),
                cells,
            )),
            _ => None,
        })
        .collect();
    let (knowledge, subjects, criteria, filled) = matrices.first().map_or(
        (false, 0, 0, false),
        |(knowledge, subjects, criteria, cells)| {
            let filled = cells.iter().flatten().all(|cell| match &cell.value {
                WorkCellValue::Unknown => false,
                WorkCellValue::Text { text } => !text.trim().is_empty(),
                _ => true,
            });
            (*knowledge, *subjects, *criteria, filled)
        },
    );
    let malformed = MALFORMED_REFUSALS.load(std::sync::atomic::Ordering::Relaxed);
    let answer = AnswerFacts::of(execution);
    let (reads, elapsed_ms) = reads_and_elapsed(execution);
    let accepted = matrices.len() == 1
        && answer.accepted()
        && knowledge
        && subjects == 2
        && criteria >= 4
        && filled
        && reads == 0
        && malformed == 0
        && u128::from(elapsed_ms) < EXPLANATION_DEADLINE.as_millis();
    let _ = writeln!(
        std::io::stdout().lock(),
        "concept_comparison_qualification {answer} matrices={} knowledge={knowledge} subjects={subjects} criteria={criteria} filled={filled} malformed={malformed} reads={reads} elapsed_ms={elapsed_ms} kinds={} accepted={accepted}",
        matrices.len(),
        sorted_kinds(execution),
    );
    accepted
}

/// A pasted function reviewed: a code excerpt with at least two notes, one of
/// them on the off-by-one line, exactly one answer, the bug named in the
/// answer or a finding, no reads, and the loop under its deadline.
fn code_review_accepted(execution: &WorkExecutionFact) -> bool {
    use zephium_core::work::artifact::WorkArtifactDataV1 as Data;
    let (mut notes, mut bug_noted) = (0, false);
    for artifact in &execution.artifacts {
        let Data::Code {
            text, notes: marks, ..
        } = &artifact.data
        else {
            continue;
        };
        let bug = text
            .lines()
            .position(|line| line.contains(CODE_REVIEW_BUG))
            .map(|index| index as u32 + 1);
        notes = notes.max(marks.len());
        bug_noted |= bug.is_some_and(|line| {
            marks
                .iter()
                .any(|note| note.from <= line && line <= note.to)
        });
    }
    let named = execution
        .artifacts
        .iter()
        .any(|artifact| match &artifact.data {
            Data::Findings { items, .. } => items.iter().any(|item| {
                names_off_by_one(&item.claim, item.detail.as_deref().unwrap_or_default())
            }),
            Data::Answer { markdown } => names_off_by_one(markdown, ""),
            _ => false,
        });
    let answer = AnswerFacts::of(execution);
    let (reads, elapsed_ms) = reads_and_elapsed(execution);
    let accepted = notes >= 2
        && answer.accepted()
        && bug_noted
        && named
        && reads == 0
        && u128::from(elapsed_ms) < EXPLANATION_DEADLINE.as_millis();
    let _ = writeln!(
        std::io::stdout().lock(),
        "code_review_qualification {answer} notes={notes} bug_noted={bug_noted} bug_named={named} reads={reads} elapsed_ms={elapsed_ms} kinds={} accepted={accepted}",
        sorted_kinds(execution),
    );
    accepted
}

fn names_off_by_one(claim: &str, detail: &str) -> bool {
    let text = format!("{claim} {detail}").to_ascii_lowercase();
    [
        "off-by-one",
        "off by one",
        "out of bounds",
        "out-of-bounds",
        "past the end",
        "..=",
        "panic",
        "panics",
        "inclusive range",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

/// The routine loop on a public comparison objective. Every step, source and
/// object is durable before the next turn; the proof file keeps the projection.
async fn agent_workflow(
    handle: &zephium_app::Handle,
    composition: &MacosWorkComposition,
    profile: ProfileId,
    binding: zephium_app::AgentWorkProfileBinding,
    turn_key: zephium_agentic::AgentProviderCredential,
    mut browser_keys: Vec<zephium_agentic::AgentProviderCredential>,
    mode: Mode,
) -> Result<WorkflowResult, &'static str> {
    let collection = mode == Mode::AgentMoney;
    let folder = if mode == Mode::AgentFiles {
        Some(probe_folder()?)
    } else {
        None
    };
    let files_objective = folder.as_ref().map(|folder| {
        format!(
            "In the granted folder {}: read README.md and place the three product names it mentions as cited findings from that file. Then edit NOTES.md, replacing the exact line Review pending with Reviewed by Zephium, and finish once the change is applied. Do not search the web or read web pages.",
            folder.display()
        )
    });
    let code_review_objective = format!(
        "Review this Rust function: find its bugs and say what to change.\n\n{CODE_REVIEW_FUNCTION}"
    );
    let objective = match mode {
        Mode::AgentFiles => files_objective.as_deref().unwrap_or_default(),
        Mode::AgentCollection => AGENT_COLLECTION_OBJECTIVE,
        Mode::AgentDetails => AGENT_DETAILS_OBJECTIVE,
        Mode::AgentTrip => "Plan a trip from Poland to San Francisco for a YC batch as a solo founder; for flats check Airbnb. Use public sources for batch timing, travel logistics and practical accommodation tradeoffs, with cited findings. Read Airbnb itself before making any claim about its listings. Dates and budget are unspecified: state planning assumptions and leave live availability and total stay cost unknown unless the pages establish them. Do not book, submit forms, sign in, create accounts, send messages or interact with verification controls. Report blocked pages honestly.",
        Mode::AgentAirbnb => "Find three good Airbnb options in San Francisco for a solo founder attending a YC batch, compare and recommend one using cited public page evidence. Inspect Airbnb itself and observed listing links. Dates and budget are unspecified: state assumptions, distinguish nightly prices from total stay costs, and leave unavailable details unknown. Include observed pictures when available. Do not book, submit forms, sign in, create accounts, send messages or interact with verification controls. If access prevents three verified options, report that limitation instead of inventing options.",
        Mode::AgentMoney => AGENT_MONEY_OBJECTIVE,
        Mode::AgentArchitecture => AGENT_ARCHITECTURE_OBJECTIVE,
        Mode::AgentEngineChart => AGENT_ENGINE_CHART_OBJECTIVE,
        Mode::AgentExplainMechanism => explain_objective(),
        Mode::AgentConceptComparison => AGENT_CONCEPT_COMPARISON_OBJECTIVE,
        Mode::AgentCodeReview => code_review_objective.as_str(),
        Mode::AgentListing => "Read https://www.airbnb.com/rooms/23813739?adults=1 in one browser read and collect this one listing: its name, displayed nightly price, displayed monthly total, stay dates or minimum stay, its own page address as an optional url column named listing_url, and picture. Dates are unspecified, so leave any value the page does not show unknown. Do not search, follow links, book, sign in or interact with verification controls.",
        Mode::AgentRead => AGENT_READ_OBJECTIVE,
        Mode::AgentPage => PAGE_OBJECTIVE.get().map(String::as_str).unwrap_or_default(),
        Mode::AgentGovernment | Mode::AgentHumanGovernment => AGENT_GOVERNMENT_OBJECTIVE,
        Mode::AgentDisclosure => "Read https://www.lego.com/en-us/product/tower-bridge-21067 in one browser assignment. Find the Specifications disclosure, bring it into view if needed, expand it, and inspect its revealed content. Return the product name, displayed price, piece count and exact dimensions with citations from this page. Do not follow links, buy, sign in, change locale, or substitute public search. Leave unsupported details unknown. Use one browser read assignment and a source-backed note.",
        Mode::AgentScroll => "Read https://www.lego.com/en-us/product/tower-bridge-21067 in one browser assignment. Dismiss entry and privacy notices if needed. Before extracting, scroll the document down by one page, inspect the new viewport, then scroll the document down by another page and inspect again. Report the product name and any details visible after scrolling, with cited evidence. The two actual scrolls are required: snapshots alone do not satisfy this task. Do not buy, sign in, change locale, or follow links. Use one read responsibility and a source-backed note.",
        _ => AGENT_OBJECTIVE,
    };
    let created = handle
        .work_authoring_command(
            profile,
            WorkAuthoringCommandV1 {
                version: 1,
                command: WorkCommandId::generate(),
                intent: WorkAuthoringIntent::Create {
                    objective: objective.into(),
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
        .map_err(|_| "agent_transport")?;
    let agent = OpenAiWorkAgent::try_new(
        transport.clone(),
        turn_key,
        WorkPlanningConfig::try_new(
            zephium_agent_model_catalog::try_gpt6_luna_provider_exact_call_config(8192)
                .map_err(|_| "agent_model")?,
            32_768,
            300_000,
        )
        .map_err(|_| "agent_limits")?,
    )
    .map_err(|_| "agent_provider")?
    .with_public_response_retention()
    .with_diagnostic(|event| {
        let _ = writeln!(
            std::io::stdout().lock(),
            "agent-work: turn_diagnostic={event:?}"
        );
    })
    .with_wire_diagnostic(|fault| {
        let _ = writeln!(
            std::io::stdout().lock(),
            "agent-work: phase=agent_turn wire_error path={} expected={} dropped={}",
            fault.path,
            fault.expected,
            fault.dropped
        );
    });
    let link_primary =
        tokio::task::spawn_blocking(zephium_agentic::load_macos_development_typesafe_credential)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|key| {
                zephium_agentic::JevDecisionClient::direct(transport.clone(), key).ok()
            });
    let agent = agent
        .with_link_decisions(
            link_primary,
            WorkPlanningConfig::try_new(
                zephium_agent_model_catalog::try_gpt6_luna_decision_call_config(
                    4096,
                    zephium_agent_model_catalog::Gpt6LunaDecisionEffort::Low,
                )
                .map_err(|_| "link_model")?,
                32_768,
                100_000,
            )
            .map_err(|_| "link_limits")?,
            Some(|fact| {
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "agent-work: link_decision={fact:?}"
                );
            }),
        )
        .map_err(|_| "link_provider")?;
    let grant = WorkAgentGrantV1 {
        no_usage_charge: false,
        lead: None,
        provider: zephium_core::work::search::WorkSearchProvider::OpenAi,
        model: zephium_core::work::search::PUBLIC_SEARCH_MODEL.into(),
        max_turns: 8,
        max_steps: 24,
        browse_hops: 3,
        folders: folder
            .iter()
            .map(|folder| folder.to_string_lossy().into_owned())
            .collect(),
        accounts: Vec::new(),
        private: false,
        skill: None,
        search_fallback: None,
        draft: None,
    };
    let search = OpenAiPublicSearch::try_new(
        transport.clone(),
        browser_keys.pop().ok_or("search_key")?,
        OpenAiPublicSearchConfig::try_new(
            zephium_agent_model_catalog::try_public_search_provider_exact_call_config(
                &grant.model,
                4096,
            )
            .map_err(|_| "search_model")?,
        )
        .map_err(|_| "search_config")?,
    )
    .map_err(|_| "search_provider")?
    .with_public_response_retention();
    let primary =
        tokio::task::spawn_blocking(zephium_agentic::load_macos_development_typesafe_credential)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|key| zephium_agentic::JevDecisionClient::direct(transport, key).ok());
    let search = search
        .with_decision_ranking(
            primary,
            WorkPlanningConfig::try_new(
                zephium_agent_model_catalog::try_gpt6_luna_decision_call_config(
                    4096,
                    zephium_agent_model_catalog::Gpt6LunaDecisionEffort::Low,
                )
                .map_err(|_| "ranking_model")?,
                32_768,
                100_000,
            )
            .map_err(|_| "ranking_limits")?,
            Some(|fact| {
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "agent-work: search_decision={fact:?}"
                );
            }),
        )
        .map_err(|_| "ranking_provider")?;
    let limits = WorkExecutionLimits {
        model_tokens: 1_000_000,
        cost_micro_usd: 1_500_000,
        operations: 64,
        timeout_seconds: 600,
        max_workers: 2,
    };
    let keys = Arc::new(Mutex::new(browser_keys));
    let callback = handle.callback_handle();
    let collection_assignment = CollectionAssignment {
        money: mode == Mode::AgentMoney,
    };
    // The person's stand-in: approves the first proposed change it sees.
    let approver = folder.is_some().then(|| {
        let handle = handle.clone();
        tokio::spawn(async move {
            let mut approved = 0u8;
            for _ in 0..600 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let Ok(request) = handle.work_projection(profile, work) else {
                    continue;
                };
                let WorkReplyV1::Projection { projection } = request.response(profile).await.reply
                else {
                    continue;
                };
                let Some(execution) = projection.executions.first() else {
                    continue;
                };
                if execution.status.terminal() {
                    break;
                }
                let waiting = execution.steps.iter().find(|step| {
                    step.status == WorkStepStatus::Running
                        && step.kind.proposes_write()
                        && step.kind.file_decision().is_none()
                });
                let Some(step) = waiting else {
                    continue;
                };
                let Ok(request) = handle.work_command(
                    profile,
                    zephium_ipc::work::WorkCommandV1 {
                        version: 1,
                        work,
                        expected_revision: projection.work.revision,
                        command: WorkCommandId::generate(),
                        intent: WorkRuntimeIntent::ApproveStep {
                            execution: execution.id,
                            step: step.id,
                            approve: true,
                            for_run: false,
                        },
                    },
                ) else {
                    continue;
                };
                if matches!(
                    request.response(profile).await.reply,
                    WorkReplyV1::ExecutionApplied { .. }
                ) {
                    approved += 1;
                    let _ = writeln!(std::io::stdout().lock(), "agent-work: approved_step=true");
                }
            }
            approved
        })
    });
    let observed = Mutex::new(None);
    let agent_run = async {
        WorkAgentService::new(handle.clone())
            .with_diagnostic(|event| {
                if let zephium_app::work_agent::WorkAgentDiagnostic::ArtifactRefused {
                    reason: zephium_core::work::agent::WorkAgentArtifactRefusal::Malformed(_),
                    ..
                } = event
                {
                    MALFORMED_REFUSALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                let _ = writeln!(std::io::stdout().lock(), "agent-work: loop={event:?}");
            })
            .run(
                profile,
                zephium_ipc::work::WorkCommandV1 {
                    version: 1,
                    work,
                    expected_revision: created.applied_revision,
                    command: WorkCommandId::generate(),
                    intent: WorkRuntimeIntent::BeginAgent { grant, limits },
                },
                None,
                WorkAgentProviders {
                    turn: if collection {
                        &collection_assignment
                    } else {
                        &agent
                    },
                    search: &search,
                },
                |probe, request| {
                    let key = keys.lock().ok().and_then(|mut keys| keys.pop());
                    let callback = &callback;
                    let observed = &observed;
                    async move {
                        let key = match key {
                            Some(key) => key,
                            None => tokio::task::spawn_blocking(load_macos_probe_openai_credential)
                                .await
                                .map_err(|_| WorkError::Unavailable)?
                                .map_err(|_| WorkError::Unavailable)?,
                        };
                        let _ = writeln!(
                            std::io::stdout().lock(),
                            "agent-work: browser_step={}; content=redacted",
                            serde_json::to_value(&request.step)
                                .ok()
                                .and_then(|value| value["kind"].as_str().map(str::to_owned))
                                .unwrap_or_default()
                        );
                        if collection {
                            let schema = money_schema()?;
                            composition
                                .run_collection_step(
                                    callback,
                                    &probe,
                                    request,
                                    browser_settings(binding, key),
                                    schema,
                                )
                                .await
                        } else {
                            let outcome = composition
                                .run_agent_step(
                                    callback,
                                    &probe,
                                    request,
                                    browser_settings(binding, key),
                                )
                                .await;
                            keep_frames(observed);
                            outcome
                        }
                    }
                },
                |observer| {
                    if let Ok(mut observed) = observed.lock() {
                        *observed = Some(observer);
                    }
                },
            )
            .await
    };
    let started = Instant::now();
    let state = if mode == Mode::AgentHumanGovernment {
        tokio::select! {
            result = agent_run => result,
            result = human_government_input(composition, profile, work) => { result?; Err(WorkError::Unavailable) }
        }
    } else { agent_run.await }
        .map_err(|error| {
            let _ = writeln!(
                std::io::stdout().lock(),
                "agent-work: run_failure={error:?}; content=redacted"
            );
            "agent_run"
        })?;
    AGENT_ELAPSED_MS.store(
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
    let execution = state.executions.first().ok_or("agent_execution")?;
    for step in &execution.steps {
        let _ = writeln!(
            std::io::stdout().lock(),
            "agent-work: step turn={} kind={} status={:?} artifacts={} note_bytes={}",
            step.turn,
            serde_json::to_value(&step.kind)
                .ok()
                .and_then(|value| value["kind"].as_str().map(str::to_owned))
                .unwrap_or_default(),
            step.status,
            step.artifacts.len(),
            step.note.as_ref().map_or(0, String::len),
        );
    }
    let mut failure = (execution.status != WorkExecutionStatus::NeedsReview
        || execution.artifacts.is_empty())
    .then_some("agent_outcome");
    if let Some(folder) = folder {
        let approved = match approver {
            Some(task) => task.await.unwrap_or(0),
            None => 0,
        };
        let read = execution.steps.iter().any(|step| {
            matches!(step.kind, WorkStepKindV1::ReadFile { .. })
                && step.status == WorkStepStatus::Succeeded
                && step.evidence.is_some()
        });
        let edited = execution.steps.iter().any(|step| {
            matches!(step.kind, WorkStepKindV1::EditFile { .. })
                && step.status == WorkStepStatus::Succeeded
                && step.kind.file_decision() == Some(true)
        });
        let notes = std::fs::read_to_string(folder.join("NOTES.md")).unwrap_or_default();
        let applied = notes.contains("Reviewed by Zephium") && !notes.contains("Review pending");
        let cited = execution.artifacts.iter().any(|artifact| {
            artifact.evidence.iter().any(|link| {
                execution
                    .file_evidence
                    .iter()
                    .any(|record| record.id == link.extraction_id)
            })
        });
        let _ = writeln!(
            std::io::stdout().lock(),
            "agent-work: files read={read}; edited={edited}; applied={applied}; cited={cited}; approved={approved}; records={}",
            execution.file_evidence.len()
        );
        let _ = std::fs::remove_dir_all(&folder);
        if !(read && edited && applied && cited) {
            failure = failure.or(Some("agent_files_outcome"));
        }
    }
    Ok(WorkflowResult { state, failure })
}

/// A throwaway folder under the home folder, where the grant policy allows.
fn probe_folder() -> Result<std::path::PathBuf, &'static str> {
    let home = std::env::var_os("HOME").ok_or("home")?;
    let folder = std::path::PathBuf::from(home)
        .join("Library/Caches/app.zephium.probe")
        .join(format!("files-{}", std::process::id()));
    std::fs::create_dir_all(&folder).map_err(|_| "probe_folder")?;
    std::fs::write(
        folder.join("README.md"),
        "# Atlas catalogue\n\nThe spring range has three products: Tower Bridge, Statue of Liberty and Taj Mahal.\nEach ships in a numbered box.\n",
    )
    .map_err(|_| "probe_folder")?;
    std::fs::write(folder.join("NOTES.md"), "Review pending\n").map_err(|_| "probe_folder")?;
    Ok(folder)
}

// Qualify the live browser worker independently of main-agent planning quality.
struct CollectionAssignment {
    money: bool,
}
impl zephium_core::work::agent::WorkAgentTurnProvider for CollectionAssignment {
    fn turn<'a>(
        &'a self,
        input: &'a zephium_core::work::agent::WorkAgentTurnDisclosure,
        _trace: zephium_core::work::synthesis::WorkSynthesisTrace,
    ) -> zephium_core::work::agent::WorkAgentTurnFuture<'a> {
        use zephium_core::work::agent::*;
        Box::pin(async move {
            let context = input.context();
            let fetch = if context
                .steps
                .iter()
                .any(|step| matches!(step.kind, "read" | "discover"))
            {
                if !context
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.kind == "comparison_matrix")
                {
                    return Err(
                        zephium_core::work::synthesis::WorkSynthesisError::NotDispatched(
                            WorkError::Unavailable,
                        ),
                    );
                }
                vec![]
            } else if let Some(source) = context.sources.iter().find(|source| {
                zephium_agentic::ContextNavigationTarget::parse(&source.url).is_ok_and(|target| {
                    if self.money {
                        target.as_url().host_str() == Some("demo.vercel.store")
                            && target.as_url().path() == "/product/acme-geometric-circles-t-shirt"
                    } else {
                        target.as_url().host_str() == Some("www.lego.com")
                            && target.as_url().path() == "/en-us/themes/architecture"
                    }
                })
            }) {
                vec![WorkAgentFetch::Read {
                    url: source.url.clone(),
                    collection: None,
                }]
            } else if self.money && context.steps.iter().any(|step| step.kind == "search") {
                vec![WorkAgentFetch::Discover {
                    query: "site:demo.vercel.store/product/acme-geometric-circles-t-shirt Acme Circles T-Shirt".into(),
                    collection: None,
                }]
            } else if context.steps.iter().any(|step| step.kind == "search") {
                return Err(
                    zephium_core::work::synthesis::WorkSynthesisError::NotDispatched(
                        WorkError::Unavailable,
                    ),
                );
            } else {
                vec![WorkAgentFetch::Search {
                    query: if self.money {
                        "site:demo.vercel.store/product/acme-geometric-circles-t-shirt Acme Circles T-Shirt".into()
                    } else {
                        "site:lego.com/en-us/themes/architecture LEGO Architecture sets official catalog".into()
                    },
                }]
            };
            Ok(WorkAgentTurnResult {
                output: WorkAgentTurnOutput {
                    say: None,
                    artifacts: vec![],
                    finish: fetch.is_empty(),
                    followups: vec![],
                    malformed: 0,
                    fetch,
                    ask: None,
                },
                usage: WorkUsage::default(),
            })
        })
    }
}

fn money_schema(
) -> Result<zephium_work_composition::durable_runtime::WorkBrowseCollectionSchema, WorkError> {
    use zephium_agentic::SemanticExtractionFieldSchema as Field;
    zephium_work_composition::durable_runtime::WorkBrowseCollectionSchema::try_new(
        "Observed product prices".into(),
        vec![
            Field::try_text("name".into(), true, 256)
                .and_then(Field::with_verbatim_text)
                .map_err(|_| WorkError::Invalid)?,
            Field::try_money("price".into(), true, vec!["USD".into()])
                .map_err(|_| WorkError::Invalid)?,
            Field::try_image_url("image_url".into(), true, 2048).map_err(|_| WorkError::Invalid)?,
        ],
        1,
    )?
    .with_subject_image_field("image_url")
}

pub(super) use super::loopback_support::browser_settings;
/// What an Airbnb read showed: one listing, whether the site linked it as
/// `/rooms/<id>` or as a search pinned to it, or some other page by path.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum AirbnbPage {
    Listing(String),
    Other(String),
}

fn airbnb_page(value: &str) -> Option<AirbnbPage> {
    let target = zephium_agentic::ContextNavigationTarget::parse(value).ok()?;
    let url = target.as_url();
    let host = url.host_str()?;
    if !["airbnb.com", "airbnb.pl"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return None;
    }
    let listing = match url.path().strip_prefix("/rooms/") {
        Some(rest) => rest
            .split('/')
            .next()
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        None if url.path() == "/s/homes" => url
            .query_pairs()
            .find(|(name, value)| name == "pinned_listings[]" && !value.is_empty())
            .map(|(_, value)| value.into_owned()),
        None => None,
    };
    Some(listing.map_or_else(
        || AirbnbPage::Other(url.path().to_owned()),
        AirbnbPage::Listing,
    ))
}

fn has_product_specification(data: &zephium_core::work::artifact::WorkArtifactDataV1) -> bool {
    use zephium_core::work::artifact::{WorkArtifactDataV1, WorkCellValue};
    let WorkArtifactDataV1::ComparisonMatrix {
        criteria, cells, ..
    } = data
    else {
        return false;
    };
    criteria.iter().enumerate().any(|(index, criterion)| {
        let name = criterion.name.to_ascii_lowercase();
        (name.contains("piece") || name.contains("dimension"))
            && cells.iter().any(|row| row.get(index).is_some_and(|cell| {
                !cell.evidence.is_empty()
                    && matches!(&cell.value, WorkCellValue::Text { text } if !text.trim().is_empty())
            }))
    })
}

async fn human_government_input(
    composition: &MacosWorkComposition,
    profile: ProfileId,
    work: WorkId,
) -> Result<(), &'static str> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let continued = Arc::new(AtomicBool::new(false));
    let mut presented = None;
    let mut reader_started = false;
    let mut previous = None;
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let pages = composition
            .human_pages(profile, work)
            .map_err(|_| "human_pages")?;
        if presented.is_some_and(|id| {
            !pages
                .iter()
                .any(|page| page.id == id && page.phase != WorkHumanPhaseV1::Released)
        }) {
            return Err("human_wait_released");
        }
        if let Some(page) = pages.first() {
            let facts = (
                page.phase,
                page.document_revision.clone(),
                page.can_continue,
            );
            if previous.as_ref() != Some(&facts) {
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "agent-work: human_state={:?} document_revision={} can_continue={}",
                    facts.0,
                    facts.1,
                    facts.2
                );
                previous = Some(facts);
            }
            if page.phase == WorkHumanPhaseV1::WaitingForHuman && presented.is_none() {
                composition
                    .present_human_page(
                        profile,
                        work,
                        page.id,
                        WorkHumanRegionV1 {
                            x: 0,
                            y: 0,
                            width: 760,
                            height: 640,
                        },
                    )
                    .map_err(|_| "human_present")?;
                presented = Some(page.id);
            }
            if page.phase == WorkHumanPhaseV1::Presented && !reader_started {
                reader_started = true;
                let flag = continued.clone();
                std::thread::Builder::new()
                    .name("human-continue-input".into())
                    .spawn(move || {
                        use std::io::Read;
                        let mut command = [0u8; 9];
                        if std::io::stdin().read_exact(&mut command).is_ok()
                            && &command == b"continue\n"
                        {
                            flag.store(true, Ordering::Release);
                        }
                    })
                    .map_err(|_| "human_input")?;
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "agent-work: human_phase=presented explicit_continue_required=true"
                );
            }
            if page.phase == WorkHumanPhaseV1::Presented
                && page.can_continue
                && continued.load(Ordering::Acquire)
            {
                composition
                    .continue_human_page(profile, work, page.id, WorkHumanAccountV1::Anonymous)
                    .map_err(|_| "human_continue")?;
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "agent-work: human_phase=continuing"
                );
                std::future::pending::<()>().await;
            }
        }
    }
}

#[cfg(test)]
mod explain_tests {
    #[test]
    fn an_objective_names_its_programming_language_by_whole_word() {
        assert_eq!(
            super::named_language(super::AGENT_EXPLAIN_RUST_OBJECTIVE),
            Some("rust")
        );
        assert_eq!(
            super::named_language("How does C++ move semantics work?"),
            Some("cpp")
        );
        assert_eq!(
            super::named_language(super::AGENT_EXPLAIN_MECHANISM_OBJECTIVE),
            None
        );
        assert_eq!(super::named_language("Explain trusted computing"), None);
    }
}

#[cfg(test)]
mod code_review_tests {
    #[test]
    fn a_finding_that_the_loop_panics_names_the_off_by_one() {
        assert!(super::names_off_by_one(
            "The rolling loop panics at the final iteration",
            "The inclusive range reaches readings.len(), then indexes readings[i]. Use `window..readings.len()` instead.",
        ));
        assert!(!super::names_off_by_one(
            "The snapshot clone is unused",
            "Remove `let snapshot = readings.to_vec();` to avoid an unnecessary allocation.",
        ));
    }
}

#[cfg(test)]
mod airbnb_page_tests {
    use super::{airbnb_page, AirbnbPage};

    #[test]
    fn a_room_and_a_pinned_search_are_the_same_listing_and_a_catalog_is_not() {
        let listing = Some(AirbnbPage::Listing("49597911".into()));
        assert_eq!(
            airbnb_page("https://www.airbnb.com/rooms/49597911"),
            listing
        );
        assert_eq!(
            airbnb_page(
                "https://www.airbnb.com/rooms/49597911?search_mode=regular_search&adults=1"
            ),
            listing
        );
        assert_eq!(
            airbnb_page("https://www.airbnb.com/s/homes?pinned_listings%5B%5D=49597911&pinned_reason=SEO&photo_id=1"),
            listing
        );
        assert_eq!(
            airbnb_page(
                "https://www.airbnb.com/s/homes?pinned_listings[]=49597911&pinned_reason=SEO"
            ),
            listing
        );
        assert_eq!(
            airbnb_page("https://www.airbnb.com/s/homes?query=San%20Francisco"),
            Some(AirbnbPage::Other("/s/homes".into()))
        );
        assert_eq!(
            airbnb_page("https://www.airbnb.com/san-francisco-ca/stays"),
            Some(AirbnbPage::Other("/san-francisco-ca/stays".into()))
        );
        assert_eq!(airbnb_page("https://example.com/rooms/49597911"), None);
    }
}

// This qualification adapter does not expose an account-mode image opt-in.
fn initialize_probe_screenshots(
    account: bool,
    readback: impl FnOnce() -> zephium_core::ports::store::AppSettingReadback,
    initialize: impl FnOnce(zephium_core::ports::store::AppSettingReadback),
) {
    if !account {
        initialize(readback());
    }
}
#[cfg(test)]
#[test]
fn account_qualification_never_reads_or_enables_model_screenshot_preference() {
    use zephium_core::ports::store::AppSettingReadback;
    initialize_probe_screenshots(
        true,
        || panic!("account readback forbidden"),
        |_| panic!("account image initialization forbidden"),
    );
    let read = std::cell::Cell::new(false);
    let enabled = std::cell::Cell::new(false);
    initialize_probe_screenshots(
        false,
        || {
            read.set(true);
            AppSettingReadback::Missing
        },
        |value| {
            assert!(matches!(value, AppSettingReadback::Missing));
            enabled.set(true);
        },
    );
    assert!(read.get() && enabled.get());
}
