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
    child: Child,
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
            // SAFETY: a negative pid addresses the group this process created
            // for its own child; no other process is named.
            unsafe {
                libc::kill(-group, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        let _ = self.group;
        let _ = self.child.start_kill();
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
                child,
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
        assert!(
            tokio::time::timeout(Duration::from_secs(10), process.child.wait())
                .await
                .expect("fixture parent did not exit")
                .unwrap()
                .success()
        );
        (process, descendant, output.into_inner())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_server_ends_a_descendant_even_after_parent_exits() {
        let (process, descendant, mut stdout) = orphaned_descendant().await;
        assert_eq!(
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
