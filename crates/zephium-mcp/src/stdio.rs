//! A stdio server as a child process in its own process group: only the
//! environment the caller gives it, stderr drained and dropped, and stdout
//! read through a bound on any one message.
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::{McpError, MAX_MESSAGE_BYTES};

#[cfg(windows)]
#[path = "stdio_windows.rs"]
mod windows;

pub struct StdioServer {
    /// An absolute path to the program.
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The whole environment the server gets.
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

/// The running child; dropping it ends the server and its group.
pub struct Process {
    child: Option<Child>,
    #[cfg(all(test, unix))]
    before_escalation: Option<Box<dyn FnOnce() + Send>>,
    group: Option<i32>,
    #[cfg(windows)]
    job: windows::Job,
}

impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(windows)]
        self.job.stop();
        #[cfg(unix)]
        if let Some(group) = self.group {
            // Retain the unreaped leader until SIGKILL so its pid (and pgid)
            // cannot be reused, including if spawning the worker fails.
            let child = std::sync::Arc::new(self.child.take());
            let retained = child.clone();
            #[cfg(test)]
            let before_escalation = self.before_escalation.take();
            // SAFETY: a negative pid addresses the group this process created
            // for its own child; no other process is named.
            unsafe {
                libc::kill(-group, libc::SIGTERM);
            }
            if std::thread::Builder::new()
                .name("mcp-stop".into())
                .spawn(move || {
                    #[cfg(test)]
                    if let Some(before_escalation) = before_escalation {
                        before_escalation();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    // SAFETY: this is the child's process group, as above.
                    unsafe {
                        libc::kill(-group, libc::SIGKILL);
                    }
                    drop(retained);
                })
                .is_err()
            {
                // SAFETY: if no worker can run, end the same group now.
                unsafe {
                    libc::kill(-group, libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        let _ = self.group;
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }
}

impl StdioServer {
    pub(crate) fn spawn(&self) -> Result<(Process, Bounded<ChildStdout>, ChildStdin), McpError> {
        if !self.program.is_absolute() || self.args.len() > 64 {
            return Err(McpError::Spawn);
        }
        let mut command = tokio::process::Command::new(&self.program);
        #[cfg(windows)]
        let job = windows::Job::new()?;
        command
            .args(&self.args)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // The server and whatever it starts share one group, ended together.
        #[cfg(unix)]
        command.process_group(0);
        // Admit the complete tree before the suspended child executes any instruction.
        #[cfg(windows)]
        command.creation_flags(0x0800_0000 | 0x0000_0004);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|_| McpError::Spawn)?;
        #[cfg(windows)]
        job.assign_and_resume(&child)?;
        let group = cfg!(unix)
            .then(|| child.id().and_then(|id| i32::try_from(id).ok()))
            .flatten();
        let stdout = child.stdout.take().ok_or(McpError::Spawn)?;
        let stdin = child.stdin.take().ok_or(McpError::Spawn)?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            });
        }
        Ok((
            Process {
                child: Some(child),
                #[cfg(all(test, unix))]
                before_escalation: None,
                group,
                #[cfg(windows)]
                job,
            },
            Bounded::new(stdout),
            stdin,
        ))
    }
}

/// Fails the stream once a line runs past `MAX_MESSAGE_BYTES`, so a server
/// cannot make the client buffer without end.
pub struct Bounded<R> {
    inner: R,
    line: usize,
    limit: usize,
    failed: bool,
}
impl<R> Bounded<R> {
    pub fn new(inner: R) -> Self {
        Self::with_limit(inner, MAX_MESSAGE_BYTES)
    }
    pub fn with_limit(inner: R, limit: usize) -> Self {
        Self {
            inner,
            line: 0,
            limit,
            failed: false,
        }
    }
}
impl<R: AsyncRead + Unpin> AsyncRead for Bounded<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.failed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "message too long",
            )));
        }
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            let fresh = &buf.filled()[before..];
            let mut line = self.line;
            for byte in fresh {
                if *byte == b'\n' {
                    line = 0;
                } else {
                    line += 1;
                    if line > self.limit {
                        self.failed = true;
                        buf.set_filled(before);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "message too long",
                        )));
                    }
                }
            }
            self.line = line;
        }
        polled
    }
}

#[cfg(test)]
mod tests {
    use super::{Bounded, ChildStdout, Process, StdioServer};
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    const FIXTURE_TEST: &str = "stdio::tests::process_tree_fixture";
    const FIXTURE_ROLE: &str = "ZEPHIUM_MCP_PROCESS_TREE_FIXTURE";
    const DESCENDANT: &str = "mcp-fixture-descendant=";

    #[cfg(unix)]
    fn waited_pid(info: &libc::siginfo_t) -> libc::pid_t {
        #[cfg(target_os = "linux")]
        // SAFETY: waitid filled this zero-initialized child-status structure.
        unsafe {
            info.si_pid()
        }
        #[cfg(not(target_os = "linux"))]
        {
            info.si_pid
        }
    }

    #[test]
    #[ignore = "invoked by the process-tree parent test"]
    #[expect(
        clippy::zombie_processes,
        reason = "the fixture parent must exit before its owned descendant"
    )]
    fn process_tree_fixture() {
        let Some(role) = std::env::var_os(FIXTURE_ROLE) else {
            return;
        };
        if role == "descendant" {
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        assert_eq!(role, "parent");
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", FIXTURE_TEST, "--ignored", "--nocapture"])
            .env(FIXTURE_ROLE, "descendant")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        println!("{DESCENDANT}{}", child.id());
        std::io::stdout().flush().unwrap();
    }

    pub(super) async fn orphaned_descendant() -> (Process, u32, BufReader<Bounded<ChildStdout>>) {
        let server = StdioServer {
            program: std::env::current_exe().unwrap(),
            args: ["--exact", FIXTURE_TEST, "--ignored", "--nocapture"]
                .map(str::to_owned)
                .to_vec(),
            env: vec![(FIXTURE_ROLE.into(), "parent".into())],
            cwd: None,
        };
        let (mut process, stdout, stdin) = server.spawn().unwrap();
        drop(stdin);
        let mut output = BufReader::new(stdout).lines();
        let descendant = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let line = output
                    .next_line()
                    .await
                    .unwrap()
                    .expect("fixture exited without a descendant");
                // Libtest may prefix the marker with the test name on this line.
                if let Some((_, pid)) = line.split_once(DESCENDANT) {
                    break pid.trim().parse::<u32>().unwrap();
                }
            }
        })
        .await
        .expect("fixture did not report its descendant");
        #[cfg(not(unix))]
        assert!(tokio::time::timeout(
            Duration::from_secs(10),
            process.child.as_mut().unwrap().wait()
        )
        .await
        .expect("fixture parent did not exit")
        .unwrap()
        .success());
        #[cfg(unix)]
        {
            // Observe exit without reaping the leader or releasing its pid.
            let leader = process.child.as_mut().unwrap().id().unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    // SAFETY: siginfo_t contains only integer and pointer fields;
                    // all-zero bits are valid before waitid fills the structure.
                    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
                    assert_eq!(
                        // SAFETY: info is writable and lives through the call;
                        // leader identifies our child and WNOWAIT leaves it unreaped.
                        unsafe {
                            libc::waitid(
                                libc::P_PID,
                                leader,
                                &mut info,
                                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                            )
                        },
                        0
                    );
                    if waited_pid(&info) == i32::try_from(leader).unwrap() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("fixture parent did not exit");
        }
        (process, descendant, output.into_inner())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_server_ends_a_descendant_even_after_parent_exits() {
        let (process, descendant, mut stdout) = orphaned_descendant().await;
        assert_eq!(
            // SAFETY: signal zero only probes this fixture pid and takes no pointers.
            unsafe { libc::kill(i32::try_from(descendant).unwrap(), 0) },
            0
        );
        drop(process);
        // The sleeping descendant retains stdout until its process exits.
        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::io::copy(&mut stdout, &mut tokio::io::sink()),
        )
        .await
        .expect("descendant survived after Process was dropped")
        .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_reserves_the_exited_leader_until_escalation() {
        let (mut process, _, mut stdout) = orphaned_descendant().await;
        let leader = process.child.as_ref().unwrap().id().unwrap();
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let (started, worker) = std::sync::mpsc::channel();
        process.before_escalation = Some(Box::new(move || {
            started.send(()).unwrap();
            let _ = gate.recv();
        }));
        drop(process);
        worker.recv().unwrap();
        // SAFETY: zero is a valid initial value for siginfo_t's integer and pointer fields.
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        // SAFETY: info remains writable for the call; leader is our child pid,
        // and WNOWAIT observes its exit without reaping it.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                leader,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            )
        };
        // Release cleanup even if the ownership assertion fails.
        drop(release);
        assert_eq!(result, 0, "leader was reaped before escalation");
        assert_eq!(waited_pid(&info), i32::try_from(leader).unwrap());
        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::io::copy(&mut stdout, &mut tokio::io::sink()),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            // SAFETY: signal zero only probes the pid and takes no pointers.
            while unsafe { libc::kill(i32::try_from(leader).unwrap(), 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("leader was not reaped after escalation");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_server_kills_a_group_ignoring_sigterm() {
        let server = StdioServer {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "trap '' TERM; /bin/sleep 60 & echo $!; wait".into(),
            ],
            env: vec![],
            cwd: None,
        };
        let (mut process, stdout, stdin) = server.spawn().unwrap();
        drop(stdin);
        let group = process.group.unwrap();
        let mut stdout = BufReader::new(stdout);
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), stdout.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let descendant: i32 = line.trim().parse().unwrap();
        // SAFETY: signal zero only probes the fixture pid and takes no pointers.
        assert_eq!(unsafe { libc::kill(descendant, 0) }, 0);
        let (started, worker) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel::<()>();
        process.before_escalation = Some(Box::new(move || {
            started.send(std::thread::current().id()).unwrap();
            let _ = gate.recv();
        }));
        drop(process);
        // Drop returns while escalation is still blocked on this test.
        let worker_thread = worker.recv().unwrap();
        drop(release);
        assert_ne!(worker_thread, std::thread::current().id());
        let ended = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::io::copy(&mut stdout, &mut tokio::io::sink()),
        )
        .await;
        // Also clean up the fixture when exercising the unfixed implementation.
        // SAFETY: group is the positive process group id created for this fixture;
        // negating it targets that group, and kill takes no pointers.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
        ended
            .expect("SIGTERM-ignoring descendant survived drop")
            .unwrap();
        for _ in 0..100 {
            // SAFETY: both calls use signal zero to probe fixture pids without
            // sending a signal or accessing memory through pointers.
            if unsafe { libc::kill(descendant, 0) } != 0 && unsafe { libc::kill(group, 0) } != 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("server or descendant still exists");
    }

    #[tokio::test]
    async fn a_newline_cannot_hide_an_overlong_message_in_one_read_or_across_reads() {
        for split in [false, true] {
            let mut stream = Bounded::with_limit(&b"12345\nok\n"[..], 4);
            if split {
                let mut first = [0; 2];
                stream.read_exact(&mut first).await.unwrap();
                assert_eq!(&first, b"12");
            }
            let mut output = [0; 32];
            assert_eq!(
                stream.read(&mut output).await.unwrap_err().kind(),
                std::io::ErrorKind::InvalidData
            );
            assert_eq!(
                stream.read(&mut output).await.unwrap_err().kind(),
                std::io::ErrorKind::InvalidData
            );
        }
        let mut stream = Bounded::with_limit(&b"1234\nok\n"[..], 4);
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, b"1234\nok\n");
    }
}
