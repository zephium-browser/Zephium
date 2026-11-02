//! `cargo xtask webext-suite`: runs real Chrome Web Store extensions through
//! the WebKit runtime's lab harness and checks each still starts, renders its
//! popup, and does its job, as `crates/zephium-webext-macos/suite.json`
//! expects. A `check` loads a page (a fixture served from
//! `suite-fixtures/`, or a real site) and asks whether the extension changed
//! it the way it should; `popup_has`/`popup_lacks` read the popup's text and
//! `logs` the extension's own output.
//! After the checks each extension idles for a while, and the memory and CPU
//! of the WebKit helpers it kept busy go to `target/webext-suite/results.json`,
//! which the next run compares itself against.
//! Packages are downloaded into `target/webext-suite/` and refreshed weekly.

mod fixtures;
mod measure;
mod results;

use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use results::{Record, Resources, Results, Status};

const CHROME_VERSION: &str = "152.0.0.0";
const START_TIMEOUT: Duration = Duration::from_secs(90);
/// The longest any one step runs, so a longer silence means the lab is stuck.
const STALL: Duration = Duration::from_secs(90);
const REFRESH_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const PARALLEL: usize = 4;
const IDLE: Duration = Duration::from_secs(15);
/// What the background gets to finish starting before anything uses it.
const SETTLE: Duration = Duration::from_millis(2500);
const POPUP_WAIT: u64 = 10_000;
/// Loads of the ordinary page timed per run; the least busy counts.
const LOADS: usize = 3;
/// Page loads and closes in a `--soak` run, after which memory should be back.
const SOAK_LOADS: usize = 30;
/// What an API the runtime lacks or gets wrong looks like when it reaches an
/// extension; its own network or account errors don't match.
const GAP_SIGNATURES: &[&str] = &[
    "TypeError",
    "ReferenceError",
    "is not a function",
    "undefined is not an object",
    "Invalid call to",
];

/// The lab closes each check's tab, which an extension may still be talking to.
const CLOSED_TAB: &str = "Tab not found";

#[derive(Deserialize)]
struct Expectation {
    name: String,
    id: String,
    popup: bool,
    /// Text the popup must show once it has rendered.
    #[serde(default)]
    popup_has: Vec<String>,
    /// Text that means the popup came up broken.
    #[serde(default)]
    popup_lacks: Vec<String>,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    known: Option<String>,
    /// Lines the extension must have logged.
    #[serde(default)]
    logs: Vec<LogLine>,
    #[serde(default)]
    check: Vec<Check>,
}

#[derive(Deserialize)]
struct LogLine {
    has: String,
    #[serde(default = "LogLine::default_min")]
    min: usize,
}

impl LogLine {
    fn default_min() -> usize {
        1
    }
}

#[derive(Deserialize)]
struct Check {
    /// A fixture path like `/login.html`, or an absolute URL.
    url: String,
    /// Body of an async function run in the page; truthy means it worked. It
    /// is retried for `wait` milliseconds, since content scripts and rule
    /// sets settle after the page loads.
    eval: String,
    /// Real key presses sent to the page before the first try.
    #[serde(default)]
    keys: String,
    #[serde(default = "Check::default_wait")]
    wait: u64,
    /// Time on the page after the check passes, for logs and late effects.
    #[serde(default)]
    settle: u64,
}

impl Check {
    fn default_wait() -> u64 {
        5000
    }
}

struct Outcome {
    name: String,
    id: String,
    known: Option<String>,
    problems: Vec<String>,
    resources: Resources,
    duration: Duration,
}

enum Job {
    Baseline,
    Extension(Expectation),
}

pub(crate) fn run(arguments: &[String]) -> Result<(), String> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let package = repository.join("crates/zephium-webext-macos");
    let suite: Vec<Expectation> = serde_json::from_slice(
        &std::fs::read(package.join("suite.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("suite.json: {e}"))?;
    let only: Vec<String> = arguments
        .windows(2)
        .find(|pair| pair[0] == "--only")
        .map(|pair| pair[1].split(',').map(str::to_lowercase).collect())
        .unwrap_or_default();
    let suite: Vec<Expectation> = suite
        .into_iter()
        .filter(|entry| only.is_empty() || only.contains(&entry.name.to_lowercase()))
        .collect();
    let soak = arguments.iter().any(|argument| argument == "--soak");

    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository.join("target"));
    let cache = target.join("webext-suite");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    for entry in &suite {
        fetch(&cache, &entry.id).map_err(|e| format!("{}: {e}", entry.name))?;
    }
    let status = Command::new("cargo")
        .current_dir(&repository)
        .args([
            "build",
            "-q",
            "-p",
            "zephium-webext-macos",
            "--features",
            "lab",
            "--bin",
            "webext-lab",
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("the lab harness did not build".into());
    }
    let lab = target.join("debug/webext-lab");
    let origin = fixtures::serve(package.join("suite-fixtures"))?;

    let jobs = std::iter::once(Job::Baseline)
        .chain(suite.into_iter().map(Job::Extension))
        .collect::<VecDeque<_>>();
    let queue = Arc::new(Mutex::new(jobs));
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let workers: Vec<_> = (0..PARALLEL)
        .map(|_| {
            let (queue, outcomes, lab, cache, origin) = (
                queue.clone(),
                outcomes.clone(),
                lab.clone(),
                cache.clone(),
                origin.clone(),
            );
            std::thread::spawn(move || loop {
                let Some(job) = queue.lock().ok().and_then(|mut queue| queue.pop_front()) else {
                    return;
                };
                let outcome = match job {
                    Job::Baseline => baseline(&lab, &cache, &origin, soak),
                    Job::Extension(entry) => check(&lab, &cache, &origin, &entry, soak),
                };
                if let Ok(mut outcomes) = outcomes.lock() {
                    outcomes.push(outcome);
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }

    let mut outcomes = Arc::try_unwrap(outcomes)
        .ok()
        .and_then(|outcomes| outcomes.into_inner().ok())
        .unwrap_or_default();
    outcomes.sort_by(|a, b| a.name.cmp(&b.name));
    let (baseline, outcomes): (Vec<_>, Vec<_>) = outcomes
        .into_iter()
        .partition(|outcome| outcome.id.is_empty());

    let results_path = cache.join("results.json");
    let mut results = Results::load(&results_path);
    let records: BTreeMap<String, Record> = outcomes
        .iter()
        .map(|outcome| (outcome.name.clone(), record(outcome)))
        .collect();

    println!(
        "{:<8} {:<26} {:>6} {:>6} {:>9} {:>7} {:>7} {:>5}",
        "status", "extension", "mem MB", "rss MB", "idle cpu", "busy ms", "soak MB", "time"
    );
    if let Some(baseline) = baseline.first() {
        println!(
            "{:<8} {:<26} {}",
            "",
            "(page only, no extension)",
            figures(&baseline.resources, baseline.duration)
        );
        for problem in &baseline.problems {
            println!("         {problem}");
        }
    }
    for outcome in &outcomes {
        let record = &records[&outcome.name];
        println!(
            "{:<8} {:<26} {}",
            record.status.label(),
            outcome.name,
            figures(&record.resources, outcome.duration)
        );
        if record.status != Status::Ok {
            if let Some(reason) = &outcome.known {
                println!("         known: {reason}");
            }
            for problem in &outcome.problems {
                println!("         {problem}");
            }
        } else if let Some(reason) = &outcome.known {
            println!("         passes although marked known: {reason}");
        }
    }
    let count = |status| records.values().filter(|r| r.status == status).count();
    let failed = count(Status::Fail);
    println!(
        "{} extensions: {} ok, {} limited, {failed} failing",
        outcomes.len(),
        count(Status::Ok),
        count(Status::Limited)
    );
    let heaviest = |pick: fn(&Resources) -> Option<f64>, unit: &str, places: usize| {
        let mut ranked: Vec<_> = records
            .iter()
            .filter_map(|(name, r)| Some((name, pick(&r.resources)?)))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked
            .iter()
            .take(5)
            .map(|(name, value)| format!("{name} {value:.places$}{unit}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!("most memory: {}", heaviest(|r| r.footprint_mb, " MB", 0));
    println!("most idle cpu: {}", heaviest(|r| r.idle_cpu_s, " s", 2));
    println!(
        "most page time taken: {}",
        heaviest(|r| r.busy_ms, " ms", 0)
    );
    if soak {
        println!(
            "most memory kept after closing pages: {}",
            heaviest(|r| r.soak_mb, " MB", 0)
        );
    }
    let regressions = results.regressions(&records);
    for regression in &regressions {
        println!("regressed since the last run: {regression}");
    }

    results.generated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if let Some(resources) = baseline.first().and_then(accepted_baseline) {
        results.baseline = Some(resources.clone());
    }
    results.extensions.extend(records);
    results.save(&results_path)?;
    if failed > 0 {
        return Err(format!("{failed} extensions regressed"));
    }
    Ok(())
}

/// The figures a baseline run may save. A run that failed or measured no
/// footprint keeps the last good figures to compare against; its idle CPU
/// is kept as measured, missing or not.
fn accepted_baseline(outcome: &Outcome) -> Option<&Resources> {
    (outcome.problems.is_empty() && outcome.resources.footprint_mb.is_some())
        .then_some(&outcome.resources)
}

fn figures(resources: &Resources, duration: Duration) -> String {
    let cell = |value: Option<f64>, places: usize| {
        value.map_or("-".to_string(), |value| format!("{value:.places$}"))
    };
    format!(
        "{:>6} {:>6} {:>9} {:>7} {:>7} {:>4}s",
        cell(resources.footprint_mb, 0),
        cell(resources.rss_mb, 0),
        cell(resources.idle_cpu_s, 2),
        cell(resources.busy_ms, 0),
        cell(resources.soak_mb, 0),
        duration.as_secs()
    )
}

fn record(outcome: &Outcome) -> Record {
    let status = match (&outcome.known, outcome.problems.is_empty()) {
        (_, true) => Status::Ok,
        (Some(_), false) => Status::Limited,
        (None, false) => Status::Fail,
    };
    Record {
        id: outcome.id.clone(),
        status,
        reasons: outcome.problems.clone(),
        resources: outcome.resources.clone(),
        duration_s: (outcome.duration.as_secs_f64() * 10.0).round() / 10.0,
    }
}

fn fetch(cache: &Path, id: &str) -> Result<(), String> {
    let path = cache.join(format!("{id}.crx"));
    let fresh = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < REFRESH_AFTER);
    if fresh {
        return Ok(());
    }
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion={CHROME_VERSION}&acceptformat=crx3&x=id%3D{id}%26installsource%3Dondemand%26uc"
    );
    let partial = cache.join(format!("{id}.crx.part"));
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "120", "-o"])
        .arg(&partial)
        .arg(url)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("download failed".into());
    }
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())
}

struct Run {
    output: String,
    resources: Resources,
    duration: Duration,
}

fn baseline(lab: &Path, cache: &Path, origin: &str, soak: bool) -> Outcome {
    let mut steps = vec![serde_json::json!({ "tab": format!("{origin}/blank.html") })];
    steps.extend(load_steps(origin));
    if soak {
        steps.extend(soak_steps(origin));
    }
    steps.extend(idle_steps());
    let (problems, resources, duration) =
        match launch(lab, cache, "baseline", &steps, START_TIMEOUT) {
            // Figures from a run that never finished would replace good ones.
            Ok(run) if run.output.lines().any(|line| line == "DONE") => {
                (Vec::new(), timed(run.resources, &run.output), run.duration)
            }
            Ok(run) => (
                vec!["baseline did not finish in time".to_string()],
                timed(run.resources, &run.output),
                run.duration,
            ),
            Err(error) => (vec![error], Resources::default(), Duration::ZERO),
        };
    Outcome {
        name: String::new(),
        id: String::new(),
        known: None,
        problems,
        resources,
        duration,
    }
}

fn check(lab: &Path, cache: &Path, origin: &str, entry: &Expectation, soak: bool) -> Outcome {
    let crx = cache.join(format!("{}.crx", entry.id));
    let budget = START_TIMEOUT
        + IDLE
        + Duration::from_millis(
            entry
                .check
                .iter()
                .map(|check| check.wait + 12_000)
                .sum::<u64>()
                + if entry.popup { POPUP_WAIT + 5000 } else { 0 },
        )
        + Duration::from_secs(if soak { 120 } else { 20 });
    let steps = steps(entry, &crx, origin, soak);
    let (problems, resources, duration) = match launch(lab, cache, &entry.id, &steps, budget) {
        Ok(run) => (
            evaluate(entry, &run.output),
            timed(run.resources, &run.output),
            run.duration,
        ),
        Err(error) => (vec![error], Resources::default(), Duration::ZERO),
    };
    Outcome {
        name: entry.name.clone(),
        id: entry.id.clone(),
        known: entry.known.clone(),
        problems,
        resources,
        duration,
    }
}

fn steps(entry: &Expectation, crx: &Path, origin: &str, soak: bool) -> Vec<serde_json::Value> {
    let mut steps = vec![serde_json::json!({ "load": crx, "compat": true })];
    // The popup is opened first, cold, from a tab with no page behind it:
    // the browser's fresh new tab, before any site has woken the extension.
    if entry.popup {
        steps.push(serde_json::json!({ "blank": true }));
    } else {
        steps.push(serde_json::json!({ "tab": format!("{origin}/blank.html") }));
        steps.push(serde_json::json!({ "background": true }));
        steps.push(serde_json::json!({ "sleep": SETTLE.as_millis() as u64 }));
    }
    if entry.popup {
        steps.push(serde_json::json!({ "action": true }));
        steps.push(serde_json::json!({
            "eval": popup_script(entry),
            "in": "page",
            "timeout": POPUP_WAIT + 5000,
            "as": "popup"
        }));
        steps.push(serde_json::json!({
            "eval": "const page = document.documentElement; return JSON.stringify({ width: innerWidth, height: innerHeight, content: page.scrollWidth });",
            "in": "page",
            "as": "popup-size"
        }));
        steps.push(serde_json::json!({ "close": "popup" }));
        steps.push(serde_json::json!({ "tab": format!("{origin}/blank.html") }));
        steps.push(serde_json::json!({ "background": true }));
        steps.push(serde_json::json!({ "sleep": SETTLE.as_millis() as u64 }));
    }
    steps.extend(load_steps(origin));
    for (index, check) in entry.check.iter().enumerate() {
        let url = if check.url.starts_with('/') {
            format!("{origin}{}", check.url)
        } else {
            check.url.clone()
        };
        steps.push(serde_json::json!({ "tab": url }));
        steps.push(serde_json::json!({ "keys": check.keys, "in": "tab" }));
        steps.push(serde_json::json!({
            "eval": retry_script(&check.eval, check.wait),
            "in": "tab",
            "timeout": check.wait + 2000,
            "as": format!("check{index}")
        }));
        steps.push(serde_json::json!({ "sleep": check.settle }));
        steps.push(serde_json::json!({ "close": "tabs" }));
    }
    if soak {
        steps.extend(soak_steps(origin));
    }
    steps.extend(idle_steps());
    steps
}

/// An ordinary page with a sign-in form, loaded a few times so the least
/// busy load stands for what the extension adds to every page.
fn load_steps(origin: &str) -> Vec<serde_json::Value> {
    let mut steps = vec![serde_json::json!({ "close": "tabs" })];
    for index in 0..LOADS {
        steps.push(serde_json::json!({ "tab": format!("{origin}/page.html?{index}") }));
        steps.push(serde_json::json!({
            "eval": "await new Promise((resolve) => setTimeout(resolve, 1600)); return Math.round(window.busy);",
            "in": "tab",
            "as": format!("load{index}")
        }));
        steps.push(serde_json::json!({ "close": "tabs" }));
    }
    steps
}

/// Pages opened and closed many times; memory the extension keeps after the
/// last one closes is what it would leak over a day.
fn soak_steps(origin: &str) -> Vec<serde_json::Value> {
    let mut steps = Vec::new();
    // A few rounds first, so processes WebKit keeps warm for closed pages
    // are there at both ends.
    for index in 0..5 {
        steps.push(serde_json::json!({ "tab": format!("{origin}/page.html?warm{index}") }));
        steps.push(serde_json::json!({ "close": "tabs" }));
    }
    steps.push(serde_json::json!({ "sleep": 5000 }));
    steps.push(serde_json::json!({ "mark": "soak-start" }));
    for index in 0..SOAK_LOADS {
        steps.push(serde_json::json!({ "tab": format!("{origin}/page.html?soak{index}") }));
        steps.push(serde_json::json!({ "close": "tabs" }));
    }
    steps.push(serde_json::json!({ "sleep": 5000 }));
    steps.push(serde_json::json!({ "mark": "soak-end" }));
    steps
}

fn timed(mut resources: Resources, output: &str) -> Resources {
    resources.busy_ms = (0..LOADS)
        .filter_map(|index| result(output, &format!("load{index}"))?.parse::<f64>().ok())
        .min_by(f64::total_cmp);
    resources
}

/// The extension rests, with one idle page open, while its helpers are sampled.
fn idle_steps() -> [serde_json::Value; 5] {
    [
        serde_json::json!({ "close": "tabs" }),
        serde_json::json!({ "mark": "idle-start" }),
        serde_json::json!({ "sleep": IDLE.as_millis() as u64 }),
        serde_json::json!({ "mark": "idle-end" }),
        serde_json::json!({ "sleep": 500 }),
    ]
}

/// Reads the popup's text, waiting for what it should say, and for a moment
/// longer when it might still turn into an error.
fn popup_script(entry: &Expectation) -> String {
    let lowered = |words: &[String]| {
        serde_json::to_string(
            &words
                .iter()
                .map(|word| word.to_lowercase())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default()
    };
    format!(
        "const has = {}; const lacks = {};\n\
         const read = () => [document.body, ...[...document.querySelectorAll('*')].map((e) => e.shadowRoot)].filter(Boolean).map((root) => root.innerText ?? root.textContent ?? '').join(' ').replace(/\\s+/g, ' ').trim();\n\
         const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));\n\
         const end = Date.now() + {POPUP_WAIT};\n\
         let text = read();\n\
         while (Date.now() < end && !(text && has.every((word) => text.toLowerCase().includes(word)))) {{ await pause(250); text = read(); }}\n\
         if (lacks.length) {{ await pause(3000); text = read(); }}\n\
         return text.slice(0, 1500);",
        lowered(&entry.popup_has),
        lowered(&entry.popup_lacks)
    )
}

/// Runs `body` until it returns something truthy or `wait` runs out.
fn retry_script(body: &str, wait: u64) -> String {
    format!(
        "const attempt = async () => {{ {body} }};\n\
         const end = Date.now() + {wait};\n\
         let note = 'falsy';\n\
         for (;;) {{\n\
           try {{ if (await attempt()) return true; note = 'falsy'; }} catch (error) {{ note = String(error); }}\n\
           if (Date.now() > end) return note;\n\
           await new Promise((resolve) => setTimeout(resolve, 250));\n\
         }}"
    )
}

/// Runs the lab on `steps`, sampling the helpers it owns across its idle marks.
fn launch(
    lab: &Path,
    cache: &Path,
    stem: &str,
    steps: &[serde_json::Value],
    budget: Duration,
) -> Result<Run, String> {
    let scenario = cache.join(format!("{stem}.scenario.json"));
    std::fs::write(&scenario, serde_json::to_vec(steps).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    let log = cache.join(format!("{stem}.log"));
    let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut child = Command::new(lab)
        .arg(&scenario)
        .stdout(file.try_clone().map_err(|e| e.to_string())?)
        .stderr(file)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let footprints = cache.join(format!("{stem}.footprint.json"));
    let mut reader = std::fs::File::open(&log).map_err(|e| e.to_string())?;
    // Decoded whole each time, because a poll can end inside a character.
    let mut bytes = Vec::new();
    let mut output = String::new();
    let mut idle_start = None;
    let mut soak_start = None;
    let mut soak_mb = None;
    // Whether the idle window was measured, whatever footprint could say.
    let mut sampled = false;
    let mut resources = Resources::default();
    let deadline = started + budget;
    let mut last_step = started;
    loop {
        let before = bytes.len();
        let _ = reader.read_to_end(&mut bytes);
        if bytes.len() > before {
            // A marker can straddle two polls; the overlap is shorter than it.
            if bytes[before.saturating_sub(4)..]
                .windows(5)
                .any(|window| window == b"STEP ")
            {
                last_step = Instant::now();
            }
            output = String::from_utf8_lossy(&bytes).into_owned();
        }
        if soak_start.is_none() && output.contains("MARK soak-start\n") {
            soak_start = measure::sample(child.id(), &footprints);
        } else if soak_mb.is_none() && output.contains("MARK soak-end\n") {
            if let (Some(start), Some(end)) =
                (&soak_start, measure::sample(child.id(), &footprints))
            {
                soak_mb = start
                    .footprint_bytes
                    .zip(end.footprint_bytes)
                    .map(|(start, end)| ((end as f64 - start as f64) / 1_048_576.0).round());
            }
        }
        if idle_start.is_none() && output.contains("MARK idle-start\n") {
            idle_start = Some(measure::sample(child.id(), &footprints).unwrap_or_default());
        } else if !sampled && output.contains("MARK idle-end\n") {
            if let (Some(start), Some(end)) =
                (&idle_start, measure::sample(child.id(), &footprints))
            {
                sampled = true;
                resources = Resources {
                    rss_mb: Some((end.rss_kb as f64 / 1024.0).round()),
                    footprint_mb: end
                        .footprint_bytes
                        .map(|bytes| (bytes as f64 / 1_048_576.0).round()),
                    idle_cpu_s: Some((end.cpu_since(start) * 100.0).round() / 100.0),
                    processes: end.processes(),
                    ..Resources::default()
                };
            }
        }
        if output.ends_with("DONE\n")
            || Instant::now() >= deadline
            || last_step.elapsed() >= STALL
            || matches!(child.try_wait(), Ok(Some(_)))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let duration = started.elapsed();
    if !sampled {
        if let Some(last) = measure::sample(child.id(), &footprints) {
            resources = Resources {
                rss_mb: Some((last.rss_kb as f64 / 1024.0).round()),
                footprint_mb: last
                    .footprint_bytes
                    .map(|bytes| (bytes as f64 / 1_048_576.0).round()),
                idle_cpu_s: None,
                processes: last.processes(),
                ..Resources::default()
            };
        }
    }
    resources.soak_mb = soak_mb;
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.read_to_end(&mut bytes);
    let output = String::from_utf8_lossy(&bytes).into_owned();
    remove_profile(&output);
    Ok(Run {
        output,
        resources,
        duration,
    })
}

/// Each run gets a fresh WebKit profile, which would otherwise stay on disk.
fn remove_profile(output: &str) {
    let Some(profile) = output
        .lines()
        .find_map(|line| line.strip_prefix("[lab] profile "))
    else {
        return;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let webkit = Path::new(&home).join("Library/WebKit/webext-lab");
    for (directory, name) in [
        ("WebsiteDataStore", profile.to_lowercase()),
        ("WebExtensions", profile.to_uppercase()),
    ] {
        let _ = std::fs::remove_dir_all(webkit.join(directory).join(name));
    }
}

/// What the lab printed for the eval step labelled `label`.
fn result<'a>(output: &'a str, label: &str) -> Option<&'a str> {
    let prefix = format!("RESULT {label} ");
    output
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
}

fn evaluate(entry: &Expectation, output: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if !output.lines().any(|line| line.starts_with("LOADED Ok(")) {
        problems.push("did not load".to_string());
    }
    if !output.lines().any(|line| line == "DONE") {
        problems.push("did not finish in time".to_string());
    }
    if entry.popup {
        let text = result(output, "popup")
            .and_then(|value| serde_json::from_str::<String>(value).ok())
            .unwrap_or_default();
        let lowered = text.to_lowercase();
        if text.is_empty() {
            problems.push("popup rendered nothing".to_string());
        }
        let size = result(output, "popup-size")
            .and_then(|value| serde_json::from_str::<String>(value).ok())
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok());
        if let Some(size) = size {
            let number = |key: &str| size.get(key).and_then(serde_json::Value::as_f64);
            if let (Some(width), Some(height), Some(content)) =
                (number("width"), number("height"), number("content"))
            {
                if content > width + 1.0 {
                    problems.push(format!(
                        "popup squeezed: {content:.0} px of content in {width:.0}x{height:.0}"
                    ));
                }
            }
        }
        for word in &entry.popup_lacks {
            if lowered.contains(&word.to_lowercase()) {
                problems.push(format!("popup says {word:?}: {}", excerpt(&text, 100)));
            }
        }
        for word in &entry.popup_has {
            if !text.is_empty() && !lowered.contains(&word.to_lowercase()) {
                problems.push(format!("popup lacks {word:?}: {}", excerpt(&text, 100)));
            }
        }
    }
    for (index, check) in entry.check.iter().enumerate() {
        match result(output, &format!("check{index}")) {
            Some("true") => {}
            Some(note) if note.starts_with('"') && note != "\"falsy\"" => problems.push(format!(
                "did nothing on {} ({})",
                check.url,
                note.trim_matches('"')
            )),
            _ => problems.push(format!("did nothing on {}", check.url)),
        }
    }
    let tag = format!("[{} ", entry.id);
    for wanted in &entry.logs {
        let seen = output
            .lines()
            .filter(|line| line.starts_with(&tag) && line.contains(&wanted.has))
            .count();
        if seen < wanted.min {
            problems.push(format!(
                "logged {:?} {seen} times, expected {}",
                wanted.has, wanted.min
            ));
        }
    }
    let tag = format!("[{} ERROR]", entry.id);
    for line in output.lines().filter(|line| line.starts_with(&tag)) {
        let message = line[tag.len()..].trim();
        let gap = GAP_SIGNATURES
            .iter()
            .any(|signature| message.contains(signature))
            && !message.contains(CLOSED_TAB);
        let allowed = entry
            .allow
            .iter()
            .any(|allowed| message.contains(allowed.as_str()));
        if gap && !allowed {
            let short = excerpt(message, 160);
            if !problems.contains(&short) {
                problems.push(short);
            }
        }
    }
    problems
}

fn excerpt(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_finished_baseline_with_a_footprint_is_saved() {
        let outcome = |problems: Vec<String>, footprint_mb: Option<f64>| Outcome {
            name: String::new(),
            id: String::new(),
            known: None,
            problems,
            resources: Resources {
                footprint_mb,
                ..Resources::default()
            },
            duration: Duration::ZERO,
        };
        let finished = outcome(Vec::new(), Some(40.0));
        let saved = accepted_baseline(&finished).expect("a finished run is saved");
        assert_eq!(saved.idle_cpu_s, None);
        assert!(accepted_baseline(&outcome(
            vec!["baseline did not finish in time".into()],
            Some(40.0)
        ))
        .is_none());
        assert!(accepted_baseline(&outcome(Vec::new(), None)).is_none());
    }

    fn expectation(popup: bool, allow: &[&str]) -> Expectation {
        Expectation {
            name: "Probe".into(),
            id: "abcdefghijklmnopabcdefghijklmnop".into(),
            popup,
            popup_has: Vec::new(),
            popup_lacks: Vec::new(),
            allow: allow.iter().map(|entry| entry.to_string()).collect(),
            known: None,
            logs: Vec::new(),
            check: Vec::new(),
        }
    }

    fn check_on(url: &str) -> Check {
        Check {
            url: url.into(),
            eval: "return true".into(),
            keys: String::new(),
            wait: 0,
            settle: 0,
        }
    }

    #[test]
    fn the_suite_lists_each_extension_once_and_only_serves_fixtures_that_exist() {
        let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/zephium-webext-macos");
        let suite: Vec<Expectation> =
            serde_json::from_slice(&std::fs::read(package.join("suite.json")).unwrap()).unwrap();
        let mut names = std::collections::HashSet::new();
        let mut ids = std::collections::HashSet::new();
        for entry in &suite {
            assert!(names.insert(&entry.name), "{} is listed twice", entry.name);
            assert!(ids.insert(&entry.id), "{} is listed twice", entry.id);
            assert!(
                entry.id.len() == 32 && entry.id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)),
                "{} is not an extension id",
                entry.name
            );
            for check in &entry.check {
                if let Some(path) = check.url.strip_prefix('/') {
                    assert!(
                        package.join("suite-fixtures").join(path).is_file(),
                        "{} checks a missing fixture {path}",
                        entry.name
                    );
                }
            }
        }
    }

    #[test]
    fn only_unallowed_api_gaps_fail_a_run() {
        let output = "LOADED Ok(..)\n\
            [abcdefghijklmnopabcdefghijklmnop ERROR] [worker] TypeError: x is undefined\n\
            [abcdefghijklmnopabcdefghijklmnop ERROR] [worker] Error: Invalid call to runtime.connectNative().\n\
            [abcdefghijklmnopabcdefghijklmnop ERROR] [worker] Failed to fetch\n\
            RESULT popup \"Welcome\"\nDONE\n";
        let problems = evaluate(&expectation(true, &["connectNative"]), output);
        assert_eq!(problems, ["[worker] TypeError: x is undefined"]);
        assert!(evaluate(&expectation(true, &["connectNative", "TypeError"]), output).is_empty());
    }

    #[test]
    fn a_closed_tab_is_not_an_api_gap() {
        let output = "LOADED Ok(..)\n\
            [abcdefghijklmnopabcdefghijklmnop ERROR] [worker] Invalid call to tabs.sendMessage(). Tab not found.\n\
            DONE\n";
        assert!(evaluate(&expectation(false, &[]), output).is_empty());
    }

    #[test]
    fn a_check_passes_only_when_the_page_changed() {
        let mut entry = expectation(false, &[]);
        entry.check = vec![check_on("/login.html")];
        let passed = "LOADED Ok(..)\nRESULT check0 true\nDONE\n";
        assert!(evaluate(&entry, passed).is_empty());
        let failed = "LOADED Ok(..)\nRESULT check0 \"falsy\"\nDONE\n";
        assert_eq!(evaluate(&entry, failed), ["did nothing on /login.html"]);
        let thrown = "LOADED Ok(..)\nRESULT check0 \"TypeError: no\"\nDONE\n";
        assert_eq!(
            evaluate(&entry, thrown),
            ["did nothing on /login.html (TypeError: no)"]
        );
    }

    #[test]
    fn a_missing_popup_or_an_unfinished_run_fails() {
        let problems = evaluate(
            &expectation(true, &[]),
            "LOADED Ok(..)\nRESULT popup \"\"\n",
        );
        assert_eq!(
            problems,
            ["did not finish in time", "popup rendered nothing"]
        );
    }

    #[test]
    fn a_popup_must_say_what_it_should_and_not_what_it_shouldnt() {
        let mut entry = expectation(true, &[]);
        entry.popup_has = vec!["Log in".into()];
        entry.popup_lacks = vec!["difficulties activating".into()];
        let good = "LOADED Ok(..)\nRESULT popup \"Welcome. LOG IN\"\nDONE\n";
        assert!(evaluate(&entry, good).is_empty());
        let broken = "LOADED Ok(..)\nRESULT popup \"Your browser is having difficulties activating X\"\nDONE\n";
        assert_eq!(
            evaluate(&entry, broken),
            [
                "popup says \"difficulties activating\": Your browser is having difficulties activating X",
                "popup lacks \"Log in\": Your browser is having difficulties activating X"
            ]
        );
    }

    #[test]
    fn logged_lines_are_counted_per_extension() {
        let mut entry = expectation(false, &[]);
        entry.logs = vec![LogLine {
            has: "monitoring".into(),
            min: 2,
        }];
        let id = &entry.id;
        let output = format!(
            "LOADED Ok(..)\n[{id} warn] [worker] Autofill monitoring disabled\n[other warn] monitoring\nDONE\n"
        );
        assert_eq!(
            evaluate(&entry, &output),
            ["logged \"monitoring\" 1 times, expected 2"]
        );
    }
}
