use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const PROBE_ARGUMENT: &str = "--zephium-internal-gtk-gl-probe";
const GL_UNAVAILABLE: u8 = 20;
const PROBE_INCONCLUSIVE: u8 = 21;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
enum ProbeOutcome {
    Supported,
    Unavailable,
    Inconclusive,
}

pub fn prepare() -> Option<ExitCode> {
    if std::env::args_os().skip(1).eq([PROBE_ARGUMENT]) {
        return Some(probe());
    }
    // GDK reads this once during initialization. Honor explicit user choices.
    if std::env::var_os("GDK_GL").is_some() {
        return None;
    }
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("graphics: could not locate GTK context probe executable: {error}");
            return None;
        }
    };
    let default = run_probe(&executable, false);
    if default != ProbeOutcome::Unavailable {
        if default == ProbeOutcome::Inconclusive {
            eprintln!("graphics: GTK context probe did not complete; retaining system defaults");
        }
        return None;
    }
    if run_probe(&executable, true) != ProbeOutcome::Supported {
        eprintln!(
            "graphics: GTK could not create a default GL context or verify a GLES fallback; \
             rendering may be incomplete. Check the system graphics driver and GTK installation"
        );
        return None;
    }
    eprintln!("graphics: using verified GLES context after default GTK GL initialization failed");
    // Re-exec sets the process-local choice before GTK or runtime threads start.
    let error = Command::new(executable)
        .args(std::env::args_os().skip(1))
        .env("GDK_GL", "gles")
        .exec();
    eprintln!("graphics: could not restart with the verified GLES context: {error}");
    Some(ExitCode::FAILURE)
}

fn run_probe(executable: &std::path::Path, gles: bool) -> ProbeOutcome {
    let mut command = Command::new(executable);
    command
        .arg(PROBE_ARGUMENT)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if gles {
        command.env("GDK_GL", "gles");
    }
    match command
        .spawn()
        .and_then(|mut child| wait_bounded(&mut child, PROBE_TIMEOUT))
    {
        Ok(status) => outcome(status),
        Err(_) => ProbeOutcome::Inconclusive,
    }
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

fn probe() -> ExitCode {
    use gtk::prelude::*;

    if gtk::init().is_err() {
        return ExitCode::from(PROBE_INCONCLUSIVE);
    }
    // Realize without mapping: no visible window, focus transfer, or WebKit.
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_accept_focus(false);
    window.set_focus_on_map(false);
    window.set_default_size(1, 1);
    window.realize();
    if window.is_visible() || window.is_mapped() {
        return ExitCode::from(PROBE_INCONCLUSIVE);
    }
    let Some(native) = window.window() else {
        return ExitCode::from(PROBE_INCONCLUSIVE);
    };
    let Ok(context) = native.create_gl_context() else {
        return ExitCode::from(GL_UNAVAILABLE);
    };
    if context.realize().is_err() {
        return ExitCode::from(GL_UNAVAILABLE);
    }
    if std::env::var_os("GDK_GL").is_some_and(|value| value == "gles") && !context.uses_es() {
        return ExitCode::from(GL_UNAVAILABLE);
    }
    ExitCode::SUCCESS
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
    fn timeout_kills_and_reaps_the_probe() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep child");
        let error = wait_bounded(&mut child, Duration::from_millis(25))
            .expect_err("sleep exceeds the probe deadline");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let status = child
            .try_wait()
            .expect("reaped child")
            .expect("exit status");
        assert_eq!(status.signal(), Some(9));
    }
}
