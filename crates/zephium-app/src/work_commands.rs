//! Command lifecycle plumbing, gated until an OS-enforced runner is available.
//! Approval and working-directory selection are not filesystem confinement.
/// Handle the private confined-test guard before normal application initialization.
#[cfg(target_os = "macos")]
pub fn private_guard_main() {
    runner::guard::entry();
}
pub mod policy;
pub(crate) mod runner;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(any(unix, windows, test))]
use sha2::{Digest, Sha256};
#[cfg(any(unix, windows, test))]
use std::collections::VecDeque;
#[cfg(any(unix, windows))]
use std::{
    ffi::OsString,
    time::{Duration, Instant},
};
use std::{future::Future, path::Path};
#[cfg(target_os = "windows")]
pub use windows::run;
use zephium_core::work::runtime::*;

pub struct CommandResult {
    pub evidence: WorkCommandEvidenceV1,
    pub note: String,
    pub succeeded: bool,
}
#[cfg(any(unix, windows, test))]
struct Output {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    hash: Sha256,
    bytes: u64,
}
#[cfg(any(unix, windows, test))]
impl Output {
    fn new() -> Self {
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            hash: Sha256::new(),
            bytes: 0,
        }
    }
    fn push(&mut self, bytes: &[u8]) {
        self.hash.update(bytes);
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        let n = bytes.len().min(2048usize.saturating_sub(self.head.len()));
        self.head.extend_from_slice(&bytes[..n]);
        for b in &bytes[n..] {
            if self.tail.len() == MAX_WORK_COMMAND_MEMORY_BYTES - 2048 {
                self.tail.pop_front();
            }
            self.tail.push_back(*b);
        }
    }
    fn view(&self) -> WorkCommandOutputV1 {
        let retained: Vec<u8> = self.head.iter().chain(&self.tail).copied().collect();
        let cleaned = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes)
                .chars()
                .map(|c| {
                    if c.is_control() && c != '\n' && c != '\t' {
                        ' '
                    } else {
                        c
                    }
                })
                .collect::<String>()
        };
        let all = cleaned(&retained);
        let truncated =
            self.bytes > MAX_WORK_FILE_TEXT_BYTES as u64 || all.len() > MAX_WORK_FILE_TEXT_BYTES;
        let text = if !truncated {
            all
        } else {
            let mut h = all.len().min(2048);
            while !all.is_char_boundary(h) {
                h -= 1;
            }
            let mut t = all.len().saturating_sub(MAX_WORK_FILE_TEXT_BYTES - h);
            while !all.is_char_boundary(t) {
                t += 1;
            }
            format!("{}{}", &all[..h], &all[t..])
        };
        WorkCommandOutputV1 {
            elapsed_ms: 0,
            text,
            bytes: self.bytes.min(u64::from(u32::MAX)) as u32,
            truncated,
        }
    }
}

/// Fail closed on every platform, including previously approved commands.
/// A future native runner must enforce grants, protected paths and network
/// policy before this gate can be replaced. Approval never overrides it.
pub(crate) fn ensure_available() -> Result<(), &'static str> {
    Err("Commands and coding-agent hand-offs are unavailable until an OS-enforced runner can confine file and network access. Use the read, glob and grep tools.")
}

#[cfg(any(unix, windows))]
fn environment() -> Vec<(OsString, OsString)> {
    // No inherited variables: arbitrary names and shell startup variables can
    // carry credentials just as readily as TOKEN or API_KEY.
    #[cfg(unix)]
    let environment = vec![
        ("LANG".into(), "C.UTF-8".into()),
        ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
    ];
    #[cfg(windows)]
    let environment = vec![("LANG".into(), "C.UTF-8".into())];
    environment
}
#[cfg(unix)]
fn shell() -> OsString {
    "/bin/sh".into()
}

#[cfg(unix)]
mod groups {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    };
    type Row = (i32, Arc<AtomicBool>, bool);
    static CLOSING: AtomicBool = AtomicBool::new(false);
    fn registry() -> &'static Mutex<Vec<Row>> {
        static GROUPS: OnceLock<Mutex<Vec<Row>>> = OnceLock::new();
        GROUPS.get_or_init(Default::default)
    }
    #[cfg(target_os = "macos")]
    pub struct GuardControl(Mutex<Option<std::os::fd::OwnedFd>>);
    #[cfg(target_os = "macos")]
    impl GuardControl {
        pub fn new(fd: std::os::fd::OwnedFd) -> Self {
            Self(Mutex::new(Some(fd)))
        }
        pub fn close(&self) {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
        pub fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
            use std::os::fd::AsRawFd;
            let owned = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let fd = owned
                .as_ref()
                .ok_or(std::io::ErrorKind::BrokenPipe)?
                .as_raw_fd();
            let count = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len().min(65536)) };
            if count < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(count as usize)
            }
        }
    }
    #[cfg(target_os = "macos")]
    fn guards() -> &'static Mutex<Vec<std::sync::Weak<GuardControl>>> {
        static GUARDS: OnceLock<Mutex<Vec<std::sync::Weak<GuardControl>>>> = OnceLock::new();
        GUARDS.get_or_init(Default::default)
    }
    #[cfg(target_os = "macos")]
    pub fn launch_guard<T, E>(
        control: &Arc<GuardControl>,
        spawn: impl FnOnce() -> Result<T, E>,
        stopped: E,
    ) -> Result<T, E> {
        let _rows = registry().lock().unwrap_or_else(|e| e.into_inner());
        if CLOSING.load(Ordering::Acquire) {
            return Err(stopped);
        }
        let result = spawn()?;
        let mut controls = guards().lock().unwrap_or_else(|e| e.into_inner());
        controls.retain(|entry| entry.strong_count() != 0);
        controls.push(Arc::downgrade(control));
        Ok(result)
    }
    pub struct Group {
        pub pid: i32,
        pub shutdown: Arc<AtomicBool>,
        armed: bool,
        owned_single: bool,
    }
    impl Group {
        /// Spawn and registration share shutdown's lock. Typed confined work
        /// cannot start after the terminal process boundary or evade its drain.
        /// The caller must own every reap through poll_single_process; this is
        /// not suitable for independently reaped Tokio or multi-process children.
        #[cfg(target_os = "macos")]
        pub fn launch<T, E>(
            spawn: impl FnOnce() -> Result<(T, u32), E>,
            stopped: E,
        ) -> Result<(T, Self), E> {
            let mut rows = registry().lock().unwrap_or_else(|e| e.into_inner());
            if CLOSING.load(Ordering::Acquire) {
                return Err(stopped);
            }
            let (child, pid) = spawn()?;
            let shutdown = Arc::new(AtomicBool::new(false));
            rows.push((pid as i32, shutdown.clone(), true));
            Ok((
                child,
                Self {
                    pid: pid as i32,
                    shutdown,
                    armed: true,
                    owned_single: true,
                },
            ))
        }
        pub fn new(pid: u32) -> Self {
            let shutdown = Arc::new(AtomicBool::new(false));
            registry().lock().unwrap_or_else(|e| e.into_inner()).push((
                pid as i32,
                shutdown.clone(),
                false,
            ));
            Self {
                pid: pid as i32,
                shutdown,
                armed: true,
                owned_single: false,
            }
        }
        /// Reap only a qualified single-process group while shutdown is excluded.
        /// Removing its registry entry before releasing the lock prevents a
        /// completed numeric PID from being signaled after it can be reused.
        #[cfg(target_os = "macos")]
        pub fn poll_single_process<T>(
            &mut self,
            poll: impl FnOnce() -> std::io::Result<Option<T>>,
        ) -> std::io::Result<Option<T>> {
            let mut rows = registry().lock().unwrap_or_else(|e| e.into_inner());
            let result = poll();
            if matches!(&result, Ok(Some(_)))
                || result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.raw_os_error() == Some(libc::ECHILD))
            {
                self.armed = false;
                rows.retain(|(pid, _, _)| *pid != self.pid);
            }
            result
        }
        pub fn signal(&self, signal: i32) {
            if self.armed {
                signal_group(self.pid, signal, self.owned_single);
            }
        }
        pub fn quitting(&self) -> bool {
            self.shutdown.load(Ordering::Acquire)
        }
    }
    fn signal_group(pid: i32, signal: i32, owned_single: bool) {
        unsafe {
            if owned_single {
                // Qualified targets deny fork. Only this exact unreaped child
                // belongs to the operation; an unrelated same-session process
                // joining its process group must not receive our signal.
                libc::kill(pid, signal);
            } else {
                // Legacy Tokio owns reaping: retain its group-only protocol,
                // never promote a possibly recycled PID to an owned child.
                libc::kill(-pid, signal);
            }
        }
    }

    impl Drop for Group {
        fn drop(&mut self) {
            let mut rows = registry().lock().unwrap_or_else(|e| e.into_inner());
            self.signal(libc::SIGKILL);
            rows.retain(|(pid, _, _)| *pid != self.pid);
        }
    }
    pub fn shutdown() {
        let started = std::time::Instant::now();
        {
            let rows = registry().lock().unwrap_or_else(|e| e.into_inner());
            CLOSING.store(true, Ordering::Release);
            #[cfg(target_os = "macos")]
            for control in guards()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter_map(std::sync::Weak::upgrade)
            {
                control.close();
            }
            if rows.is_empty() {
                return;
            }
            for (pid, flag, single) in rows.iter() {
                flag.store(true, Ordering::Release);
                signal_group(*pid, libc::SIGTERM, *single);
            }
        }
        // The app must not exit with surviving children, even if its async
        // runtime stops polling the runner during native shutdown.
        loop {
            {
                let rows = registry().lock().unwrap_or_else(|e| e.into_inner());
                if rows.is_empty() {
                    return;
                }
                if started.elapsed() >= std::time::Duration::from_secs(2) {
                    for (pid, _, single) in rows.iter() {
                        signal_group(*pid, libc::SIGKILL, *single);
                    }
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    #[cfg(all(test, target_os = "macos"))]
    mod tests {
        use super::*;
        #[test]
        #[ignore = "Only the isolated fixture process closes the one-way shutdown gate"]
        fn guard_shutdown_fixture_child() {
            use std::{
                io::Read,
                os::fd::{FromRawFd, IntoRawFd, OwnedFd},
            };
            assert_eq!(
                std::env::var("ZEPHIUM_GUARD_SHUTDOWN_FIXTURE").as_deref(),
                Ok("1")
            );
            let (mut read, write) = std::io::pipe().unwrap();
            let write = unsafe { OwnedFd::from_raw_fd(write.into_raw_fd()) };
            let control = Arc::new(GuardControl::new(write));
            launch_guard(&control, || Ok::<_, ()>(()), ()).unwrap();
            assert_eq!(control.write(b"x").unwrap(), 1);
            shutdown();
            assert_eq!(
                control.write(b"y").unwrap_err().kind(),
                std::io::ErrorKind::BrokenPipe
            );
            let mut bytes = Vec::new();
            read.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"x");
            let attempted = AtomicBool::new(false);
            assert!(launch_guard(
                &control,
                || {
                    attempted.store(true, Ordering::Release);
                    Ok::<_, ()>(())
                },
                ()
            )
            .is_err());
            assert!(!attempted.load(Ordering::Acquire));
        }
        #[test]
        fn guard_shutdown_closes_control_without_async_polling_or_late_spawn() {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "work_commands::groups::tests::guard_shutdown_fixture_child",
                    "--ignored",
                ])
                .env("ZEPHIUM_GUARD_SHUTDOWN_FIXTURE", "1")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("isolated shutdown fixture did not settle");
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}
/// Called by the original shutdown owner, never from a persisted PID.
pub(crate) fn shutdown() {
    #[cfg(unix)]
    groups::shutdown();
    #[cfg(target_os = "windows")]
    windows::shutdown();
}

#[cfg(unix)]
pub async fn run<C, CF, P, PF>(
    cwd: &Path,
    command: &str,
    timeout_secs: u32,
    cancelled: C,
    progress: P,
) -> Result<CommandResult, &'static str>
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
    P: Fn(WorkCommandOutputV1) -> PF,
    PF: Future<Output = ()>,
{
    ensure_available()?;
    run_unconfined(cwd, command, timeout_secs, cancelled, progress).await
}

#[cfg(unix)]
async fn run_unconfined<C, CF, P, PF>(
    cwd: &Path,
    command: &str,
    timeout_secs: u32,
    cancelled: C,
    progress: P,
) -> Result<CommandResult, &'static str>
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
    P: Fn(WorkCommandOutputV1) -> PF,
    PF: Future<Output = ()>,
{
    use std::{os::unix::process::ExitStatusExt, process::Stdio};
    use tokio::{io::AsyncReadExt, process::Command};
    let started = Instant::now();
    let mut process = Command::new(shell());
    process
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env_clear()
        .envs(environment())
        .env("NO_COLOR", "1")
        .env("CI", "1")
        .env("TERM", "dumb")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    process.process_group(0);
    let mut child = process
        .spawn()
        .map_err(|_| "The command could not be started")?;
    let group = groups::Group::new(child.id().ok_or("The command could not be started")?);
    progress(Output::new().view()).await;
    let mut stdout = child.stdout.take().ok_or("The output could not be read")?;
    let mut stderr = child.stderr.take().ok_or("The output could not be read")?;
    let mut output = Output::new();
    let mut out_open = true;
    let mut err_open = true;
    let mut status = None;
    let mut out = [0u8; 8192];
    let mut err = [0u8; 8192];
    let mut stopping: Option<(Instant, String)> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut published = Instant::now();
    loop {
        tokio::select! {
            read = stdout.read(&mut out), if out_open => { match read { Ok(0) => out_open = false, Ok(n) => output.push(&out[..n]), Err(_) => { out_open = false; stopping.get_or_insert((Instant::now(), "The output could not be read".into())); group.signal(libc::SIGTERM); } } }
            read = stderr.read(&mut err), if err_open => { match read { Ok(0) => err_open = false, Ok(n) => output.push(&err[..n]), Err(_) => { err_open = false; stopping.get_or_insert((Instant::now(), "The output could not be read".into())); group.signal(libc::SIGTERM); } } }
            result = child.wait(), if status.is_none() => { status = Some(result.map_err(|_| "The command could not be waited for")?); }
            _ = tick.tick() => {
                if stopping.is_none() {
                    let note = if group.quitting() { Some("Stopped when the app quit".into()) }
                        else if started.elapsed() >= Duration::from_secs(u64::from(timeout_secs)) { Some(format!("Stopped after {timeout_secs} s")) }
                        else if cancelled().await { Some("Stopped".into()) } else { None };
                    if let Some(note) = note { group.signal(libc::SIGTERM); stopping = Some((Instant::now(), note)); }
                }
                if stopping.as_ref().is_some_and(|(at,_)| at.elapsed() >= Duration::from_secs(2)) {
                    group.signal(libc::SIGKILL);
                    if status.is_none() { status = tokio::time::timeout(Duration::from_millis(300), child.wait()).await.ok().and_then(Result::ok); }
                    break;
                }
                if published.elapsed() >= Duration::from_millis(300) {
                    let mut view = output.view();
                    view.elapsed_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
                    progress(view).await;
                    published = Instant::now();
                }
            }
        }
        if status.is_some() && !out_open && !err_open {
            break;
        }
    }
    let view = output.view();
    let succeeded = stopping.is_none() && status.is_some_and(|s| s.success());
    let note = stopping.map(|(_, note)| note).unwrap_or_else(|| {
        if succeeded {
            "Completed".into()
        } else {
            "The command failed".into()
        }
    });
    Ok(CommandResult {
        evidence: WorkCommandEvidenceV1 {
            cwd: cwd.to_string_lossy().into_owned(),
            command: command.into(),
            exit: status.and_then(|s| s.code()),
            signal: status.and_then(|s| s.signal()),
            elapsed_ms: started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
            bytes: view.bytes,
            digest: format!("{:x}", output.hash.finalize()),
            text: view.text,
            truncated: view.truncated,
        },
        note,
        succeeded,
    })
}
#[cfg(not(any(unix, windows)))]
pub async fn run<C, CF, P, PF>(
    _: &Path,
    _: &str,
    _: u32,
    _: C,
    _: P,
) -> Result<CommandResult, &'static str>
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
    P: Fn(WorkCommandOutputV1) -> PF,
    PF: Future<Output = ()>,
{
    Err("Commands are not available on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn commands_are_refused_before_spawn_or_progress() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("must-not-exist");
        let error = run(
            dir.path(),
            "touch must-not-exist",
            1,
            || async { false },
            |_| async { panic!("blocked command published output") },
        )
        .await
        .err()
        .unwrap();
        assert!(error.contains("OS-enforced runner"));
        assert!(!sentinel.exists());
        let env = environment();
        assert_eq!(env.len(), if cfg!(windows) { 1 } else { 2 });
        assert!(env.iter().all(|(name, _)| name == "LANG" || name == "PATH"));
    }
    #[test]
    fn work_output_cap_hash_and_utf8() {
        let unicode = format!("{}é tail", "x".repeat(2047));
        let mut exact = Output::new();
        exact.push(unicode.as_bytes());
        assert_eq!(exact.view().text, unicode);
        let mut output = Output::new();
        let bytes = vec![b'x'; 600_000];
        output.push(&bytes);
        output.push(b"final\xff");
        let view = output.view();
        assert!(view.truncated);
        assert!(view.text.ends_with("final�"));
        assert!(view.text.len() <= MAX_WORK_FILE_TEXT_BYTES);
        assert_eq!(view.bytes, 600006);
        assert!(output.head.len() + output.tail.len() <= MAX_WORK_COMMAND_MEMORY_BYTES);
        let mut expected = Sha256::new();
        expected.update(&bytes);
        expected.update(b"final\xff");
        assert_eq!(output.hash.finalize(), expected.finalize());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn work_quiet_command_publishes_start_and_elapsed_time() {
        let dir = tempfile::tempdir().unwrap();
        let updates = std::sync::Mutex::new(Vec::new());
        let result = run_unconfined(
            dir.path(),
            "sleep 1",
            10,
            || async { false },
            |view| {
                updates.lock().unwrap().push(view);
                async {}
            },
        )
        .await
        .unwrap();
        assert!(result.succeeded);
        let updates = updates.into_inner().unwrap();
        assert_eq!(updates[0].elapsed_ms, 0);
        assert_eq!(updates[0].bytes, 0);
        assert!(updates.iter().any(|view| view.elapsed_ms >= 300));
        assert!(updates
            .windows(2)
            .all(|pair| pair[0].elapsed_ms <= pair[1].elapsed_ms));
        assert!(updates.last().unwrap().elapsed_ms <= result.evidence.elapsed_ms);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn work_shell_read_timeout_and_stop() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_unconfined(
            dir.path(),
            "printf hello; printf error >&2",
            10,
            || async { false },
            |_| async {},
        )
        .await
        .unwrap();
        assert!(result.succeeded);
        assert!(result.evidence.text.contains("hello"));
        assert!(result.evidence.text.contains("error"));
        let started = Instant::now();
        let result = run_unconfined(
            dir.path(),
            "trap '' TERM; sleep 30 & wait",
            1,
            || async { false },
            |_| async {},
        )
        .await
        .unwrap();
        assert_eq!(result.note, "Stopped after 1 s");
        assert!(started.elapsed() < Duration::from_secs(4));
        let started = Instant::now();
        let result = run_unconfined(
            dir.path(),
            "/bin/sh -c 'trap \"\" TERM; sleep 30 & printf \"%s\" \"$!\"; wait'",
            30,
            || async { started.elapsed() >= Duration::from_millis(200) },
            |_| async {},
        )
        .await
        .unwrap();
        assert_eq!(result.note, "Stopped");
        assert!(started.elapsed() < Duration::from_secs(3));
        let child: i32 = result.evidence.text.trim().parse().unwrap();
        let reaped = Instant::now();
        while unsafe { libc::kill(child, 0) } == 0 && reaped.elapsed() < Duration::from_secs(1) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(unsafe { libc::kill(child, 0) }, -1, "child survived Stop");
    }
}
