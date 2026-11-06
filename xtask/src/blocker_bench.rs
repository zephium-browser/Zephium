//! `cargo xtask blocker-bench`: loads real sites in Zephium with protection
//! off and on, and in Brave and Firefox with uBlock Origin, and compares page
//! speed, CPU, memory, network and how many ads stay visible.
//!
//! Every browser is driven over the line protocol in `protocol.rs`:
//! `scripts/blocker-bench/driver.mjs` speaks CDP and WebDriver BiDi for Brave,
//! Edge, Chrome and Firefox, and the Zephium protection QA app
//! (`--blocker-bench`) speaks it itself. Each configuration gets a fresh
//! profile, and the configuration order rotates from run to run so none always
//! goes first. Raw samples go to `target/blocker-bench/<label>/results.json`
//! and the tables to `summary.md` beside it.
//! uBlock Origin and the Brave and Edge template profiles are kept in
//! `target/blocker-bench/` and refreshed weekly.
//!
//! Build the app with release code generation (the `protection-qa` profile),
//! or numbers against other browsers and absolute startup times are unfair.
//! Build the frontend once with `pnpm -C frame run build`, then:
//!
//! ```text
//! TAURI_CONFIG="$(cat desktop/tauri.protection-qa.conf.json)" \
//!   cargo build -p zephium-desktop --profile protection-qa --features adblock-qa
//! ```
//!
//! and pass `--zephium target/protection-qa/zephium-desktop` (`.exe` on
//! Windows, where PowerShell sets `$env:TAURI_CONFIG` from the same file). A
//! debug build (`CARGO_PROFILE_DEV_OPT_LEVEL=2`, same command without
//! `--profile`) is enough for on/off comparisons.

mod measure;
mod protocol;
mod results;
mod sites;
mod stats;
mod summary;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use measure::Traffic;
use protocol::{Driver, Failure, REPLY_GRACE};
use results::{Launch, Results, SpeedometerRun, Visit, YoutubeStep, SCHEMA};
use sites::{ConsentMode, Recipe, Site};

const OPEN_TIMEOUT_MS: u64 = 30_000;
const EVAL_TIMEOUT_MS: u64 = 20_000;
/// Above the Zephium driver's own waits: 120 s for its policy, then 120 s for
/// its list sources, plus startup.
const READY_WITHIN: Duration = Duration::from_secs(300);
const REFRESH_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// What a Chromium template profile runs on `about:blank` so the browser
/// finishes the background downloads (Brave's Shields lists among them) that
/// a brand-new profile would otherwise fetch while the first page loads.
const TEMPLATE_WARMUP: Duration = Duration::from_secs(60);
const TEMPLATE_MARKER: &str = ".blocker-bench-template";
const CONSENT_SETTLE: Duration = Duration::from_secs(2);
const BETWEEN_SITES: Duration = Duration::from_secs(1);
/// How long a watch page opened behind another tab is left before it is read.
const BACKGROUND_WAIT: Duration = Duration::from_secs(15);
const UBLOCK_URL: &str =
    "https://addons.mozilla.org/firefox/downloads/latest/ublock-origin/latest.xpi";
/// What Firefox gets to finish after it is ready. uBlock Origin downloads its
/// list updates after a temporary install, so its configuration gets longer.
const FIREFOX_SETTLE_MS: u64 = 5000;
const UBLOCK_SETTLE_MS: u64 = 30_000;
/// Every top-level document after the driver is ready.
const PRELOAD: &str = include_str!("blocker_bench/preload.js");
const WATCH_SECONDS: u64 = 30;
/// Each of the two seeks after the watch gets this long to play again.
const SEEK_WITHIN_MS: u64 = 15_000;
const YOUTUBE_DIALOG_WAIT_MS: u64 = 4000;
const QA_IDENTIFIER: &str = "app.zephium.protection-qa";
/// Speedometer starts itself when it is given `startAutomatically`.
const SPEEDOMETER_URL: &str = "https://browserbench.org/Speedometer3.1/?startAutomatically=true";
const SPEEDOMETER_POLL: Duration = Duration::from_secs(5);
const SPEEDOMETER_CAP: Duration = Duration::from_secs(10 * 60);
// A fresh Zephium profile compiles its lists again when the list update it
// fetches at launch lands; by default every browser idles this long so no run
// overlaps that.
const SPEEDOMETER_SETTLE: Duration = Duration::from_secs(20);
/// The score is in the page once `#result-number` has text; a failed run
/// writes "Error" there.
const SPEEDOMETER_STATE: &str = "const text = (id) => document.getElementById(id)?.textContent.trim() ?? '';\n\
    const score = text('result-number');\n\
    const m = score && window.benchmarkClient && window.benchmarkClient.metrics;\n\
    const suites = m ? Object.fromEntries(Object.entries(m).filter(([k]) => !k.includes('/')).map(([k, v]) => [k, v.mean])) : null;\n\
    return { score, range: text('confidence-number'), progress: text('info-progress'), suites };";

/// Long-lived, monetized music videos, and one of over twenty minutes.
const VIDEOS: [(&str, &str); 5] = [
    ("dQw4w9WgXcQ", "Rick Astley, Never Gonna Give You Up"),
    ("kJQP7kiw5Fk", "Luis Fonsi, Despacito"),
    ("9bZkp7q19f0", "PSY, Gangnam Style"),
    ("JGwWNGJdvx8", "Ed Sheeran, Shape of You"),
    (
        "0e3GPea1Tyg",
        "MrBeast, Squid Game in real life (over 20 minutes)",
    ),
];
const SHORT: &str = "41UzbuW6wJs";

/// Scrolls to the bottom in steps over about four seconds, back to the top,
/// and waits for what the scrolling loaded.
const SCROLL: &str = "const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));\n\
    const steps = 16;\n\
    for (let step = 1; step <= steps; step += 1) {\n\
      const height = Math.max(document.documentElement.scrollHeight, document.body?.scrollHeight ?? 0);\n\
      scrollTo({ top: ((height - innerHeight) * step) / steps, behavior: 'instant' });\n\
      await sleep(250);\n\
    }\n\
    scrollTo({ top: 0, behavior: 'instant' });\n\
    await sleep(1500);\n\
    return document.visibilityState;";

/// YouTube asks a visitor from the EU either on `consent.youtube.com`, where
/// the form that saves "Reject all" sets `set_eom` to true and the one that
/// saves "Accept all" sets it to false, or in a dialog on the page, where those
/// are its first and second buttons; neither depends on the page's language.
/// The dialog is waited for up to `__WAIT__` milliseconds, until the button
/// the answer needs is there.
const YOUTUBE_CONSENT: &str = include_str!("blocker_bench/youtube-consent.js");

/// Whether YouTube still asks: the consent page, or the dialog `YOUTUBE_CONSENT`
/// answers.
const YOUTUBE_CONSENT_PRESENT: &str = "return location.hostname.startsWith('consent.')\n\
    || document.querySelector('ytd-consent-bump-v2-lightbox .eom-buttons button') !== null;";

/// The bench's answer to YouTube's consent page or dialog.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum YoutubeConsent {
    #[default]
    Reject,
    Accept,
}

impl YoutubeConsent {
    const fn name(self) -> &'static str {
        match self {
            YoutubeConsent::Reject => "reject",
            YoutubeConsent::Accept => "accept",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Config {
    ZephiumOff,
    ZephiumOn,
    Brave,
    /// Brave with Shields down on every site: the same engine and lists, none applied.
    BraveOff,
    Edge,
    Chrome,
    Firefox,
    FirefoxUbo,
}

impl Config {
    const DEFAULT: [Config; 4] = [
        Config::ZephiumOff,
        Config::ZephiumOn,
        Config::Brave,
        Config::FirefoxUbo,
    ];

    fn name(self) -> &'static str {
        match self {
            Config::ZephiumOff => summary::ZEPHIUM_OFF,
            Config::ZephiumOn => summary::ZEPHIUM_ON,
            Config::Brave => "brave",
            Config::BraveOff => "brave-off",
            Config::Edge => "edge",
            Config::Chrome => "chrome",
            Config::Firefox => "firefox",
            Config::FirefoxUbo => "firefox-ubo",
        }
    }

    fn parse(name: &str) -> Option<Config> {
        [
            Config::ZephiumOff,
            Config::ZephiumOn,
            Config::Brave,
            Config::BraveOff,
            Config::Edge,
            Config::Chrome,
            Config::Firefox,
            Config::FirefoxUbo,
        ]
        .into_iter()
        .find(|config| config.name() == name)
    }

    fn zephium(self) -> bool {
        matches!(self, Config::ZephiumOff | Config::ZephiumOn)
    }

    fn chromium(self) -> bool {
        matches!(
            self,
            Config::Brave | Config::BraveOff | Config::Edge | Config::Chrome
        )
    }

    /// The configuration whose template profile this one starts from.
    fn template(self) -> Config {
        match self {
            Config::BraveOff => Config::Brave,
            other => other,
        }
    }

    fn firefox(self) -> bool {
        matches!(self, Config::Firefox | Config::FirefoxUbo)
    }

    /// Short enough for a QA session label.
    fn token(self) -> &'static str {
        match self {
            Config::ZephiumOff => "zoff",
            Config::ZephiumOn => "zon",
            Config::Brave => "brave",
            Config::BraveOff => "braveoff",
            Config::Edge => "edge",
            Config::Chrome => "chrome",
            Config::Firefox => "ff",
            Config::FirefoxUbo => "ffubo",
        }
    }
}

#[derive(Debug, Default)]
struct Options {
    browsers: Vec<Config>,
    runs: usize,
    only: Vec<String>,
    settle: Duration,
    youtube: bool,
    youtube_consent: YoutubeConsent,
    startup: Option<usize>,
    speedometer: Option<usize>,
    speedometer_url: Option<String>,
    speedometer_settle: Duration,
    /// Keep each browser's profile (Zephium's QA session, the others' profile
    /// directory) across Speedometer runs, after one uncounted warm-up run, as a
    /// returning user's caches would be.
    speedometer_warm: bool,
    label: Option<String>,
    compare: Option<PathBuf>,
    zephium: Option<PathBuf>,
    brave: Option<PathBuf>,
    edge: Option<PathBuf>,
    chrome: Option<PathBuf>,
    firefox: Option<PathBuf>,
    /// Replaces the driver program for every configuration; for harness tests.
    driver_command: Vec<String>,
    no_net: bool,
    no_warmup: bool,
    allow_builds: bool,
}

impl Options {
    fn parse(arguments: &[String]) -> Result<Options, String> {
        let mut options = Options {
            browsers: Config::DEFAULT.to_vec(),
            runs: 3,
            settle: Duration::from_millis(6000),
            speedometer_settle: SPEEDOMETER_SETTLE,
            speedometer_warm: false,
            ..Options::default()
        };
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            let (flag, inline) = match argument.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_string())),
                None => (argument.as_str(), None),
            };
            let mut value = || {
                inline
                    .clone()
                    .or_else(|| arguments.next().cloned())
                    .ok_or_else(|| format!("{flag} needs a value"))
            };
            let path = |text: String| Some(PathBuf::from(text));
            match flag {
                "--browsers" => {
                    options.browsers = value()?
                        .split(',')
                        .map(|name| {
                            Config::parse(name.trim())
                                .ok_or_else(|| format!("unknown browser {name:?}"))
                        })
                        .collect::<Result<_, _>>()?;
                    if options.browsers.is_empty() {
                        return Err("--browsers is empty".into());
                    }
                }
                "--runs" => options.runs = number(&value()?, "--runs")?.max(1) as usize,
                "--only" => {
                    options.only = value()?
                        .split(',')
                        .map(|name| name.trim().to_lowercase())
                        .collect();
                }
                "--settle-ms" => options.settle = Duration::from_millis(number(&value()?, flag)?),
                "--startup" => options.startup = Some(number(&value()?, flag)?.max(1) as usize),
                "--speedometer" => {
                    options.speedometer = Some(number(&value()?, flag)?.max(1) as usize);
                }
                "--speedometer-url" => options.speedometer_url = Some(value()?),
                "--speedometer-warm" => options.speedometer_warm = true,
                "--speedometer-settle-ms" => {
                    options.speedometer_settle = Duration::from_millis(number(&value()?, flag)?);
                }
                "--label" => options.label = Some(value()?),
                "--compare" => options.compare = path(value()?),
                "--zephium" => options.zephium = path(value()?),
                "--brave" => options.brave = path(value()?),
                "--edge" => options.edge = path(value()?),
                "--chrome" => options.chrome = path(value()?),
                "--firefox" => options.firefox = path(value()?),
                "--driver-command" => {
                    options.driver_command =
                        value()?.split_whitespace().map(str::to_string).collect();
                }
                "--youtube" => options.youtube = true,
                "--youtube-consent" => {
                    options.youtube_consent = match value()?.as_str() {
                        "reject" => YoutubeConsent::Reject,
                        "accept" => YoutubeConsent::Accept,
                        other => {
                            return Err(format!(
                                "--youtube-consent takes accept or reject, not {other:?}"
                            ));
                        }
                    };
                }
                "--no-net" => options.no_net = true,
                "--no-warmup" => options.no_warmup = true,
                "--allow-builds" => options.allow_builds = true,
                _ => return Err(format!("unknown option {argument:?}")),
            }
        }
        let mut seen = Vec::new();
        for config in &options.browsers {
            if seen.contains(config) {
                return Err(format!("{} is listed twice", config.name()));
            }
            seen.push(*config);
        }
        let passes = [
            options.youtube,
            options.startup.is_some(),
            options.speedometer.is_some(),
        ];
        if passes.into_iter().filter(|pass| *pass).count() > 1 {
            return Err("--youtube, --startup and --speedometer are separate passes".into());
        }
        if options.speedometer_warm && options.speedometer.is_none() {
            return Err("--speedometer-warm needs --speedometer".into());
        }
        if let Some(label) = &options.label {
            let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
            if label.is_empty() || label.starts_with('.') || !label.chars().all(plain) {
                return Err("--label may hold letters, digits, - _ and . only".into());
            }
        }
        Ok(options)
    }

    fn mode(&self) -> &'static str {
        if self.youtube {
            "youtube"
        } else if self.startup.is_some() {
            "startup"
        } else if self.speedometer.is_some() {
            "speedometer"
        } else {
            "sites"
        }
    }
}

fn number(text: &str, flag: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|_| format!("{flag} needs a whole number, not {text:?}"))
}

struct Context {
    options: Options,
    repository: PathBuf,
    cache: PathBuf,
    out: PathBuf,
    executables: BTreeMap<&'static str, PathBuf>,
    xpi: Option<PathBuf>,
    templates: BTreeMap<&'static str, PathBuf>,
}

/// A running browser. The driver goes before the profile it is using.
struct Session {
    driver: Driver,
    pid: u32,
    ready: Value,
    /// Whether the driver took the document-start script; older ones do not.
    preload: bool,
    /// What the driver waited after Firefox was ready.
    settle_ms: Option<u64>,
    /// Bytes received per pid when the last visit ended, which the next visit
    /// counts from: one `nettop` per page instead of two.
    traffic: Option<HashMap<u32, u64>>,
    _profile: Option<tempfile::TempDir>,
}

/// What each page visit needs to know about the browser it runs in.
struct Plan {
    zephium: bool,
    net: bool,
    settle: Duration,
    youtube_consent: YoutubeConsent,
}

pub(crate) fn run(arguments: &[String]) -> Result<(), String> {
    let options = Options::parse(arguments)?;
    let old = options.compare.as_deref().map(Results::load).transpose()?;
    if !cfg!(any(target_os = "macos", windows)) {
        return Err("the blocker benchmark runs on macOS and Windows".into());
    }
    let builds = running_builds()?;
    if !builds.is_empty() && !options.allow_builds {
        return Err(format!(
            "a build is running (pid {}), which would ruin the numbers; wait for it",
            builds
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let _awake = Caffeinate::start();

    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository.join("target"));
    let cache = target.join("blocker-bench");
    let generated = now();
    let label = options
        .label
        .clone()
        .unwrap_or_else(|| utc_stamp(generated).replace(['-', ':'], ""));
    let out = cache.join(&label);
    std::fs::create_dir_all(out.join("logs")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(cache.join("profiles")).map_err(|e| e.to_string())?;

    let mut sites = sites::all()?;
    if !options.only.is_empty() {
        for name in &options.only {
            if !sites.iter().any(|site| site.name == *name) {
                return Err(format!("no site named {name:?}"));
            }
        }
        sites.retain(|site| options.only.contains(&site.name));
    }

    let mut context = Context {
        executables: executables(&options)?,
        options,
        repository,
        cache,
        out,
        xpi: None,
        templates: BTreeMap::new(),
    };
    if context.options.browsers.contains(&Config::FirefoxUbo) {
        context.xpi = Some(fetch_ublock(&context.cache)?);
    }
    prepare_templates(&mut context)?;

    let mut results = Results {
        schema: SCHEMA,
        label,
        generated,
        mode: context.options.mode().to_string(),
        configs: context
            .options
            .browsers
            .iter()
            .map(|c| c.name().to_string())
            .collect(),
        sites: sites.iter().map(|site| site.name.clone()).collect(),
        machine: machine(&context),
        options: json!({
            "runs": context.options.runs,
            "settleMs": context.options.settle.as_millis() as u64,
            "startup": context.options.startup,
            "speedometer": context.options.speedometer,
            "speedometerWarm": context.options.speedometer_warm,
            "youtubeConsent": context.options.youtube_consent.name(),
            "net": !context.options.no_net,
            "warmup": !context.options.no_warmup,
            "buildsRunning": builds,
        }),
        ..Results::default()
    };
    let saved = context.out.join("results.json");

    let outcome = if context.options.youtube {
        run_youtube(&context, &mut results, &saved)
    } else if let Some(runs) = context.options.speedometer {
        run_speedometer(&context, &mut results, &saved, runs)
    } else if let Some(launches) = context.options.startup {
        run_startup(&context, &mut results, &saved, launches)
    } else {
        run_sites(&context, &mut results, &saved, &sites)
    };
    results.save(&saved)?;
    let summary = summary::render(&results);
    std::fs::write(context.out.join("summary.md"), &summary).map_err(|e| e.to_string())?;
    println!("{summary}");
    if let Some(old) = &old {
        println!("{}", summary::compare(old, &results));
    }
    eprintln!("results: {}", saved.display());
    outcome
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `2026-11-02T13:05:01Z`.
fn utc_stamp(seconds: u64) -> String {
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The configurations, rotated one place further for each run.
fn rotated<T: Clone>(items: &[T], run: usize) -> Vec<T> {
    let mut items = items.to_vec();
    if !items.is_empty() {
        let by = run % items.len();
        items.rotate_left(by);
    }
    items
}

/// A cargo or rustc running anywhere but above this process, which `cargo
/// xtask` itself is below.
fn running_builds() -> Result<Vec<u32>, String> {
    let table = measure::process_table()?;
    let mut ancestors = vec![std::process::id()];
    while let Some(row) = table.iter().find(|row| Some(&row.pid) == ancestors.last()) {
        if row.ppid == 0 || ancestors.contains(&row.ppid) {
            break;
        }
        ancestors.push(row.ppid);
    }
    Ok(table
        .iter()
        .filter(|row| !ancestors.contains(&row.pid))
        .filter(|row| {
            let program = row.name.rsplit(['/', '\\']).next().unwrap_or("");
            matches!(program, "cargo" | "rustc" | "cargo.exe" | "rustc.exe")
        })
        .map(|row| row.pid)
        .collect())
}

/// Keeps the display and the machine awake for as long as the run lasts.
struct Caffeinate(Option<Child>);

impl Caffeinate {
    fn start() -> Caffeinate {
        if !cfg!(target_os = "macos") {
            return Caffeinate(None);
        }
        Caffeinate(
            Command::new("caffeinate")
                .args(["-d", "-i", "-w", &std::process::id().to_string()])
                .stdin(Stdio::null())
                .spawn()
                .ok(),
        )
    }
}

impl Drop for Caffeinate {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn executables(options: &Options) -> Result<BTreeMap<&'static str, PathBuf>, String> {
    let mut found = BTreeMap::new();
    for config in &options.browsers {
        let path = match config {
            Config::ZephiumOff | Config::ZephiumOn => options.zephium.clone().ok_or(
                "zephium-off and zephium-on need --zephium <path to the protection QA app binary>",
            )?,
            Config::Brave | Config::BraveOff => options
                .brave
                .clone()
                .or_else(|| default_executable(*config))
                .ok_or("no default Brave path here; pass --brave")?,
            Config::Edge => options
                .edge
                .clone()
                .or_else(|| default_executable(*config))
                .ok_or("no default Edge path here; pass --edge")?,
            Config::Chrome => options
                .chrome
                .clone()
                .or_else(|| default_executable(*config))
                .ok_or("no default Chrome path here; pass --chrome")?,
            Config::Firefox | Config::FirefoxUbo => options
                .firefox
                .clone()
                .or_else(|| default_executable(*config))
                .ok_or("no default Firefox path here; pass --firefox")?,
        };
        if options.driver_command.is_empty() && !path.is_file() {
            return Err(format!(
                "{}: {} does not exist",
                config.name(),
                path.display()
            ));
        }
        found.insert(config.name(), path);
    }
    Ok(found)
}

fn default_executable(config: Config) -> Option<PathBuf> {
    if cfg!(windows) {
        let under = |variable: &str, path: &str| {
            std::env::var_os(variable).map(|root| PathBuf::from(root).join(path))
        };
        match config {
            Config::Brave | Config::BraveOff => under(
                "ProgramFiles",
                r"BraveSoftware\Brave-Browser\Application\brave.exe",
            ),
            Config::Edge => under(
                "ProgramFiles(x86)",
                r"Microsoft\Edge\Application\msedge.exe",
            ),
            Config::Chrome => under("ProgramFiles", r"Google\Chrome\Application\chrome.exe"),
            Config::Firefox | Config::FirefoxUbo => {
                under("ProgramFiles", r"Mozilla Firefox\firefox.exe")
            }
            _ => None,
        }
    } else {
        let path = match config {
            Config::Brave | Config::BraveOff => {
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"
            }
            Config::Edge => "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            Config::Chrome => "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            Config::Firefox | Config::FirefoxUbo => {
                "/Applications/Firefox.app/Contents/MacOS/firefox"
            }
            _ => return None,
        };
        Some(PathBuf::from(path))
    }
}

fn younger_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed < age)
}

fn fetch_ublock(cache: &Path) -> Result<PathBuf, String> {
    let path = cache.join("ublock-origin.xpi");
    if younger_than(&path, REFRESH_AFTER) {
        return Ok(path);
    }
    let partial = cache.join("ublock-origin.xpi.part");
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "120", "-o"])
        .arg(&partial)
        .arg(UBLOCK_URL)
        .status()
        .map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&partial).unwrap_or_default();
    if !status.success() || !bytes.starts_with(b"PK") {
        let _ = std::fs::remove_file(&partial);
        return Err("could not download uBlock Origin".into());
    }
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// One template profile per Chromium browser, so a measured run starts from a
/// profile that has already had its first-run downloads.
fn prepare_templates(context: &mut Context) -> Result<(), String> {
    for config in context.options.browsers.clone() {
        let config = config.template();
        if !config.chromium() || context.templates.contains_key(config.name()) {
            continue;
        }
        let template = context.cache.join(format!("{}-template", config.name()));
        if !younger_than(&template.join(TEMPLATE_MARKER), REFRESH_AFTER) {
            eprintln!(
                "preparing the {} template profile ({} s)",
                config.name(),
                TEMPLATE_WARMUP.as_secs()
            );
            let _ = std::fs::remove_dir_all(&template);
            std::fs::create_dir_all(&template).map_err(|e| e.to_string())?;
            let command = context.command(config, &template, None)?;
            let log = std::fs::File::create(
                context
                    .out
                    .join("logs")
                    .join(format!("{}-template.log", config.name())),
            )
            .map_err(|e| e.to_string())?;
            let (mut driver, _) = Driver::start(command, Stdio::from(log), READY_WITHIN)
                .map_err(|e| format!("{} template: {e}", config.name()))?;
            std::thread::sleep(TEMPLATE_WARMUP);
            driver.quit();
            drop(driver);
            std::fs::write(template.join(TEMPLATE_MARKER), now().to_string())
                .map_err(|e| e.to_string())?;
        }
        context.templates.insert(config.name(), template);
    }
    Ok(())
}

/// Turns Brave's Shields down by default in a profile before it starts:
/// `braveShields` is a content setting, where block means down.
fn shields_down(profile: &Path) -> Result<(), String> {
    let path = profile.join("Default").join("Preferences");
    std::fs::create_dir_all(profile.join("Default")).map_err(|e| e.to_string())?;
    let mut preferences: Value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
        Err(_) => json!({}),
    };
    let object = |value: &mut Value, key: &str| -> Result<(), String> {
        let map = value.as_object_mut().ok_or("not a JSON object")?;
        map.entry(key).or_insert_with(|| json!({}));
        Ok(())
    };
    object(&mut preferences, "profile")?;
    object(
        &mut preferences["profile"],
        "default_content_setting_values",
    )?;
    preferences["profile"]["default_content_setting_values"]["braveShields"] = json!(2);
    std::fs::write(
        &path,
        serde_json::to_vec(&preferences).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Copies a profile, leaving out the locks and the port file a running
/// browser keeps in it.
fn copy_profile(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text.starts_with("Singleton")
            || name_text == "DevToolsActivePort"
            || name_text == TEMPLATE_MARKER
        {
            continue;
        }
        let kind = entry.file_type()?;
        let target = to.join(&name);
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_profile(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// A throwaway Zephium QA session, deleted with its guard. The app keeps
/// every session under `qa-sessions/` in its own data directory.
struct QaSession {
    label: String,
}

impl QaSession {
    fn new(config: Config, tag: &str) -> QaSession {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let label = format!(
            "bb{:x}{:x}{:x}-{}-{tag}",
            std::process::id(),
            now(),
            COUNT.fetch_add(1, Ordering::Relaxed),
            config.token()
        );
        QaSession { label }
    }
}

impl Drop for QaSession {
    fn drop(&mut self) {
        static WARNED: AtomicBool = AtomicBool::new(false);
        let Some(root) = qa_root() else { return };
        match remove_qa_session(&root, &self.label) {
            Ok(true) => {}
            Ok(false) => {
                if !WARNED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "note: no QA session directory under {} for {}",
                        root.display(),
                        self.label
                    );
                }
            }
            Err(error) => eprintln!("could not delete QA session {}: {error}", self.label),
        }
    }
}

/// The protection QA app's data directory, as Tauri's `app_data_dir` gives it.
fn qa_root() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join(QA_IDENTIFIER))
    } else {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library/Application Support")
                .join(QA_IDENTIFIER)
        })
    }
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 48
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Deletes `qa-sessions/session-<label>` under `root` and nothing else;
/// whether there was one to delete.
fn remove_qa_session(root: &Path, label: &str) -> Result<bool, String> {
    if !valid_label(label) {
        return Err("not a QA session label".into());
    }
    let sessions = root.join("qa-sessions");
    let target = sessions.join(format!("session-{label}"));
    if !target.is_dir() {
        return Ok(false);
    }
    let sessions = std::fs::canonicalize(&sessions).map_err(|e| e.to_string())?;
    let target = std::fs::canonicalize(&target).map_err(|e| e.to_string())?;
    if target.parent() != Some(sessions.as_path()) {
        return Err(format!(
            "{} is not directly under the QA sessions",
            target.display()
        ));
    }
    std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    Ok(true)
}

impl Context {
    /// The command that starts a driver (or, for Zephium, the app itself).
    fn command(
        &self,
        config: Config,
        profile: &Path,
        qa: Option<&QaSession>,
    ) -> Result<Command, String> {
        let executable = &self.executables[config.name()];
        let mut command = if let Some((program, rest)) = self.options.driver_command.split_first() {
            let mut command = Command::new(program);
            command.args(rest);
            command
        } else if config.zephium() {
            Command::new(executable)
        } else {
            let mut command = Command::new("node");
            command.arg(self.repository.join("scripts/blocker-bench/driver.mjs"));
            command
        };
        command.current_dir(&self.repository);
        if config.zephium() {
            let qa = qa.ok_or("a Zephium launch needs a QA session")?;
            command.args([
                "--blocker-bench".to_string(),
                format!(
                    "--blocker-bench-protection={}",
                    if config == Config::ZephiumOn {
                        "on"
                    } else {
                        "off"
                    }
                ),
                format!("--blocker-bench-session={}", qa.label),
            ]);
            // The shipped app gets this from LSEnvironment; a direct launch
            // must match it or the footprint keeps freed large buffers.
            if cfg!(target_os = "macos") {
                command.env("MallocLargeCache", "0");
            }
        } else {
            command
                .args([
                    "--browser",
                    if config.firefox() {
                        "firefox"
                    } else {
                        "chromium"
                    },
                ])
                .arg("--executable")
                .arg(executable)
                .arg("--profile")
                .arg(profile);
            if config == Config::FirefoxUbo {
                let xpi = self.xpi.as_ref().ok_or("uBlock Origin is not downloaded")?;
                command.arg("--extension").arg(xpi);
            }
            if let Some(settle) = self.driver_settle_ms(config) {
                command.args(["--settle-ms", &settle.to_string()]);
            }
        }
        Ok(command)
    }

    /// Only a startup pass has no use for the wait.
    fn driver_settle_ms(&self, config: Config) -> Option<u64> {
        config.firefox().then(|| {
            if self.options.startup.is_some() {
                0
            } else if config == Config::FirefoxUbo {
                UBLOCK_SETTLE_MS
            } else {
                FIREFOX_SETTLE_MS
            }
        })
    }

    /// Starts `config` on a fresh profile; `qa` is the Zephium session.
    fn launch(&self, config: Config, qa: Option<&QaSession>, log: &str) -> Result<Session, String> {
        let profile = if config.zephium() {
            None
        } else {
            Some(self.profile(config)?)
        };
        self.launch_in(config, qa, profile, None, log)
    }

    /// A new profile directory for a non-Zephium browser, from its template.
    fn profile(&self, config: Config) -> Result<tempfile::TempDir, String> {
        let directory = tempfile::Builder::new()
            .prefix(&format!("{}-", config.name()))
            .tempdir_in(self.cache.join("profiles"))
            .map_err(|e| e.to_string())?;
        if let Some(template) = self.templates.get(config.template().name()) {
            copy_profile(template, directory.path())
                .map_err(|e| format!("copying the template profile: {e}"))?;
        }
        if config == Config::BraveOff {
            shields_down(directory.path())
                .map_err(|e| format!("turning Brave's Shields down: {e}"))?;
        }
        Ok(directory)
    }

    /// Starts `config` in `profile`, owned by the session, or in `kept`, a
    /// directory the caller keeps across launches.
    fn launch_in(
        &self,
        config: Config,
        qa: Option<&QaSession>,
        profile: Option<tempfile::TempDir>,
        kept: Option<&Path>,
        log: &str,
    ) -> Result<Session, String> {
        let directory = kept
            .or_else(|| profile.as_ref().map(tempfile::TempDir::path))
            .unwrap_or(Path::new(""));
        let command = self.command(config, directory, qa)?;
        let stderr = std::fs::File::create(self.out.join("logs").join(format!("{log}.log")))
            .map_err(|e| e.to_string())?;
        let (mut driver, ready) = Driver::start(command, Stdio::from(stderr), READY_WITHIN)?;
        let preload =
            match driver.call("preload", json!({ "js": PRELOAD }), Duration::from_secs(30)) {
                Ok(_) => true,
                Err(failure) if failure.is_gone() => return Err(failure.message().to_string()),
                Err(_) => false,
            };
        let pid = ready
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or("the ready event has no pid")?;
        Ok(Session {
            driver,
            pid,
            ready,
            preload,
            settle_ms: self.driver_settle_ms(config),
            traffic: None,
            _profile: profile,
        })
    }

    fn plan(&self, config: Config) -> Plan {
        Plan {
            zephium: config.zephium(),
            net: !self.options.no_net,
            settle: self.options.settle,
            youtube_consent: self.options.youtube_consent,
        }
    }
}

fn record_session(launch: &mut Launch, session: &Session) {
    launch.ready = Some(session.ready.clone());
    launch.preload = Some(session.preload);
    launch.settle_ms = session.settle_ms;
}

fn eval(driver: &mut Driver, tab: &Value, js: &str, timeout_ms: u64) -> Result<Value, Failure> {
    let reply = driver.call(
        "eval",
        json!({ "tab": tab, "js": js, "timeoutMs": timeout_ms }),
        Duration::from_millis(timeout_ms) + REPLY_GRACE,
    )?;
    Ok(reply.get("value").cloned().unwrap_or(Value::Null))
}

struct Opened {
    tab: Value,
    load_ms: Option<f64>,
    timed_out: bool,
}

fn open(driver: &mut Driver, url: &str) -> Result<Opened, Failure> {
    let reply = driver.call(
        "open",
        json!({ "url": url, "timeoutMs": OPEN_TIMEOUT_MS }),
        Duration::from_millis(OPEN_TIMEOUT_MS) + REPLY_GRACE,
    )?;
    Ok(Opened {
        tab: reply.get("tab").cloned().unwrap_or(Value::Null),
        load_ms: reply.get("loadMs").and_then(Value::as_f64),
        timed_out: reply
            .get("timedOut")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// As [`open`], in a new tab behind the current one.
fn open_behind(driver: &mut Driver, url: &str) -> Result<Opened, Failure> {
    let reply = driver.call(
        "open",
        json!({ "url": url, "timeoutMs": OPEN_TIMEOUT_MS, "background": true }),
        Duration::from_millis(OPEN_TIMEOUT_MS) + REPLY_GRACE,
    )?;
    Ok(Opened {
        tab: reply.get("tab").cloned().unwrap_or(Value::Null),
        load_ms: reply.get("loadMs").and_then(Value::as_f64),
        timed_out: reply
            .get("timedOut")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn navigate(driver: &mut Driver, tab: &Value, url: &str) -> Result<Opened, Failure> {
    let reply = driver.call(
        "navigate",
        json!({ "tab": tab, "url": url, "timeoutMs": OPEN_TIMEOUT_MS }),
        // The app may first wait out an earlier load, then this one.
        Duration::from_millis(2 * OPEN_TIMEOUT_MS) + REPLY_GRACE,
    )?;
    Ok(Opened {
        tab: tab.clone(),
        load_ms: reply.get("loadMs").and_then(Value::as_f64),
        timed_out: reply
            .get("timedOut")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn close(driver: &mut Driver, tab: &Value) -> Result<(), Failure> {
    driver.call(
        "close",
        json!({ "tab": tab, "timeoutMs": EVAL_TIMEOUT_MS }),
        Duration::from_millis(EVAL_TIMEOUT_MS) + REPLY_GRACE,
    )?;
    Ok(())
}

/// Today's blocked count, which only the Zephium app reports.
fn blocked(driver: &mut Driver) -> Result<Option<u64>, Failure> {
    match driver.call("blocked", json!({}), Duration::from_secs(30)) {
        Ok(reply) => Ok(reply.get("value").and_then(Value::as_u64)),
        Err(failure) if failure.is_gone() => Err(failure),
        Err(_) => Ok(None),
    }
}

/// Keeps a failed step out of the way of the rest of a visit: only a browser
/// that is gone stops it.
fn soft<T>(
    errors: &mut Vec<String>,
    step: &str,
    result: Result<T, Failure>,
) -> Result<Option<T>, Failure> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(failure) if failure.is_gone() => Err(failure),
        Err(failure) => {
            errors.push(format!("{step}: {}", failure.message()));
            Ok(None)
        }
    }
}

fn megabytes(bytes: u64) -> f64 {
    (bytes as f64 / 1_048_576.0 * 10.0).round() / 10.0
}

fn tenths(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[derive(Default)]
struct ConsentOutcome {
    shown: bool,
    cmp: Option<String>,
    clicked: Option<String>,
    cleared: Option<bool>,
}

/// How long to wait for a wall: as long as the page settles, which a site
/// without one spends waiting anyway.
fn consent_wait_ms(settle: Duration) -> u64 {
    (settle.as_millis() as u64).max(sites::MIN_CONSENT_WAIT_MS)
}

/// Accepts a site's consent wall the way its recipe says, then looks two
/// seconds later for the wall.
fn accept_consent(
    driver: &mut Driver,
    tab: &Value,
    recipe: Option<&Recipe>,
    wait_ms: u64,
) -> Result<ConsentOutcome, Failure> {
    let script = sites::consent_script(recipe, ConsentMode::Act, wait_ms);
    let act = match eval(driver, tab, &script, wait_ms + EVAL_TIMEOUT_MS) {
        // A page that redirects after it has loaded interrupts the first try.
        Err(failure) if !failure.is_gone() => {
            std::thread::sleep(Duration::from_secs(1));
            eval(driver, tab, &script, wait_ms + EVAL_TIMEOUT_MS)?
        }
        other => other?,
    };
    let text = |key: &str| act.get(key).and_then(Value::as_str).map(str::to_string);
    let mut outcome = ConsentOutcome {
        shown: act.get("shown").and_then(Value::as_bool).unwrap_or(false),
        cmp: text("cmp"),
        clicked: text("clicked"),
        cleared: None,
    };
    if outcome.shown {
        if outcome.clicked.is_some() {
            std::thread::sleep(CONSENT_SETTLE);
        }
        outcome.cleared = Some(!wall_present(driver, tab, recipe)?);
    }
    Ok(outcome)
}

fn wall_present(
    driver: &mut Driver,
    tab: &Value,
    recipe: Option<&Recipe>,
) -> Result<bool, Failure> {
    let check = eval(
        driver,
        tab,
        &sites::consent_script(recipe, ConsentMode::Check, 0),
        EVAL_TIMEOUT_MS,
    )?;
    Ok(check
        .get("present")
        .and_then(Value::as_bool)
        .unwrap_or(true))
}

struct YoutubeConsentOutcome {
    answered: bool,
    /// Whether YouTube stopped asking, checked after the answer settled.
    cleared: bool,
    /// Set where the answer sent the browser back to the page.
    reloaded: Option<Opened>,
}

fn youtube_consent_script(wait_ms: u64, answer: YoutubeConsent) -> String {
    YOUTUBE_CONSENT
        .replacen("__WAIT__", &wait_ms.to_string(), 1)
        .replacen(
            "__ACCEPT__",
            &(answer == YoutubeConsent::Accept).to_string(),
            1,
        )
}

fn youtube_consent_present(driver: &mut Driver, tab: &Value) -> Result<bool, Failure> {
    Ok(eval(driver, tab, YOUTUBE_CONSENT_PRESENT, EVAL_TIMEOUT_MS)?
        .as_bool()
        .unwrap_or(true))
}

/// Gives `answer` on YouTube's consent page or dialog, if there is one;
/// from the consent page the browser is sent back to `url`.
fn answer_youtube_consent(
    driver: &mut Driver,
    tab: &Value,
    url: &str,
    wait_ms: u64,
    answer: YoutubeConsent,
) -> Result<Option<YoutubeConsentOutcome>, Failure> {
    let seen = eval(
        driver,
        tab,
        &youtube_consent_script(wait_ms, answer),
        EVAL_TIMEOUT_MS,
    )?;
    let answered = seen.get("answered").and_then(Value::as_bool) == Some(true);
    match seen.get("page").and_then(Value::as_str) {
        Some("consent") => {
            if answered {
                std::thread::sleep(CONSENT_SETTLE);
            }
            let reloaded = navigate(driver, tab, url)?;
            Ok(Some(YoutubeConsentOutcome {
                answered,
                cleared: !youtube_consent_present(driver, tab)?,
                reloaded: Some(reloaded),
            }))
        }
        Some("dialog") => {
            std::thread::sleep(CONSENT_SETTLE);
            Ok(Some(YoutubeConsentOutcome {
                answered,
                cleared: !youtube_consent_present(driver, tab)?,
                reloaded: None,
            }))
        }
        _ => Ok(None),
    }
}

/// One site loaded once. The browser being gone is the second value.
fn measure_site(
    session: &mut Session,
    plan: &Plan,
    config: Config,
    run: usize,
    site: &Site,
) -> (Visit, bool) {
    let mut visit = Visit {
        config: config.name().to_string(),
        run,
        site: site.name.clone(),
        kind: site.kind.clone(),
        url: site.url.clone(),
        ..Visit::default()
    };
    let gone = match measure_steps(session, plan, site, &mut visit) {
        Ok(()) => false,
        Err(failure) => {
            visit.errors.push(failure.message().to_string());
            failure.is_gone()
        }
    };
    (visit, gone)
}

fn measure_steps(
    session: &mut Session,
    plan: &Plan,
    site: &Site,
    visit: &mut Visit,
) -> Result<(), Failure> {
    let Session {
        driver,
        pid: root,
        traffic: last_traffic,
        ..
    } = session;
    let root = *root;
    let traffic_before = last_traffic.take();
    let before = measure::sample(root)
        .map_err(|error| visit.errors.push(format!("sampling: {error}")))
        .ok();
    visit.footprint_before_mb = before.as_ref().map(|b| megabytes(b.footprint_bytes));
    if plan.zephium {
        visit.blocked_before = blocked(driver)?;
    }

    let opened = match open(driver, &site.url) {
        Ok(opened) => opened,
        Err(failure) if failure.is_gone() => return Err(failure),
        Err(failure) => {
            visit.errors.push(format!("open: {}", failure.message()));
            return Ok(());
        }
    };
    let tab = opened.tab.clone();
    visit.nav_ms = opened.load_ms;
    visit.timed_out = opened.timed_out;
    let opened_at = Instant::now();

    let youtube = site.url.contains("youtube.com");
    let recipe = site.consent.as_ref();
    let steps = (|| -> Result<(), Failure> {
        if youtube {
            if let Some(again) = soft(
                &mut visit.errors,
                "consent",
                answer_youtube_consent(
                    driver,
                    &tab,
                    &site.url,
                    YOUTUBE_DIALOG_WAIT_MS,
                    plan.youtube_consent,
                ),
            )?
            .flatten()
            {
                visit.consent_shown = true;
                visit.consent_cmp = Some("youtube".into());
                if again.answered {
                    visit.consent_clicked = Some(format!("{} all", plan.youtube_consent.name()));
                }
                visit.consent_cleared = Some(again.cleared);
                if let Some(reloaded) = again.reloaded {
                    visit.nav_ms = reloaded.load_ms;
                    visit.timed_out |= reloaded.timed_out;
                }
            }
        } else if let Some(outcome) = soft(
            &mut visit.errors,
            "consent",
            accept_consent(driver, &tab, recipe, consent_wait_ms(plan.settle)),
        )? {
            visit.consent_shown = outcome.shown;
            visit.consent_cmp = outcome.cmp;
            visit.consent_clicked = outcome.clicked;
            visit.consent_cleared = outcome.cleared;
        }
        std::thread::sleep(plan.settle.saturating_sub(opened_at.elapsed()));
        let scrolled = soft(
            &mut visit.errors,
            "scroll",
            eval(driver, &tab, SCROLL, EVAL_TIMEOUT_MS),
        )?;
        visit.visibility = scrolled.and_then(|state| state.as_str().map(str::to_string));

        let after = measure::sample(root)
            .map_err(|error| visit.errors.push(format!("sampling: {error}")))
            .ok();
        // Started after the last timed sample, so its five seconds fall outside
        // every window that is measured.
        let traffic = plan.net.then(Traffic::start);
        if let (Some(before), Some(after)) = (&before, &after) {
            visit.cpu_ms = Some(tenths(after.cpu_ms_since(before)));
        }
        if let Some(after) = &after {
            visit.footprint_mb = Some(megabytes(after.footprint_bytes));
            visit.processes = Some(after.processes());
        }

        // A wall that came up late still counts against the numbers.
        if !youtube {
            if let Some(present) = soft(
                &mut visit.errors,
                "consent check",
                wall_present(driver, &tab, recipe),
            )? {
                if present {
                    visit.consent_shown = true;
                    visit.consent_cleared = Some(false);
                }
            }
        }
        let probe = eval(driver, &tab, &sites::probe_script(site), EVAL_TIMEOUT_MS);
        visit.probe = soft(&mut visit.errors, "probe", probe)?.filter(Value::is_object);
        if plan.zephium {
            visit.blocked_after = blocked(driver)?;
            visit.blocked = visit
                .blocked_before
                .zip(visit.blocked_after)
                .map(|(before, after)| after.saturating_sub(before));
        }
        if let (Some(traffic), Some(after)) = (traffic, &after) {
            if let Some(traffic_after) = traffic.finish() {
                if let Some(before) = &traffic_before {
                    visit.net_kb = Some(tenths(measure::net_kb(
                        before,
                        &traffic_after,
                        after.pids(),
                    )));
                }
                *last_traffic = Some(traffic_after);
            }
        }
        Ok(())
    })();
    let closed = close(driver, &tab);
    steps?;
    soft(&mut visit.errors, "close", closed)?;
    std::thread::sleep(BETWEEN_SITES);
    Ok(())
}

/// Opens every selected site once, so the first lookups and connections of
/// the run do not fall on whichever configuration goes first.
fn warm_up(context: &Context, config: Config, sites: &[Site]) {
    eprintln!("warming up with {} on {} sites", config.name(), sites.len());
    let qa = config.zephium().then(|| QaSession::new(config, "warm"));
    let mut session =
        match context.launch(config, qa.as_ref(), &format!("warmup-{}", config.name())) {
            Ok(session) => session,
            Err(error) => {
                eprintln!("warm-up: {} did not start: {error}", config.name());
                return;
            }
        };
    let driver = &mut session.driver;
    for site in sites {
        let Ok(opened) = open(driver, &site.url) else {
            continue;
        };
        let step = if site.url.contains("youtube.com") {
            answer_youtube_consent(
                driver,
                &opened.tab,
                &site.url,
                YOUTUBE_DIALOG_WAIT_MS,
                context.options.youtube_consent,
            )
            .map(|_| ())
        } else {
            accept_consent(
                driver,
                &opened.tab,
                site.consent.as_ref(),
                consent_wait_ms(context.options.settle),
            )
            .map(|_| ())
        };
        let closed = close(driver, &opened.tab);
        if step.is_err_and(|f| f.is_gone()) || closed.is_err_and(|f| f.is_gone()) {
            break;
        }
        std::thread::sleep(BETWEEN_SITES);
    }
    session.driver.quit();
}

fn run_sites(
    context: &Context,
    results: &mut Results,
    saved: &Path,
    sites: &[Site],
) -> Result<(), String> {
    if !context.options.no_warmup {
        warm_up(context, context.options.browsers[0], sites);
    }
    for run in 1..=context.options.runs {
        for config in rotated(&context.options.browsers, run - 1) {
            let name = config.name();
            eprintln!("run {run}/{} {name}", context.options.runs);
            let qa = config
                .zephium()
                .then(|| QaSession::new(config, &format!("r{run}")));
            let mut launch = Launch {
                config: name.to_string(),
                run,
                phase: "run".into(),
                ..Launch::default()
            };
            let mut session = match context.launch(config, qa.as_ref(), &format!("{name}-r{run}")) {
                Ok(session) => session,
                Err(error) => {
                    eprintln!("  {name} did not start: {error}");
                    launch.error = Some(error);
                    results.launches.push(launch);
                    continue;
                }
            };
            record_session(&mut launch, &session);
            results.launches.push(launch);
            let plan = context.plan(config);
            if plan.net {
                session.traffic = Traffic::start().finish();
            }
            for site in sites {
                let (visit, gone) = measure_site(&mut session, &plan, config, run, site);
                eprintln!("  {}", visit_line(&visit));
                results.visits.push(visit);
                if gone {
                    break;
                }
            }
            session.driver.quit();
            drop(session);
            drop(qa);
            results.save(saved)?;
        }
    }
    Ok(())
}

fn visit_line(visit: &Visit) -> String {
    let value = |number: Option<f64>| number.map_or("-".to_string(), |n| format!("{n:.0}"));
    let load = visit
        .probe
        .as_ref()
        .and_then(|probe| probe.get("load")?.as_f64())
        .filter(|load| *load > 0.0);
    format!(
        "{:<16} load {:>6} ms  cpu {:>6} ms  mem {:>5} MB  net {:>6} KB{}{}",
        visit.site,
        value(load),
        value(visit.cpu_ms),
        value(visit.footprint_mb),
        value(visit.net_kb),
        if visit.walled() { "  WALL" } else { "" },
        visit
            .errors
            .first()
            .map_or(String::new(), |error| format!("  ! {error}")),
    )
}

/// The YouTube pass: the home feed, two videos opened in the page from a
/// watch page, five videos, one Shorts page and a video behind another tab.
fn run_youtube(context: &Context, results: &mut Results, saved: &Path) -> Result<(), String> {
    for run in 1..=context.options.runs {
        for config in rotated(&context.options.browsers, run - 1) {
            let name = config.name();
            eprintln!("run {run}/{} {name}", context.options.runs);
            let qa = config
                .zephium()
                .then(|| QaSession::new(config, &format!("y{run}")));
            let mut launch = Launch {
                config: name.to_string(),
                run,
                phase: "run".into(),
                ..Launch::default()
            };
            let mut session =
                match context.launch(config, qa.as_ref(), &format!("{name}-youtube-r{run}")) {
                    Ok(session) => session,
                    Err(error) => {
                        eprintln!("  {name} did not start: {error}");
                        launch.error = Some(error);
                        results.launches.push(launch);
                        continue;
                    }
                };
            record_session(&mut launch, &session);
            results.launches.push(launch);
            let plan = context.plan(config);
            let feed = sites::YOUTUBE_FEED.to_string();
            let watch = sites::watch_script(WATCH_SECONDS, SEEK_WITHIN_MS);
            let inpage = sites::inpage_script(WATCH_SECONDS);
            let mut pages: Vec<(&str, Option<&str>, String, &str, u64)> = vec![
                (
                    "feed",
                    None,
                    "https://www.youtube.com/".to_string(),
                    &feed,
                    EVAL_TIMEOUT_MS + 10_000,
                ),
                (
                    "inpage",
                    Some(VIDEOS[0].0),
                    format!("https://www.youtube.com/watch?v={}", VIDEOS[0].0),
                    &inpage,
                    2 * (WATCH_SECONDS * 1000 + 35_000) + 25_000,
                ),
            ];
            for (id, _) in VIDEOS {
                pages.push((
                    "watch",
                    Some(id),
                    format!("https://www.youtube.com/watch?v={id}"),
                    &watch,
                    WATCH_SECONDS * 1000 + 2 * SEEK_WITHIN_MS + 15_000,
                ));
            }
            pages.push((
                "shorts",
                Some(SHORT),
                format!("https://www.youtube.com/shorts/{SHORT}"),
                sites::YOUTUBE_SHORTS,
                60_000,
            ));
            // Every step waits for YouTube's dialog until one was answered or
            // a full wait saw none, so a dialog the feed missed is still met
            // and later pages are timed alike in every browser.
            let mut consent_pending = true;
            let mut browser_gone = false;
            for (step, id, url, script, timeout_ms) in pages {
                let (record, gone) = youtube_step(
                    &mut session,
                    &plan,
                    (config, run),
                    (step, id, &url),
                    (script, timeout_ms),
                    &mut consent_pending,
                );
                eprintln!(
                    "  {step:<7} {}  {}",
                    id.unwrap_or(""),
                    record
                        .result
                        .as_ref()
                        .map_or(record.errors.join("; "), Value::to_string)
                );
                results.youtube.push(record);
                if gone {
                    browser_gone = true;
                    break;
                }
            }
            if !browser_gone {
                let (record, _) = youtube_background_step(&mut session, (config, run));
                eprintln!(
                    "  background  {}",
                    record
                        .result
                        .as_ref()
                        .map_or(record.errors.join("; "), Value::to_string)
                );
                results.youtube.push(record);
            }
            session.driver.quit();
            drop(session);
            drop(qa);
            results.save(saved)?;
        }
    }
    Ok(())
}

/// A watch page opened behind the home feed and read after
/// `BACKGROUND_WAIT`: nobody asked it to play, so nothing may reload or
/// re-mark it.
fn youtube_background_step(
    session: &mut Session,
    (config, run): (Config, usize),
) -> (YoutubeStep, bool) {
    let id = VIDEOS[1].0;
    let url = format!("https://www.youtube.com/watch?v={id}");
    let mut record = YoutubeStep {
        config: config.name().to_string(),
        run,
        step: "background".into(),
        id: Some(id.to_string()),
        url: url.clone(),
        ..YoutubeStep::default()
    };
    let driver = &mut session.driver;
    let outcome = (|| -> Result<(), Failure> {
        let Some(front) = soft(
            &mut record.errors,
            "open front",
            open(driver, "https://www.youtube.com/"),
        )?
        else {
            return Ok(());
        };
        if let Some(behind) = soft(&mut record.errors, "open behind", open_behind(driver, &url))? {
            record.nav_ms = behind.load_ms;
            record.timed_out = behind.timed_out;
            std::thread::sleep(BACKGROUND_WAIT);
            let result = eval(
                driver,
                &behind.tab,
                sites::YOUTUBE_BACKGROUND,
                EVAL_TIMEOUT_MS,
            );
            record.result = soft(&mut record.errors, "background", result)?;
            let closed = close(driver, &behind.tab);
            soft(&mut record.errors, "close", closed)?;
        }
        let closed = close(driver, &front.tab);
        soft(&mut record.errors, "close", closed)?;
        std::thread::sleep(BETWEEN_SITES);
        Ok(())
    })();
    let gone = match outcome {
        Ok(()) => false,
        Err(failure) => {
            record.errors.push(failure.message().to_string());
            failure.is_gone()
        }
    };
    (record, gone)
}

fn youtube_step(
    session: &mut Session,
    plan: &Plan,
    (config, run): (Config, usize),
    (step, id, url): (&str, Option<&str>, &str),
    (script, timeout_ms): (&str, u64),
    consent_pending: &mut bool,
) -> (YoutubeStep, bool) {
    let mut record = YoutubeStep {
        config: config.name().to_string(),
        run,
        step: step.to_string(),
        id: id.map(str::to_string),
        url: url.to_string(),
        ..YoutubeStep::default()
    };
    let root = session.pid;
    let driver = &mut session.driver;
    let outcome = (|| -> Result<(), Failure> {
        let before = measure::sample(root).ok();
        let blocked_before = if plan.zephium { blocked(driver)? } else { None };
        let opened = match open(driver, url) {
            Ok(opened) => opened,
            Err(failure) if failure.is_gone() => return Err(failure),
            Err(failure) => {
                record.errors.push(format!("open: {}", failure.message()));
                return Ok(());
            }
        };
        let tab = opened.tab.clone();
        record.nav_ms = opened.load_ms;
        record.timed_out = opened.timed_out;
        let page = (|| -> Result<(), Failure> {
            let opened_at = Instant::now();
            let wait_ms = if *consent_pending {
                YOUTUBE_DIALOG_WAIT_MS
            } else {
                0
            };
            match soft(
                &mut record.errors,
                "consent",
                answer_youtube_consent(driver, &tab, url, wait_ms, plan.youtube_consent),
            )? {
                Some(Some(again)) => {
                    record.consent_answered = again.answered;
                    *consent_pending &= !again.answered;
                    if let Some(reloaded) = again.reloaded {
                        record.nav_ms = reloaded.load_ms;
                        record.timed_out |= reloaded.timed_out;
                    }
                }
                Some(None) if wait_ms > 0 => *consent_pending = false,
                _ => {}
            }
            if step != "watch" {
                std::thread::sleep(plan.settle.saturating_sub(opened_at.elapsed()));
            }
            let result = eval(driver, &tab, script, timeout_ms);
            record.result = soft(&mut record.errors, step, result)?;
            if let (Some(before), Ok(after)) = (&before, measure::sample(root)) {
                record.cpu_ms = Some(tenths(after.cpu_ms_since(before)));
                record.footprint_mb = Some(megabytes(after.footprint_bytes));
            }
            if plan.zephium {
                record.blocked = blocked_before
                    .zip(blocked(driver)?)
                    .map(|(before, after)| after.saturating_sub(before));
            }
            Ok(())
        })();
        let closed = close(driver, &tab);
        page?;
        soft(&mut record.errors, "close", closed)?;
        std::thread::sleep(BETWEEN_SITES);
        Ok(())
    })();
    let gone = match outcome {
        Ok(()) => false,
        Err(failure) => {
            record.errors.push(failure.message().to_string());
            failure.is_gone()
        }
    };
    (record, gone)
}

/// Speedometer, `runs` times in each configuration, each on a fresh profile.
/// Nothing else is opened or measured while a run is going: the page is polled
/// every few seconds and the processes are read before and after.
fn run_speedometer(
    context: &Context,
    results: &mut Results,
    saved: &Path,
    runs: usize,
) -> Result<(), String> {
    let url = context
        .options
        .speedometer_url
        .as_deref()
        .unwrap_or(SPEEDOMETER_URL);
    let warm = context.options.speedometer_warm;
    // Warm runs keep each browser's profile: Zephium's QA session, or the
    // other browsers' profile directory.
    let mut kept_sessions: BTreeMap<&str, QaSession> = BTreeMap::new();
    let mut kept_profiles: BTreeMap<&str, tempfile::TempDir> = BTreeMap::new();
    // A browser whose warm-up gave no score is not measured as warm.
    let mut unwarmed: Vec<&str> = Vec::new();
    if warm {
        for config in &context.options.browsers {
            let name = config.name();
            if config.zephium() {
                kept_sessions.insert(name, QaSession::new(*config, "sw"));
                continue;
            }
            match context.profile(*config) {
                Ok(profile) => {
                    kept_profiles.insert(name, profile);
                }
                Err(error) => {
                    eprintln!("  {name}: no profile to keep warm: {error}");
                    results.launches.push(Launch {
                        config: name.to_string(),
                        phase: "warm-up".into(),
                        error: Some(error),
                        ..Launch::default()
                    });
                    unwarmed.push(name);
                }
            }
        }
    }
    for run in usize::from(!warm)..=runs {
        for config in rotated(&context.options.browsers, run.saturating_sub(1)) {
            let name = config.name();
            if unwarmed.contains(&name) {
                continue;
            }
            if run == 0 {
                eprintln!("warm-up {name}: leave the machine alone");
            } else {
                eprintln!("run {run}/{runs} {name}: leave the machine alone");
            }
            let fresh =
                (!warm && config.zephium()).then(|| QaSession::new(config, &format!("s{run}")));
            let qa = fresh.as_ref().or_else(|| kept_sessions.get(name));
            let mut launch = Launch {
                config: name.to_string(),
                run,
                phase: if run == 0 { "warm-up" } else { "run" }.into(),
                ..Launch::default()
            };
            let launched = (!warm && !config.zephium())
                .then(|| context.profile(config))
                .transpose()
                .and_then(|profile| {
                    context.launch_in(
                        config,
                        qa,
                        profile,
                        kept_profiles.get(name).map(tempfile::TempDir::path),
                        &format!("{name}-speedometer-r{run}"),
                    )
                });
            let mut session = match launched {
                Ok(session) => session,
                Err(error) => {
                    eprintln!("  {name} did not start: {error}");
                    launch.error = Some(error);
                    results.launches.push(launch);
                    if run == 0 {
                        unwarmed.push(name);
                    }
                    continue;
                }
            };
            record_session(&mut launch, &session);
            let record = speedometer_run(
                &mut session,
                config,
                run,
                url,
                context.options.speedometer_settle,
            );
            eprintln!(
                "  score {}  range {}  {} s{}",
                record.score_text.as_deref().unwrap_or("-"),
                record.range_text.as_deref().unwrap_or("-"),
                record
                    .duration_s
                    .map_or("-".to_string(), |s| format!("{s:.0}")),
                record
                    .errors
                    .first()
                    .map_or(String::new(), |error| format!("  ! {error}")),
            );
            if run > 0 {
                results.launches.push(launch);
                results.speedometer.push(record);
            } else if record.score.is_none() {
                // Only a failed warm-up is kept, so the Launches table says why
                // this browser has no warm runs.
                eprintln!("  {name}: the warm-up gave no score, so its warm runs are skipped");
                launch.error = Some(
                    record
                        .errors
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "the warm-up gave no score".into()),
                );
                results.launches.push(launch);
                unwarmed.push(name);
            }
            session.driver.quit();
            drop(session);
            drop(fresh);
            results.save(saved)?;
        }
    }
    Ok(())
}

fn speedometer_run(
    session: &mut Session,
    config: Config,
    run: usize,
    url: &str,
    settle: Duration,
) -> SpeedometerRun {
    let mut record = SpeedometerRun {
        config: config.name().to_string(),
        run,
        url: url.to_string(),
        ..SpeedometerRun::default()
    };
    let (root, driver) = (session.pid, &mut session.driver);
    let outcome = (|| -> Result<(), Failure> {
        std::thread::sleep(settle);
        let before = measure::sample(root).ok();
        record.footprint_before_mb = before.as_ref().map(|b| megabytes(b.footprint_bytes));
        let blocked_before = if config.zephium() {
            blocked(driver)?
        } else {
            None
        };
        let opened = open(driver, url)?;
        record.nav_ms = opened.load_ms;
        let started = Instant::now();
        let mut last = Value::Null;
        let finished = loop {
            std::thread::sleep(SPEEDOMETER_POLL);
            record.polls += 1;
            match eval(driver, &opened.tab, SPEEDOMETER_STATE, EVAL_TIMEOUT_MS) {
                Ok(state) => last = state,
                Err(failure) if failure.is_gone() => return Err(failure),
                // A page this busy can miss a poll.
                Err(failure) => record.errors.push(format!("poll: {}", failure.message())),
            }
            let text = |key: &str| {
                last.get(key)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string)
            };
            if let Some(score) = text("score") {
                record.range = text("range").and_then(|text| number_in(&text));
                record.range_text = text("range");
                record.score = number_in(&score);
                record.score_text = Some(score);
                record.suites = last
                    .get("suites")
                    .cloned()
                    .filter(|suites| !suites.is_null());
                break true;
            }
            if started.elapsed() >= SPEEDOMETER_CAP {
                break false;
            }
        };
        record.capped = !finished;
        record.duration_s = Some(tenths(started.elapsed().as_secs_f64()));
        if let (Some(before), Ok(after)) = (&before, measure::sample(root)) {
            record.cpu_ms = Some(tenths(after.cpu_ms_since(before)));
            record.footprint_mb = Some(megabytes(after.footprint_bytes));
        }
        if config.zephium() {
            record.blocked = blocked_before
                .zip(blocked(driver)?)
                .map(|(before, after)| after.saturating_sub(before));
        }
        close(driver, &opened.tab)
    })();
    if let Err(failure) = outcome {
        record.errors.push(failure.message().to_string());
    }
    record
}

/// The first number in text such as `± 1.2` or `1,234.5`.
fn number_in(text: &str) -> Option<f64> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let digits: String = text[start..]
        .chars()
        .filter(|c| *c != ',')
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.parse().ok()
}

/// Launch-and-quit, `launches` times for each configuration. A Zephium
/// session is launched once more first, cold, because the first launch on a
/// new session compiles the lists; the rest are the warm launches users see.
fn run_startup(
    context: &Context,
    results: &mut Results,
    saved: &Path,
    launches: usize,
) -> Result<(), String> {
    let configs = &context.options.browsers;
    let sessions: BTreeMap<&str, QaSession> = configs
        .iter()
        .filter(|config| config.zephium())
        .map(|config| (config.name(), QaSession::new(*config, "st")))
        .collect();
    for round in 0..=launches {
        for config in rotated(configs, round) {
            if round == 0 && !config.zephium() {
                continue;
            }
            let phase = match (config.zephium(), round) {
                (true, 0) => "cold",
                (true, _) => "warm",
                _ => "fresh",
            };
            let name = config.name();
            let mut launch = Launch {
                config: name.to_string(),
                run: round,
                phase: phase.into(),
                ..Launch::default()
            };
            match context.launch(
                config,
                sessions.get(name),
                &format!("{name}-startup-{round}"),
            ) {
                Ok(mut session) => {
                    eprintln!("{name} {phase} {round}: {}", session.ready);
                    record_session(&mut launch, &session);
                    session.driver.quit();
                }
                Err(error) => {
                    eprintln!("{name} {phase} {round}: {error}");
                    launch.error = Some(error);
                }
            }
            results.launches.push(launch);
            results.save(saved)?;
        }
    }
    Ok(())
}

fn command_text(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(arguments)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// The version of the app a browser binary sits in, read without starting it.
fn app_version(executable: &Path) -> Option<String> {
    if cfg!(windows) {
        let script = format!(
            "(Get-Item -LiteralPath '{}').VersionInfo.ProductVersion",
            executable.display()
        );
        return command_text("powershell", &["-NoProfile", "-Command", &script]);
    }
    let bundle = executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))?;
    command_text(
        "defaults",
        &[
            "read",
            &bundle.join("Contents/Info").to_string_lossy(),
            "CFBundleShortVersionString",
        ],
    )
}

fn machine(context: &Context) -> Value {
    let git = |arguments: &[&str]| {
        let mut full = vec!["-C", context.repository.to_str().unwrap_or(".")];
        full.extend_from_slice(arguments);
        command_text("git", &full)
    };
    let (model, cpu, memory, os) = if cfg!(windows) {
        let query = |class: &str, field: &str| {
            command_text(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    &format!("(Get-CimInstance {class}).{field}"),
                ],
            )
        };
        (
            query("Win32_ComputerSystem", "Model"),
            query("Win32_Processor", "Name"),
            query("Win32_ComputerSystem", "TotalPhysicalMemory")
                .and_then(|bytes| bytes.parse::<u64>().ok()),
            query("Win32_OperatingSystem", "Caption")
                .zip(query("Win32_OperatingSystem", "Version"))
                .map(|(caption, version)| format!("{caption} {version}")),
        )
    } else {
        (
            command_text("sysctl", &["-n", "hw.model"]),
            command_text("sysctl", &["-n", "machdep.cpu.brand_string"]),
            command_text("sysctl", &["-n", "hw.memsize"])
                .and_then(|bytes| bytes.parse::<u64>().ok()),
            command_text("sw_vers", &["-productVersion"]).map(|version| format!("macOS {version}")),
        )
    };
    let mut browsers = serde_json::Map::new();
    for config in &context.options.browsers {
        let executable = &context.executables[config.name()];
        let version = if config.zephium() {
            std::fs::metadata(executable).ok().and_then(|metadata| {
                let modified = metadata
                    .modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()?
                    .as_secs();
                Some(format!(
                    "binary built {}, {} bytes",
                    utc_stamp(modified),
                    metadata.len()
                ))
            })
        } else {
            app_version(executable)
        };
        browsers.insert(config.name().to_string(), json!(version));
    }
    let ublock = context.xpi.as_ref().and_then(|xpi| {
        let manifest = command_text("unzip", &["-p", &xpi.to_string_lossy(), "manifest.json"])
            .or_else(|| command_text("tar", &["-xOf", &xpi.to_string_lossy(), "manifest.json"]))?;
        serde_json::from_str::<Value>(&manifest)
            .ok()?
            .get("version")?
            .as_str()
            .map(str::to_string)
    });
    json!({
        "model": model,
        "cpu": cpu,
        "memoryGb": memory.map(|bytes| bytes / (1 << 30)),
        "os": os,
        "gitRevision": git(&["rev-parse", "--short=12", "HEAD"]),
        "gitBranch": git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        "gitDirty": git(&["status", "--porcelain"]).is_some(),
        "node": command_text("node", &["--version"]),
        "browsers": browsers,
        "ublockOrigin": ublock,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    /// A page with only what the consent script reads: the consent page's two
    /// forms, or the dialog's buttons, of which the second (accept) shows up
    /// `late_ms` after the first when that is set.
    const CONSENT_FIXTURE: &str = "const [kind, lateMs] = JSON.parse(process.argv[1]);
        const clicked = [];
        const button = (name) => ({ name, click() { clicked.push(name); } });
        const form = (eom, name) => {
          const submit = button(name);
          return {
            querySelector(selector) {
              if (selector === 'button, input[type=submit]') return submit;
              const value = /^input\\[name=set_eom\\]\\[value=(true|false)\\]$/.exec(selector);
              return value && value[1] === eom ? {} : null;
            },
          };
        };
        const started = Date.now();
        const buttons = () => (Date.now() - started >= lateMs ? [button('reject'), button('accept')] : [button('reject')]);
        globalThis.location = { hostname: kind === 'page' ? 'consent.youtube.com' : 'www.youtube.com' };
        globalThis.document = {
          querySelectorAll(selector) {
            if (selector === 'form') return kind === 'page' ? [form('true', 'reject'), form('false', 'accept')] : [];
            if (selector === 'ytd-consent-bump-v2-lightbox .eom-buttons button') return kind === 'dialog' ? buttons() : [];
            return [];
          },
        };
        const answer = async () => { __SCRIPT__ };
        answer().then((result) => console.log(JSON.stringify({ result, clicked })));";

    /// What the consent script returns and clicks on `kind`, run in Node.
    fn run_consent(kind: &str, late_ms: u64, answer: YoutubeConsent) -> Value {
        let script =
            CONSENT_FIXTURE.replacen("__SCRIPT__", &youtube_consent_script(2000, answer), 1);
        let output = Command::new("node")
            .args(["-e", &script, &json!([kind, late_ms]).to_string()])
            .output()
            .expect("node runs the consent fixture");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("the fixture prints JSON")
    }

    #[test]
    fn the_consent_script_submits_the_form_that_saves_the_answer() {
        for (answer, name) in [
            (YoutubeConsent::Reject, "reject"),
            (YoutubeConsent::Accept, "accept"),
        ] {
            let seen = run_consent("page", 0, answer);
            assert_eq!(seen["clicked"], json!([name]), "{seen}");
            assert_eq!(
                seen["result"],
                json!({ "page": "consent", "answered": true })
            );
        }
    }

    #[test]
    fn the_consent_script_clicks_the_dialog_button_of_the_answer() {
        for (answer, name) in [
            (YoutubeConsent::Reject, "reject"),
            (YoutubeConsent::Accept, "accept"),
        ] {
            let seen = run_consent("dialog", 0, answer);
            assert_eq!(seen["clicked"], json!([name]), "{seen}");
            assert_eq!(
                seen["result"],
                json!({ "page": "dialog", "answered": true })
            );
        }
    }

    #[test]
    fn accepting_waits_for_the_dialog_s_second_button() {
        let seen = run_consent("dialog", 600, YoutubeConsent::Accept);
        assert_eq!(seen["clicked"], json!(["accept"]), "{seen}");
        // Rejecting needs only the first one, which is there at once.
        let seen = run_consent("dialog", 600, YoutubeConsent::Reject);
        assert_eq!(seen["clicked"], json!(["reject"]), "{seen}");
    }

    #[test]
    fn a_dialog_that_never_offers_the_answer_is_left_unanswered() {
        let seen = run_consent("dialog", 60_000, YoutubeConsent::Accept);
        assert_eq!(seen["clicked"], json!([]), "{seen}");
        assert_eq!(
            seen["result"],
            json!({ "page": "dialog", "answered": false })
        );
        let seen = run_consent("none", 0, YoutubeConsent::Reject);
        assert_eq!(seen["result"], json!({ "page": "none", "answered": false }));
    }

    #[test]
    fn options_have_the_documented_defaults() {
        let options = Options::parse(&[]).unwrap();
        assert_eq!(options.runs, 3);
        assert_eq!(options.settle, Duration::from_millis(6000));
        assert_eq!(options.browsers, Config::DEFAULT);
        assert_eq!(options.mode(), "sites");
        assert_eq!(options.youtube_consent, YoutubeConsent::Reject);
        assert!(!options.allow_builds);
    }

    #[test]
    fn options_read_values_in_both_spellings() {
        let mut given = arguments(
            "--browsers brave,firefox --runs=2 --only Wikipedia,cnn --settle-ms 2000 --youtube --zephium /x/qa --driver-command",
        );
        given.push("node /y/stub.mjs".into());
        let options = Options::parse(&given).unwrap();
        assert_eq!(options.browsers, [Config::Brave, Config::Firefox]);
        assert_eq!(options.runs, 2);
        assert_eq!(options.only, ["wikipedia", "cnn"]);
        assert_eq!(options.settle, Duration::from_millis(2000));
        assert_eq!(options.mode(), "youtube");
        assert_eq!(options.zephium, Some(PathBuf::from("/x/qa")));
        assert_eq!(options.driver_command, ["node", "/y/stub.mjs"]);
    }

    #[test]
    fn bad_options_are_refused() {
        for bad in [
            "--browsers brave,safari",
            "--browsers brave,brave",
            "--runs many",
            "--settle-ms",
            "--frobnicate",
            "--youtube --startup 3",
            "--speedometer 2 --youtube",
            "--label ../x",
            "--speedometer-warm",
            "--youtube-consent maybe",
            "--youtube-consent",
        ] {
            assert!(Options::parse(&arguments(bad)).is_err(), "{bad}");
        }
        assert_eq!(
            Options::parse(&arguments("--startup 4")).unwrap().startup,
            Some(4)
        );
        assert_eq!(
            Options::parse(&arguments("--youtube --youtube-consent accept"))
                .unwrap()
                .youtube_consent,
            YoutubeConsent::Accept
        );
    }

    #[test]
    fn runs_rotate_the_order_so_nobody_always_goes_first() {
        let configs = Config::DEFAULT.to_vec();
        let firsts: Vec<Config> = (0..4).map(|run| rotated(&configs, run)[0]).collect();
        assert_eq!(firsts, configs);
        assert_eq!(rotated(&configs, 4), configs);
        assert_eq!(rotated::<u8>(&[], 3), Vec::<u8>::new());
    }

    #[test]
    fn speedometer_scores_are_read_from_the_page_text() {
        assert_eq!(number_in("123.45"), Some(123.45));
        assert_eq!(number_in("\u{b1} 1.2"), Some(1.2));
        assert_eq!(number_in("1,234.5"), Some(1234.5));
        assert_eq!(number_in("Error"), None);
        assert_eq!(number_in(""), None);
    }

    #[test]
    fn utc_stamps_are_calendar_dates() {
        assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp(1_762_088_701), "2025-11-02T13:05:01Z");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn session_labels_are_valid_and_unique() {
        let first = QaSession::new(Config::ZephiumOn, "r3");
        let second = QaSession::new(Config::ZephiumOn, "r3");
        assert!(valid_label(&first.label), "{}", first.label);
        assert_ne!(first.label, second.label);
        assert!(!valid_label(""));
        assert!(!valid_label("../x"));
        assert!(!valid_label(&"a".repeat(49)));
        std::mem::forget((first, second));
    }

    #[test]
    fn only_the_named_qa_session_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("qa-sessions");
        std::fs::create_dir_all(sessions.join("session-abc/data")).unwrap();
        std::fs::create_dir_all(sessions.join("session-keep")).unwrap();
        std::fs::write(root.path().join("precious"), "x").unwrap();
        assert_eq!(remove_qa_session(root.path(), "abc"), Ok(true));
        assert!(!sessions.join("session-abc").exists());
        assert!(sessions.join("session-keep").exists());
        assert_eq!(remove_qa_session(root.path(), "abc"), Ok(false));
        assert!(remove_qa_session(root.path(), "..").is_err());
        assert!(remove_qa_session(root.path(), "a/b").is_err());
        assert!(root.path().join("precious").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_session_that_is_a_link_elsewhere_is_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("qa-sessions")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), root.path().join("qa-sessions/session-x"))
            .unwrap();
        assert!(remove_qa_session(root.path(), "x").is_err());
        assert!(elsewhere.path().exists());
    }

    #[test]
    fn a_profile_copy_leaves_out_locks_and_the_port_file() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(from.path().join("Default/Cache")).unwrap();
        std::fs::write(from.path().join("Default/Preferences"), "{}").unwrap();
        std::fs::write(from.path().join("Default/Cache/a"), "1").unwrap();
        std::fs::write(from.path().join("Local State"), "{}").unwrap();
        std::fs::write(from.path().join("SingletonLock"), "x").unwrap();
        std::fs::write(from.path().join("DevToolsActivePort"), "9222").unwrap();
        std::fs::write(from.path().join(TEMPLATE_MARKER), "1").unwrap();
        copy_profile(from.path(), to.path()).unwrap();
        assert!(to.path().join("Default/Preferences").is_file());
        assert!(to.path().join("Default/Cache/a").is_file());
        assert!(to.path().join("Local State").is_file());
        for left_out in ["SingletonLock", "DevToolsActivePort", TEMPLATE_MARKER] {
            assert!(!to.path().join(left_out).exists(), "{left_out}");
        }
    }

    #[test]
    fn firefox_waits_longer_with_ublock_origin_and_not_at_all_to_measure_a_startup() {
        let context = |startup: Option<usize>| Context {
            executables: BTreeMap::from([
                ("firefox-ubo", PathBuf::from("/x/firefox")),
                ("firefox", PathBuf::from("/x/firefox")),
                ("brave", PathBuf::from("/x/brave")),
            ]),
            options: Options {
                browsers: vec![Config::FirefoxUbo],
                startup,
                ..Options::default()
            },
            repository: PathBuf::from("/repo"),
            cache: PathBuf::new(),
            out: PathBuf::new(),
            xpi: Some(PathBuf::from("/x/ubo.xpi")),
            templates: BTreeMap::new(),
        };
        let settle = |context: &Context, config| {
            let command = context.command(config, Path::new("/p"), None).unwrap();
            let arguments: Vec<_> = command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let at = arguments.iter().position(|a| a == "--settle-ms");
            at.map(|at| arguments[at + 1].clone())
        };
        assert_eq!(
            settle(&context(None), Config::FirefoxUbo).as_deref(),
            Some("30000")
        );
        assert_eq!(
            settle(&context(None), Config::Firefox).as_deref(),
            Some("5000")
        );
        assert_eq!(
            settle(&context(Some(3)), Config::FirefoxUbo).as_deref(),
            Some("0")
        );
        assert_eq!(settle(&context(None), Config::Brave), None);
    }

    #[test]
    fn a_zephium_launch_names_the_protection_and_session() {
        let options = Options {
            browsers: vec![Config::ZephiumOn],
            zephium: Some(PathBuf::from("/x/qa")),
            ..Options::default()
        };
        let context = Context {
            executables: BTreeMap::from([("zephium-on", PathBuf::from("/x/qa"))]),
            options,
            repository: PathBuf::from("/repo"),
            cache: PathBuf::new(),
            out: PathBuf::new(),
            xpi: None,
            templates: BTreeMap::new(),
        };
        let qa = QaSession::new(Config::ZephiumOn, "r1");
        let command = context
            .command(Config::ZephiumOn, Path::new(""), Some(&qa))
            .unwrap();
        let arguments: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(command.get_program(), "/x/qa");
        assert_eq!(arguments[0], "--blocker-bench");
        assert_eq!(arguments[1], "--blocker-bench-protection=on");
        assert_eq!(
            arguments[2],
            format!("--blocker-bench-session={}", qa.label)
        );
        assert!(context
            .command(Config::ZephiumOn, Path::new(""), None)
            .is_err());
        std::mem::forget(qa);
    }
}
