mod detection;
mod environment;
mod report;
mod state;

use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use zephium_core::linux_graphics::{
    select_renderer, CrashRecovery, GraphicsVersion, PolicyContext, RendererOverrides,
    RendererPath, RendererPreference, SelectionSource,
};
use zephium_ipc::linux_graphics::{GpuReport, ProbeData, ProbeOutcome};

const PROBE_ARGUMENT: &str = "--zephium-internal-gtk-gl-probe";
const GL_UNAVAILABLE: u8 = 20;
const PROBE_INCONCLUSIVE: u8 = 21;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PROBE_BYTES: u64 = 64 * 1024;
static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();
static APPLIED: OnceLock<&'static str> = OnceLock::new();

struct Runtime {
    report: GpuReport,
    store: Option<state::StateStore>,
    initial_preference: RendererPreference,
}

/// Called by main before GTK, WebKit or the async runtime creates threads.
pub fn prepare() -> Option<ExitCode> {
    if std::env::args_os().skip(1).eq([PROBE_ARGUMENT]) {
        let data = detection::probe();
        let status = match data.outcome {
            ProbeOutcome::Supported => ExitCode::SUCCESS,
            ProbeOutcome::Unavailable => ExitCode::from(GL_UNAVAILABLE),
            ProbeOutcome::Inconclusive => ExitCode::from(PROBE_INCONCLUSIVE),
        };
        match serde_json::to_string(&data) {
            Ok(json) => println!("{json}"),
            Err(_) => return Some(ExitCode::from(PROBE_INCONCLUSIVE)),
        }
        return Some(status);
    }
    let report_only = std::env::args_os().skip(1).any(|arg| arg == "--gpu-report");
    let executable = std::env::current_exe().ok();
    let mut probe = executable
        .as_deref()
        .and_then(|path| run_probe(path, false));
    if std::env::var_os("GDK_GL").is_none()
        && probe
            .as_ref()
            .is_some_and(|data| data.outcome == ProbeOutcome::Unavailable)
    {
        if let Some(path) = executable.as_deref() {
            let fallback = run_probe(path, true);
            if fallback
                .as_ref()
                .is_some_and(|data| data.outcome == ProbeOutcome::Supported)
            {
                eprintln!("graphics: using verified GLES context after default GTK GL initialization failed");
                let error = Command::new(path)
                    .args(std::env::args_os().skip(1))
                    .env("GDK_GL", "gles")
                    .exec();
                eprintln!("graphics: could not restart with the verified GLES context: {error}");
                return Some(ExitCode::FAILURE);
            }
            // Retain the actual default backend and monitor facts even without GL.
            if probe.is_none() {
                probe = fallback;
            }
        }
    }
    let detection = detection::collect(probe);
    let environment = environment::capture();
    let mut warnings = Vec::new();
    let mut store = None;
    let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let snapshot = match state::resolve_config_directory(&context.config().identifier) {
        Ok(config) => {
            if report_only {
                state::StateStore::read_existing(&config)
            } else {
                match state::StateStore::open_or_create(&config) {
                    Ok(opened) => {
                        let snapshot = opened.snapshot();
                        store = Some(opened);
                        Ok(snapshot)
                    }
                    Err(error) => Err(error),
                }
            }
        }
        Err(error) => Err(error),
    };
    let (preference, recovery) = match snapshot {
        Ok(snapshot) => (snapshot.preference, snapshot.recovery),
        Err(error) => {
            warnings.push(error.to_string());
            (RendererPreference::Automatic, CrashRecovery::default())
        }
    };
    let disable = std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER");
    let force_shm = std::env::var_os("WEBKIT_DMABUF_RENDERER_FORCE_SHM");
    let disable = disable.as_ref().map(|value| value.to_string_lossy());
    let force_shm = force_shm.as_ref().map(|value| value.to_string_lossy());
    let gl = detection.gl.as_ref();
    let selection = select_renderer(
        PolicyContext {
            driver_class: detection.driver_class,
            session: detection.session,
            webkit_version: version(&detection.webkit_version),
            mesa_version: gl
                .and_then(|gl| gl.mesa_version.as_deref())
                .and_then(version),
            nvidia_version: gl
                .and_then(|gl| gl.nvidia_version.as_deref())
                .and_then(version),
        },
        preference,
        RendererOverrides {
            disable_dmabuf: disable.as_deref(),
            force_shm: force_shm.as_deref(),
        },
        recovery,
    );
    if !report_only {
        if !matches!(selection.source, SelectionSource::Environment) {
            apply_path(selection.path);
        }
        if preference == RendererPreference::Automatic
            && !matches!(selection.source, SelectionSource::Environment)
        {
            if let Some(store) = store.as_mut() {
                if let Err(error) =
                    store.record_automatic_attempt(selection.path, selection.crash_recovery)
                {
                    warnings.push(error.to_string());
                }
            }
        }
    }
    let report = GpuReport {
        detection,
        preference,
        running_preference: preference,
        selection,
        environment,
        restart_required: false,
        startup_status: if report_only {
            "report only; no launch recorded"
        } else {
            "awaiting window shown"
        }
        .into(),
        state_warnings: warnings,
        text: String::new(),
    };
    if report_only {
        print!("{}", report::render(&report));
        return Some(ExitCode::SUCCESS);
    }
    let _ = RUNTIME.set(Mutex::new(Runtime {
        report,
        store,
        initial_preference: preference,
    }));
    None
}

fn version(value: &str) -> Option<GraphicsVersion> {
    GraphicsVersion::parse(value)
}

fn apply_path(path: RendererPath) {
    // Only called in the single-threaded entry point, and only without user overrides.
    let name = match path {
        RendererPath::Default => return,
        RendererPath::ForceShm => "WEBKIT_DMABUF_RENDERER_FORCE_SHM",
        RendererPath::DisableDmabuf => "WEBKIT_DISABLE_DMABUF_RENDERER",
    };
    std::env::set_var(name, "1");
    let _ = APPLIED.set(name);
}

/// Starts the next process without the renderer variable this one set for
/// itself, so it selects again instead of reading it as the person's override.
pub(crate) fn restart(env: &tauri::Env) -> ! {
    if let Ok(path) = tauri::process::current_binary(env) {
        let mut command = Command::new(path);
        command.args(env.args_os.iter().skip(1));
        if let Some(name) = APPLIED.get() {
            command.env_remove(name);
        }
        if let Err(error) = command.spawn() {
            eprintln!("graphics: could not restart Zephium: {error}");
        }
    }
    std::process::exit(0);
}

pub(crate) fn window_shown() {
    drop(tauri::async_runtime::spawn_blocking(settle_window_shown));
}

fn settle_window_shown() {
    let Some(runtime) = RUNTIME.get() else {
        return;
    };
    let Ok(mut runtime) = runtime.lock() else {
        return;
    };
    if runtime.report.startup_status == "window shown; recovery reset" {
        return;
    }
    if let Some(store) = runtime.store.as_mut() {
        if let Err(error) = store.window_shown() {
            runtime.report.state_warnings.push(error.to_string());
            runtime.report.startup_status =
                "window shown; recovery reset could not be saved".into();
            return;
        }
    } else {
        runtime.report.startup_status = "window shown; recovery storage unavailable".into();
        return;
    }
    runtime.report.startup_status = "window shown; recovery reset".into();
}

pub(crate) fn report() -> Result<GpuReport, String> {
    let runtime = RUNTIME
        .get()
        .ok_or("Graphics startup report is unavailable")?
        .lock()
        .map_err(|_| "Graphics startup state is unavailable")?;
    let mut report = runtime.report.clone();
    report.text = report::render(&report);
    Ok(report)
}

pub(crate) fn set_preference(preference: RendererPreference) -> Result<GpuReport, String> {
    {
        let mut runtime = RUNTIME
            .get()
            .ok_or("Graphics startup report is unavailable")?
            .lock()
            .map_err(|_| "Graphics startup state is unavailable")?;
        let store = runtime
            .store
            .as_mut()
            .ok_or("Renderer preferences are unavailable for this launch")?;
        store
            .set_preference(preference)
            .map_err(|error| error.to_string())?;
        runtime.report.preference = preference;
        runtime.report.restart_required = preference != runtime.initial_preference;
    }
    report()
}

fn run_probe(executable: &std::path::Path, gles: bool) -> Option<ProbeData> {
    let mut output = tempfile::tempfile().ok()?;
    let mut command = Command::new(executable);
    command
        .arg(PROBE_ARGUMENT)
        .stdin(Stdio::null())
        .stdout(output.try_clone().ok()?)
        .stderr(Stdio::null());
    if gles {
        command.env("GDK_GL", "gles");
    }
    let status = command
        .spawn()
        .and_then(|mut child| wait_bounded(&mut child, PROBE_TIMEOUT))
        .ok()?;
    if output.metadata().ok()?.len() > MAX_PROBE_BYTES {
        return None;
    }
    output.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    output.take(MAX_PROBE_BYTES).read_to_end(&mut bytes).ok()?;
    let data: ProbeData = serde_json::from_slice(&bytes).ok()?;
    (data.outcome == outcome(status)).then_some(data)
}

fn outcome(status: ExitStatus) -> ProbeOutcome {
    match status.code() {
        Some(0) => ProbeOutcome::Supported,
        Some(code) if code == i32::from(GL_UNAVAILABLE) => ProbeOutcome::Unavailable,
        _ => ProbeOutcome::Inconclusive,
    }
}

fn wait_bounded(child: &mut Child, timeout: Duration) -> io::Result<ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(
                    Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
                );
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return match result {
                    Err(error) => Err(error),
                    _ => Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "GTK context probe timed out",
                    )),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn only_explicit_context_failure_allows_a_fallback() {
        assert_eq!(outcome(ExitStatus::from_raw(0)), ProbeOutcome::Supported);
        assert_eq!(
            outcome(ExitStatus::from_raw(i32::from(GL_UNAVAILABLE) << 8)),
            ProbeOutcome::Unavailable
        );
        for raw in [1 << 8, i32::from(PROBE_INCONCLUSIVE) << 8, 9, 11] {
            assert_eq!(
                outcome(ExitStatus::from_raw(raw)),
                ProbeOutcome::Inconclusive
            );
        }
    }

    #[test]
    fn a_restart_leaves_the_renderer_this_launch_chose_behind() {
        let restart = include_str!("linux_graphics.rs")
            .split("pub(crate) fn restart")
            .nth(1)
            .unwrap();
        assert!(restart.contains("command.env_remove(name)"));
        for caller in [include_str!("relaunch.rs"), include_str!("updates.rs")] {
            assert!(caller.contains("crate::linux_graphics::restart(&app.env());"));
        }
    }

    #[test]
    fn timeout_kills_and_reaps_the_probe() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep child");
        let error =
            wait_bounded(&mut child, Duration::from_millis(25)).expect_err("probe deadline");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            child
                .try_wait()
                .expect("reaped child")
                .expect("exit status")
                .signal(),
            Some(9)
        );
    }
}
