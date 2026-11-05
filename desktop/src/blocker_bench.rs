//! QA only: the protection QA application driven by a benchmark harness over
//! stdin and stdout, the way a reference browser is driven over CDP. Compiled
//! only with `adblock-qa`, which no product build can enable.
//!
//! Every protocol line on stdout is `@bench ` and one JSON object; the app's
//! own logs share the stream. Commands are JSON lines on stdin, one at a time.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::Manager;
use zephium_app::{Command, Handle};
use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::ports::engine::EngineEvent;
use zephium_engine::WebviewEngine;
use zephium_ipc::{
    BlockerPhase, BlockerPreferenceState, BlockerSourcePhase, ItemsState, OperationDisposition,
    ProfilesView, TabView,
};

const FLAG: &str = "--blocker-bench";
const PROTECTION_FLAG: &str = "--blocker-bench-protection=";
const SESSION_FLAG: &str = "--blocker-bench-session=";
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const SOURCES_TIMEOUT: Duration = Duration::from_secs(120);
const READY_POLL: Duration = Duration::from_millis(25);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Default)]
struct Bench {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    bootstrapped: Option<Duration>,
    profile: Option<String>,
    active: Option<String>,
    known: HashSet<String>,
    loading: HashSet<String>,
    // The two below only fill while a command waits on a load.
    watching: bool,
    fresh: Vec<String>,
    finished: HashMap<String, Instant>,
    blocked: HashMap<String, String>,
    // Fills only while a command waits on an operation's result.
    awaiting: bool,
    results: HashMap<String, OperationDisposition>,
}

static BENCH: OnceLock<Bench> = OnceLock::new();

impl Bench {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn watch(&self, on: bool) {
        let mut state = self.lock();
        state.watching = on;
        state.fresh.clear();
        state.finished.clear();
        state.blocked.clear();
    }

    fn note_tabs<'a>(
        &self,
        ids: impl Iterator<Item = &'a String>,
        active: Option<&Option<String>>,
    ) {
        let mut state = self.lock();
        if let Some(active) = active {
            state.active.clone_from(active);
        }
        let mut added = false;
        for id in ids {
            if state.known.insert(id.clone()) && state.watching {
                state.fresh.push(id.clone());
                added = true;
            }
        }
        drop(state);
        if added {
            self.changed.notify_all();
        }
    }

    fn wait_bootstrap(&self, deadline: Instant) -> Option<Duration> {
        let mut state = self.lock();
        loop {
            if state.bootstrapped.is_some() {
                return state.bootstrapped;
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    /// The first tab that appeared, once one has; `None` at `deadline`.
    fn wait_fresh(&self, deadline: Instant) -> Option<String> {
        let mut state = self.lock();
        loop {
            if let Some(tab) = state.fresh.first() {
                return Some(tab.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    /// Whether the tab stopped loading by `deadline`.
    fn wait_idle(&self, tab: &str, deadline: Instant) -> bool {
        let mut state = self.lock();
        loop {
            if !state.loading.contains(tab) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    fn loading_changed(&self, tab: String, loading: bool, at: Instant) {
        let mut state = self.lock();
        if loading {
            state.loading.insert(tab);
        } else {
            state.loading.remove(&tab);
            if state.watching {
                state.finished.insert(tab, at);
            }
        }
        drop(state);
        self.changed.notify_all();
    }

    /// A refused page never starts loading; it ends where chrome shows why.
    fn refused(&self, tab: String, url: &str, at: Instant) {
        let mut state = self.lock();
        state.loading.remove(&tab);
        if state.watching {
            let url = tauri::Url::parse(url).map_or_else(|_| url.to_owned(), String::from);
            state.blocked.insert(tab.clone(), url);
            state.finished.insert(tab, at);
        }
        drop(state);
        self.changed.notify_all();
    }

    fn note_result(&self, disposition: &OperationDisposition) {
        let mut state = self.lock();
        if state.awaiting {
            state
                .results
                .insert(disposition.operation_id.clone(), disposition.clone());
            drop(state);
            self.changed.notify_all();
        }
    }

    fn await_results(&self, on: bool) {
        let mut state = self.lock();
        state.awaiting = on;
        state.results.clear();
    }

    fn wait_result(&self, operation: &str, deadline: Instant) -> Option<OperationDisposition> {
        let mut state = self.lock();
        loop {
            if let Some(disposition) = state.results.remove(operation) {
                return Some(disposition);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    /// When the tab's load finished, once it has; `None` at `deadline`.
    fn wait_finished(&self, tab: &str, deadline: Instant) -> Option<Instant> {
        let mut state = self.lock();
        loop {
            if let Some(at) = state.finished.get(tab) {
                return Some(*at);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }
}

struct Options {
    protection: bool,
}

fn options() -> Result<Option<Options>, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|arg| arg == FLAG) {
        return Ok(None);
    }
    let values: Vec<&str> = args
        .iter()
        .filter_map(|arg| arg.strip_prefix(PROTECTION_FLAG))
        .collect();
    let protection = match values.as_slice() {
        [] | ["on"] => true,
        ["off"] => false,
        _ => return Err(format!("{PROTECTION_FLAG} takes one value, on or off")),
    };
    Ok(Some(Options { protection }))
}

pub(crate) fn enabled() -> bool {
    std::env::args().skip(1).any(|arg| arg == FLAG)
}

/// The disposable data directory label, if one was asked for.
pub(crate) fn session() -> std::io::Result<Option<String>> {
    crate::webext_qa::session_label(SESSION_FLAG)
}

/// A failed setup ends in a startup dialog nobody drives, so bad flags stop
/// the process before any window exists.
pub(crate) fn refuse_bad_arguments() {
    let problem = options()
        .err()
        .or_else(|| session().err().map(|error| error.to_string()));
    if let Some(message) = problem {
        emit(&json!({ "event": "error", "message": message }));
        std::process::exit(2);
    }
}

fn fail(message: &str) -> std::io::Error {
    emit(&json!({ "event": "error", "message": message }));
    std::io::Error::other(message.to_owned())
}

fn emit(value: &Value) {
    let line = format!("@bench {value}\n");
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes()).and_then(|()| out.flush());
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

fn uptime() -> Duration {
    crate::diagnostics::uptime().unwrap_or_default()
}

pub(crate) fn engine_event(event: &EngineEvent) {
    let Some(bench) = BENCH.get() else {
        return;
    };
    match event {
        EngineEvent::LoadingChanged { id, loading } => {
            bench.loading_changed(id.to_string(), *loading, Instant::now());
        }
        EngineEvent::NavigationBlocked { id, url, .. } => {
            bench.refused(id.to_string(), url, Instant::now());
        }
        _ => {}
    }
}

/// The actor's result for an operation the driver is waiting on. Chrome
/// acknowledges every result, so the driver keeps its own copy.
pub(crate) fn operation_processed(disposition: &OperationDisposition) {
    if let Some(bench) = BENCH.get() {
        bench.note_result(disposition);
    }
}

pub(crate) fn items(state: &ItemsState) {
    if let Some(bench) = BENCH.get() {
        bench.note_tabs(state.tabs.iter().map(|tab| &tab.id), Some(&state.active));
    }
}

pub(crate) fn tab(tab: &TabView) {
    if let Some(bench) = BENCH.get() {
        bench.note_tabs(std::iter::once(&tab.id), None);
    }
}

/// When the main window became visible: what a person sees as "started". It
/// can happen before the driver starts, so it is kept apart from its state.
static WINDOW_SHOWN: OnceLock<Duration> = OnceLock::new();

pub(crate) fn window_shown() {
    let _ = WINDOW_SHOWN.set(uptime());
}

/// Only sent once the shell has restored or created its session.
pub(crate) fn profiles(view: &ProfilesView) {
    let Some(bench) = BENCH.get() else {
        return;
    };
    let mut state = bench.lock();
    state.profile.clone_from(&view.current);
    if state.bootstrapped.is_none() {
        state.bootstrapped = Some(uptime());
    }
    drop(state);
    bench.changed.notify_all();
}

pub(crate) fn start(app: &tauri::AppHandle, engine: &Arc<WebviewEngine>) -> std::io::Result<()> {
    let options = match options() {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(()),
        Err(message) => return Err(fail(&message)),
    };
    if BENCH.set(Bench::default()).is_err() {
        return Err(fail("the benchmark driver is already running"));
    }
    let (lines, commands) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("blocker-bench-stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                if !line.is_ok_and(|line| lines.send(line).is_ok()) {
                    break;
                }
            }
        })
        .map_err(|error| fail(&format!("could not start the stdin reader: {error}")))?;
    let (app, engine) = (app.clone(), Arc::downgrade(engine));
    std::thread::Builder::new()
        .name("blocker-bench".into())
        .spawn(move || drive(&app, &engine, &options, &commands))
        .map_err(|error| fail(&format!("could not start the driver: {error}")))?;
    Ok(())
}

fn drive(
    app: &tauri::AppHandle,
    engine: &Weak<WebviewEngine>,
    options: &Options,
    commands: &Receiver<String>,
) {
    let Some(bench) = BENCH.get() else {
        return;
    };
    match await_ready(app, bench, options.protection) {
        Ok((bootstrap, policy)) => {
            let startup = uptime();
            let Ok((sources, phase)) = await_sources(app) else {
                emit(&json!({
                    "event": "error",
                    "message": "list sources were still updating at the deadline",
                }));
                app.exit(0);
                return;
            };
            emit(&json!({
                "event": "ready",
                "pid": std::process::id(),
                "startupMs": millis(startup),
                "windowMs": WINDOW_SHOWN.get().copied().map(millis),
                "bootstrapMs": millis(bootstrap),
                "policyReadyMs": millis(policy),
                "sourcesSettledMs": millis(sources),
                "sourcePhase": format!("{phase:?}"),
            }));
        }
        Err(message) => {
            emit(&json!({ "event": "error", "message": message }));
            app.exit(0);
            return;
        }
    }
    // A closed stdin means the harness is gone.
    while let Ok(line) = commands.recv() {
        if line.trim().is_empty() {
            continue;
        }
        let (reply, quit) = handle(app, engine, bench, &line);
        emit(&reply);
        if quit {
            break;
        }
    }
    app.exit(0);
}

fn await_ready(
    app: &tauri::AppHandle,
    bench: &Bench,
    protection: bool,
) -> Result<(Duration, Duration), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let bootstrap = bench
        .wait_bootstrap(deadline)
        .ok_or("the browser did not finish starting")?;
    // Raising the window earlier would show the blank bootstrap page.
    while WINDOW_SHOWN.get().is_none() {
        if Instant::now() >= deadline {
            return Err("the window was never shown".into());
        }
        std::thread::sleep(READY_POLL);
    }
    raise(app);
    loop {
        let shell = app
            .try_state::<Handle>()
            .ok_or("the shell is unavailable")?;
        let status = shell
            .focused_content_policy_status()
            .recv_timeout(Duration::from_secs(2));
        if status.phase == BlockerPhase::Ready
            && status.desired_enabled == Some(protection)
            && status.applied_enabled == Some(protection)
        {
            return Ok((bootstrap, uptime()));
        }
        // A fresh profile's preference is not settled yet and refuses a change.
        // Answers to this request queue behind the change, so it is not sent twice.
        if status.preference == BlockerPreferenceState::Authoritative
            && status
                .desired_enabled
                .is_some_and(|enabled| enabled != protection)
        {
            let command = Command::SetFocusedContentBlockerEnabled(protection);
            if !crate::dispatch_operation(app, &shell, command).accepted {
                return Err("the protection change was not admitted".into());
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "protection did not become ready (phase {:?}, preference {:?}, desired {:?}, applied {:?})",
                status.phase, status.preference, status.desired_enabled, status.applied_enabled
            ));
        }
        std::thread::sleep(READY_POLL);
    }
}

/// A fresh profile downloads and compiles the lists in the background, and that
/// work would land in whatever the harness measures next. Ready waits for it,
/// and the launch fails when it is still running at the deadline.
fn await_sources(app: &tauri::AppHandle) -> Result<(Duration, BlockerSourcePhase), ()> {
    let deadline = Instant::now() + SOURCES_TIMEOUT;
    loop {
        let Some(shell) = app.try_state::<Handle>() else {
            return Err(());
        };
        let status = shell
            .focused_content_policy_status()
            .recv_timeout(Duration::from_secs(2));
        let phase = status.source_phase;
        // An unanswered query reads as unavailable, which says nothing yet; a
        // settled source can start a compile in the same turn, and a fresh one
        // can still hold a candidate to activate or material to repair.
        if !matches!(
            status.phase,
            BlockerPhase::Unavailable | BlockerPhase::Compiling | BlockerPhase::Installing
        ) && !matches!(
            phase,
            BlockerSourcePhase::Stale | BlockerSourcePhase::Refreshing
        ) && !status.source_activation_pending
            && !status.source_material_repair_pending
        {
            return Ok((uptime(), phase));
        }
        if Instant::now() >= deadline {
            return Err(());
        }
        std::thread::sleep(READY_POLL);
    }
}

fn handle(
    app: &tauri::AppHandle,
    engine: &Weak<WebviewEngine>,
    bench: &Bench,
    line: &str,
) -> (Value, bool) {
    let Ok(command) = serde_json::from_str::<Value>(line) else {
        return (
            json!({ "id": null, "ok": false, "error": "invalid JSON" }),
            false,
        );
    };
    let id = command.get("id").cloned().unwrap_or(Value::Null);
    let op = command
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = match op {
        "open" => open(app, bench, &command),
        "navigate" => navigate(app, bench, &command),
        "eval" => eval(engine, &command),
        "preload" => preload(engine, &command),
        "blocked" => blocked(app, bench, &command),
        "close" => close(app, &command),
        "restart" => restart(app),
        "command" => run_command(app, &command),
        "setting" => setting(app, bench, &command),
        "capture" => capture(app, &command),
        "quit" => Ok(json!({})),
        _ => Err(format!("unknown op {op:?}")),
    };
    let reply = match result {
        Ok(Value::Object(fields)) => reply_with(&id, true, fields),
        Ok(_) => reply_with(&id, true, serde_json::Map::new()),
        Err(error) => {
            let mut fields = serde_json::Map::new();
            fields.insert("error".into(), error.into());
            reply_with(&id, false, fields)
        }
    };
    (Value::Object(reply), op == "quit")
}

fn reply_with(
    id: &Value,
    ok: bool,
    fields: serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut reply = serde_json::Map::new();
    reply.insert("id".into(), id.clone());
    reply.insert("ok".into(), ok.into());
    reply.extend(fields);
    reply
}

fn timeout(command: &Value) -> Duration {
    command
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .filter(|ms| *ms > 0)
        .map_or(DEFAULT_TIMEOUT, Duration::from_millis)
        .min(MAX_TIMEOUT)
}

fn text<'a>(command: &'a Value, key: &str) -> Result<&'a str, String> {
    command
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is required"))
}

fn tab_id(command: &Value) -> Result<ItemId, String> {
    ItemId::parse(text(command, "tab")?).ok_or_else(|| "tab is not a tab id".to_owned())
}

fn shell(app: &tauri::AppHandle) -> Result<tauri::State<'_, Handle>, String> {
    app.try_state::<Handle>()
        .ok_or_else(|| "the shell is unavailable".to_owned())
}

fn open(app: &tauri::AppHandle, bench: &Bench, command: &Value) -> Result<Value, String> {
    let (url, timeout) = (text(command, "url")?, timeout(command));
    let shell = shell(app)?;
    raise(app);
    if command.get("private").and_then(Value::as_bool) == Some(true) {
        return open_private(app, &shell, bench, url, timeout);
    }
    bench.watch(true);
    let started = Instant::now();
    let command = Command::OpenUrl {
        input: url.to_owned(),
        new_tab: true,
    };
    let dispatched = shell.dispatch(command);
    let tab = dispatched
        .then(|| bench.wait_fresh(started + timeout))
        .flatten();
    if let Some(tab) = &tab {
        select(&shell, bench, tab);
    }
    load(app, bench, dispatched, tab, started, timeout)
}

/// Opens a private window and loads `url` in its first tab.
fn open_private(
    app: &tauri::AppHandle,
    shell: &Handle,
    bench: &Bench,
    url: &str,
    timeout: Duration,
) -> Result<Value, String> {
    let deadline = Instant::now() + timeout;
    bench.watch(true);
    let tab = shell
        .dispatch(Command::Run("window.newPrivate".into()))
        .then(|| bench.wait_fresh(deadline))
        .flatten();
    let Some((tab, id)) = tab.and_then(|tab| ItemId::parse(&tab).map(|id| (tab, id))) else {
        bench.watch(false);
        return Err("no private window opened".into());
    };
    select(shell, bench, &tab);
    if !bench.wait_idle(&tab, deadline) {
        bench.watch(false);
        return Err(format!(
            "the tab was still loading after {} ms",
            timeout.as_millis()
        ));
    }
    bench.watch(true);
    let started = Instant::now();
    let dispatched = shell.dispatch(Command::Navigate {
        id,
        input: url.to_owned(),
    });
    load(app, bench, dispatched, Some(tab), started, timeout)
}

fn navigate(app: &tauri::AppHandle, bench: &Bench, command: &Value) -> Result<Value, String> {
    let (id, url, timeout) = (tab_id(command)?, text(command, "url")?, timeout(command));
    let shell = shell(app)?;
    raise(app);
    let tab = id.to_string();
    select(&shell, bench, &tab);
    // A load still in flight would end after the watch starts and pass for
    // this one, and its end event cannot be told apart. The wait is not part
    // of this navigation's time.
    if !bench.wait_idle(&tab, Instant::now() + timeout) {
        return Err(format!(
            "the tab was still loading after {} ms",
            timeout.as_millis()
        ));
    }
    bench.watch(true);
    let started = Instant::now();
    let command = Command::Navigate {
        id,
        input: url.to_owned(),
    };
    let dispatched = shell.dispatch(command);
    load(app, bench, dispatched, Some(tab), started, timeout)
}

/// A page that is not the window's active tab is hidden and throttled, which
/// changes what it loads. Opening normally selects the tab; this makes sure.
fn select(shell: &Handle, bench: &Bench, tab: &str) {
    let selected = bench.lock().active.as_deref() == Some(tab);
    if let (false, Some(id)) = (selected, ItemId::parse(tab)) {
        shell.dispatch(Command::Activate(id));
    }
}

/// The window the pages are in must be shown, upright and in front of other
/// windows, or the system treats its pages as occluded.
fn raise(app: &tauri::AppHandle) {
    let (done, raised) = std::sync::mpsc::channel();
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(crate::MAIN_LABEL) {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
            #[cfg(target_os = "macos")]
            front(&window);
        }
        let _ = done.send(());
    });
    if queued.is_ok() {
        let _ = raised.recv_timeout(Duration::from_secs(2));
    }
}

/// A process started from a terminal is not the active application, and
/// `set_focus` alone leaves it behind the terminal.
#[cfg(target_os = "macos")]
fn front(window: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSApplication, NSWindow};
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    if let Ok(pointer) = window.ns_window() {
        // SAFETY: Tao's live NSWindow, borrowed on the main thread for this call.
        let ns_window = unsafe { &*(pointer as *const NSWindow) };
        ns_window.orderFrontRegardless();
        ns_window.makeKeyAndOrderFront(None);
    }
}

/// Raises the restart a newer runtime waits on, so its notice can be seen
/// and captured without a real runtime update.
fn restart(app: &tauri::AppHandle) -> Result<Value, String> {
    if shell(app)?.dispatch(Command::Engine(EngineEvent::RuntimeRestartRequired)) {
        Ok(json!({}))
    } else {
        Err("the command queue refused the request".into())
    }
}

/// Runs a named chrome command, as a menu or shortcut does. `id` belongs to
/// the protocol, so the command is named by `name`.
fn run_command(app: &tauri::AppHandle, command: &Value) -> Result<Value, String> {
    let name = text(command, "name")?;
    admitted(&super::execute_command(app, name))
}

fn admitted(admission: &zephium_ipc::OperationAdmission) -> Result<Value, String> {
    if admission.accepted {
        Ok(json!({ "operation": admission.operation_id }))
    } else {
        Err("the command was not admitted".into())
    }
}

/// Waits for a dispatched navigation to finish loading. On timeout it still
/// names the tab, so the harness can measure what loaded and close it.
fn load(
    app: &tauri::AppHandle,
    bench: &Bench,
    dispatched: bool,
    tab: Option<String>,
    started: Instant,
    timeout: Duration,
) -> Result<Value, String> {
    let deadline = started + timeout;
    let finished = tab
        .as_deref()
        .filter(|_| dispatched)
        .and_then(|tab| bench.wait_finished(tab, deadline));
    let refused = tab
        .as_deref()
        .and_then(|tab| bench.lock().blocked.get(tab).cloned());
    bench.watch(false);
    let finished = match (finished, &tab, refused) {
        (Some(_), Some(tab), Some(url)) => wait_rendered(app, tab, &url, deadline),
        (finished, ..) => finished,
    };
    if !dispatched {
        return Err("the command queue refused the request".into());
    }
    let Some(tab) = tab else {
        return Err(format!("no tab opened within {} ms", timeout.as_millis()));
    };
    Ok(match finished {
        Some(at) => json!({ "tab": tab, "loadMs": millis(at.saturating_duration_since(started)) }),
        None => json!({ "tab": tab, "loadMs": millis(timeout), "timedOut": true }),
    })
}

/// The blocked page for `url` as chrome draws it for `tab`, once it is laid
/// out; `None` at `deadline`.
fn wait_rendered(
    app: &tauri::AppHandle,
    tab: &str,
    url: &str,
    deadline: Instant,
) -> Option<Instant> {
    let (tab, url) = (
        serde_json::to_string(tab).ok()?,
        serde_json::to_string(url).ok()?,
    );
    let script = format!(
        "(() => {{ const page = document.querySelector('[data-zephium-blocked-tab]'); \
         if (!page || page.dataset.zephiumBlockedTab !== {tab} || page.dataset.zephiumBlockedUrl !== {url}) return false; \
         return page.getBoundingClientRect().height > 0; }})()"
    );
    loop {
        if shown(app, &script) {
            return Some(Instant::now());
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(READY_POLL);
    }
}

fn shown(app: &tauri::AppHandle, script: &str) -> bool {
    let Some(window) = app.get_webview_window(crate::MAIN_LABEL) else {
        return false;
    };
    let (done, answer) = std::sync::mpsc::channel();
    let asked = window.eval_with_callback(script, move |result| {
        let _ = done.send(result == "true");
    });
    asked.is_ok() && answer.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
}

/// The body of an async function, run in the page's main world. The envelope
/// carries a page-side failure back as text, which the native call cannot.
fn script(body: &str) -> String {
    format!(
        "(async () => {{\n{body}\n}})().then(\
         v => {{ try {{ return JSON.stringify({{ ok: true, value: v === undefined ? null : v }}); }} \
         catch (e) {{ return JSON.stringify({{ ok: false, error: 'result is not JSON: ' + e }}); }} }}, \
         e => JSON.stringify({{ ok: false, error: e && e.message ? e.name + ': ' + e.message : String(e) }}))"
    )
}

fn eval(engine: &Weak<WebviewEngine>, command: &Value) -> Result<Value, String> {
    let (id, body, timeout) = (tab_id(command)?, text(command, "js")?, timeout(command));
    let engine = engine.upgrade().ok_or("the engine is unavailable")?;
    let answer = engine.blocker_bench_evaluate(id, script(body));
    drop(engine);
    let result = match answer.recv_timeout(timeout) {
        Ok(Some(result)) => result,
        Ok(None) => {
            return Err(
                "the page gave no result: no such tab, a syntax error, a navigation, or a result over 256 KB"
                    .into(),
            )
        }
        Err(RecvTimeoutError::Timeout) => {
            return Err(format!("eval timed out after {} ms", timeout.as_millis()))
        }
        Err(RecvTimeoutError::Disconnected) => {
            return Err("the engine could not take the evaluation".into())
        }
    };
    let envelope: Value = serde_json::from_str(&result).map_err(|_| "unreadable result")?;
    if envelope.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(json!({ "value": envelope.get("value").cloned().unwrap_or(Value::Null) }))
    } else {
        Err(envelope
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the page failed")
            .to_owned())
    }
}

/// Runs at document start in the main frame of every page loaded afterwards.
fn preload(engine: &Weak<WebviewEngine>, command: &Value) -> Result<Value, String> {
    let (source, timeout) = (text(command, "js")?, timeout(command));
    let engine = engine.upgrade().ok_or("the engine is unavailable")?;
    let answer = engine.blocker_bench_preload(source.to_owned());
    drop(engine);
    match answer.recv_timeout(timeout) {
        Ok(true) => Ok(json!({})),
        Ok(false) => Err("a page did not take the script".into()),
        Err(_) => Err("the engine could not take the script".into()),
    }
}

fn blocked(app: &tauri::AppHandle, bench: &Bench, command: &Value) -> Result<Value, String> {
    let shell = shell(app)?;
    let profile = bench.lock().profile.as_deref().and_then(ProfileId::parse);
    let today = profile
        .and_then(|profile| {
            shell
                .blocker_statistics(profile)
                .recv_timeout(timeout(command))
                .ok()
        })
        .flatten()
        .map(|statistics| statistics.today);
    Ok(json!({ "value": today }))
}

/// An app setting, such as `appearance`, as the settings page would set it:
/// admitted as an operation, then answered with what the shell made of it.
fn setting(app: &tauri::AppHandle, bench: &Bench, command: &Value) -> Result<Value, String> {
    let (key, value) = (text(command, "key")?, text(command, "value")?);
    let (shell, deadline) = (shell(app)?, Instant::now() + timeout(command));
    // The result can arrive before the admission returns its id.
    bench.await_results(true);
    let admission = crate::dispatch_operation(
        app,
        &shell,
        Command::SetAppSetting {
            key: key.to_owned(),
            value: value.to_owned(),
        },
    );
    let result = admission
        .operation_id
        .filter(|_| admission.accepted)
        .map(|operation| bench.wait_result(&operation, deadline));
    bench.await_results(false);
    match result.ok_or("the setting was not admitted")? {
        Some(disposition) => Ok(json!({
            "admitted": true,
            "outcome": disposition.outcome,
            "reason": disposition.reason,
        })),
        None => Ok(json!({ "admitted": true, "timedOut": true })),
    }
}

/// The main window as a PNG at `path`, for page screenshots.
#[cfg(target_os = "macos")]
fn capture(app: &tauri::AppHandle, command: &Value) -> Result<Value, String> {
    let path = std::path::PathBuf::from(text(command, "path")?);
    let (done, captured) = std::sync::mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let png = handle
            .get_webview_window(crate::MAIN_LABEL)
            .ok_or_else(|| "no main window".to_owned())
            .and_then(|window| window.ns_window().map_err(|error| error.to_string()))
            // SAFETY: Tao's live NSWindow, on the main thread for this call.
            .and_then(|pointer| unsafe { zephium_engine::capture_window_png(pointer) });
        let _ = done.send(png);
    })
    .map_err(|error| error.to_string())?;
    let png = captured
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the capture did not finish".to_owned())??;
    std::fs::write(&path, png).map_err(|error| error.to_string())?;
    Ok(json!({ "path": path }))
}

#[cfg(not(target_os = "macos"))]
fn capture(_: &tauri::AppHandle, _: &Value) -> Result<Value, String> {
    Err("capture is macOS only".into())
}

fn close(app: &tauri::AppHandle, command: &Value) -> Result<Value, String> {
    let id = tab_id(command)?;
    if shell(app)?.dispatch(Command::Close(id)) {
        Ok(json!({}))
    } else {
        Err("the command queue refused the request".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_ipc::{OperationOutcome, OperationReason};

    #[test]
    fn a_command_is_named_apart_from_the_protocol_id_and_a_refusal_fails() {
        let request = json!({ "id": 7, "op": "command", "name": "browser.relaunch" });
        assert_eq!(text(&request, "name"), Ok("browser.relaunch"));
        let refused = zephium_ipc::OperationAdmission {
            operation_id: None,
            accepted: false,
        };
        assert!(admitted(&refused).is_err());
        let taken = zephium_ipc::OperationAdmission {
            operation_id: Some("01".into()),
            accepted: true,
        };
        assert_eq!(admitted(&taken), Ok(json!({ "operation": "01" })));
    }

    #[test]
    fn a_load_ends_only_for_a_watch_and_idleness_follows_the_last_event() {
        let bench = Bench::default();
        let soon = || Instant::now() + Duration::from_millis(20);
        bench.loading_changed("t".into(), true, Instant::now());
        assert!(!bench.wait_idle("t", soon()));
        bench.loading_changed("t".into(), false, Instant::now());
        assert!(bench.wait_idle("t", soon()));
        assert!(bench.wait_finished("t", soon()).is_none());
        bench.watch(true);
        bench.loading_changed("t".into(), true, Instant::now());
        assert!(bench.wait_finished("t", soon()).is_none());
        bench.loading_changed("t".into(), false, Instant::now());
        assert!(bench.wait_finished("t", soon()).is_some());
    }

    #[test]
    fn a_body_cannot_swallow_the_closing_of_its_function() {
        let wrapped = script("return 1; // trailing comment");
        assert!(wrapped.starts_with("(async () => {\nreturn 1; // trailing comment\n})()"));
    }

    #[test]
    fn timeouts_are_bounded_and_default() {
        assert_eq!(timeout(&json!({})), DEFAULT_TIMEOUT);
        assert_eq!(timeout(&json!({ "timeoutMs": 0 })), DEFAULT_TIMEOUT);
        assert_eq!(
            timeout(&json!({ "timeoutMs": 250 })),
            Duration::from_millis(250)
        );
        assert_eq!(timeout(&json!({ "timeoutMs": u64::MAX })), MAX_TIMEOUT);
    }

    #[test]
    fn replies_carry_the_command_id_and_the_outcome() {
        let mut fields = serde_json::Map::new();
        fields.insert("error".into(), "no".into());
        let reply = reply_with(&json!(7), false, fields);
        assert_eq!(
            Value::Object(reply),
            json!({ "id": 7, "ok": false, "error": "no" })
        );
    }

    #[test]
    fn a_refusal_ends_the_load_and_names_the_page_chrome_must_show() {
        let bench = Bench::default();
        bench.loading_changed("tab".into(), true, Instant::now());
        bench.watch(true);
        bench.refused("tab".into(), "https://example.com", Instant::now());
        assert!(bench
            .wait_finished("tab", Instant::now() + Duration::from_secs(1))
            .is_some());
        assert!(bench.wait_idle("tab", Instant::now()));
        assert_eq!(
            bench.lock().blocked.get("tab").map(String::as_str),
            Some("https://example.com/")
        );
        bench.watch(false);
        assert!(bench.lock().blocked.is_empty());
    }

    #[test]
    fn a_result_is_kept_only_while_a_command_waits_for_one() {
        let bench = Bench::default();
        let disposition = |id: &str| OperationDisposition {
            operation_id: id.into(),
            outcome: OperationOutcome::Rejected,
            reason: OperationReason::InvalidInput,
        };
        bench.note_result(&disposition("early"));
        bench.await_results(true);
        assert_eq!(bench.wait_result("early", Instant::now()), None);
        bench.note_result(&disposition("op"));
        assert_eq!(
            bench.wait_result("op", Instant::now()),
            Some(disposition("op"))
        );
        bench.await_results(false);
        assert!(bench.lock().results.is_empty());
    }
}
