//! The line protocol both benchmark drivers speak: JSON objects on stdout,
//! one command at a time on stdin, each answered by a line with the same
//! `id`. The Zephium app prefixes its lines with `@bench ` because its own
//! logs share the stream.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// What a driver gets beyond a command's own timeout to answer.
pub(super) const REPLY_GRACE: Duration = Duration::from_secs(15);
const EXIT_GRACE: Duration = Duration::from_secs(20);
const MARKER: &str = "@bench";

#[derive(Debug)]
pub(super) enum Failure {
    /// The browser exited or stopped answering; nothing more can be asked.
    Gone(String),
    /// The driver refused or failed this one command.
    Rejected(String),
}

impl Failure {
    pub(super) fn message(&self) -> &str {
        match self {
            Failure::Gone(message) | Failure::Rejected(message) => message,
        }
    }

    pub(super) fn is_gone(&self) -> bool {
        matches!(self, Failure::Gone(_))
    }
}

pub(super) fn parse_line(line: &str) -> Option<Value> {
    let line = line.trim();
    let line = line.strip_prefix(MARKER).unwrap_or(line).trim_start();
    if !line.starts_with('{') {
        return None;
    }
    serde_json::from_str(line).ok()
}

pub(super) struct Driver {
    child: Child,
    stdin: Option<ChildStdin>,
    /// `None` is the end of the driver's output.
    lines: Receiver<Option<Value>>,
    next_id: u64,
    browser_pid: Option<u32>,
    gone: Option<String>,
}

impl Driver {
    /// Starts the driver and waits for its `ready` event, which is returned.
    pub(super) fn start(
        mut command: Command,
        stderr: Stdio,
        ready_within: Duration,
    ) -> Result<(Driver, Value), String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .map_err(|e| format!("cannot start the driver: {e}"))?;
        let stdout = child.stdout.take().ok_or("driver has no stdout")?;
        let stdin = child.stdin.take();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(message) = parse_line(&line) {
                    if sender.send(Some(message)).is_err() {
                        return;
                    }
                }
            }
            let _ = sender.send(None);
        });
        let mut driver = Driver {
            child,
            stdin,
            lines,
            next_id: 0,
            browser_pid: None,
            gone: None,
        };
        let ready = driver.wait_ready(ready_within)?;
        driver.browser_pid = ready
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok());
        Ok((driver, ready))
    }

    fn wait_ready(&mut self, within: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + within;
        loop {
            match self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Some(message)) => match message.get("event").and_then(Value::as_str) {
                    Some("ready") => return Ok(message),
                    Some("exited") => return Err(exit_reason(&message)),
                    Some("error") => {
                        return Err(message
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("the browser reported an error")
                            .to_string())
                    }
                    _ => {}
                },
                Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                    return Err("the driver exited before it was ready".into())
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!("not ready after {} s", within.as_secs()))
                }
            }
        }
    }

    /// Sends one command and returns its reply. `timeout` is how long the
    /// reply may take; the driver's own `timeoutMs` should be shorter.
    pub(super) fn call(
        &mut self,
        op: &str,
        mut fields: Value,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        if let Some(reason) = &self.gone {
            return Err(Failure::Gone(reason.clone()));
        }
        self.next_id += 1;
        let id = self.next_id;
        fields["id"] = json!(id);
        fields["op"] = json!(op);
        let sent = self
            .stdin
            .as_mut()
            .map(|stdin| writeln!(stdin, "{fields}").and_then(|()| stdin.flush()));
        if !matches!(sent, Some(Ok(()))) {
            return Err(self.lost("the driver's input closed"));
        }
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Some(message)) => {
                    if message.get("event").and_then(Value::as_str) == Some("exited") {
                        return Err(self.lost(&exit_reason(&message)));
                    }
                    if message.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    return if message.get("ok").and_then(Value::as_bool) == Some(true) {
                        Ok(message)
                    } else {
                        Err(Failure::Rejected(
                            message
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("the driver refused the command")
                                .to_string(),
                        ))
                    };
                }
                Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.lost("the browser exited"))
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(self.lost(&format!("no answer to {op} in {} s", timeout.as_secs())))
                }
            }
        }
    }

    fn lost(&mut self, reason: &str) -> Failure {
        self.gone = Some(reason.to_string());
        Failure::Gone(reason.to_string())
    }

    /// Asks the browser to quit and waits for the driver to go.
    pub(super) fn quit(&mut self) {
        if self.gone.is_none() {
            let _ = self.call("quit", json!({}), EXIT_GRACE);
        }
        self.stdin = None;
        self.reap();
    }

    fn reap(&mut self) {
        let deadline = Instant::now() + EXIT_GRACE;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Some(pid) = self.browser_pid {
            super::measure::kill_tree(pid);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        // Closing stdin tells a healthy driver to quit its browser.
        self.stdin = None;
        self.reap();
    }
}

fn exit_reason(event: &Value) -> String {
    match event.get("error").and_then(Value::as_str) {
        Some(error) => format!("the browser exited: {error}"),
        None => format!("the browser exited (code {})", event["code"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_lines_are_json_objects_with_an_optional_marker() {
        let plain = parse_line("{\"event\":\"ready\",\"pid\":7}").unwrap();
        assert_eq!(plain["pid"], 7);
        let marked = parse_line("@bench {\"id\":3,\"ok\":true}\r\n").unwrap();
        assert_eq!(marked["id"], 3);
        assert_eq!(parse_line("@bench{\"id\":4}").unwrap()["id"], 4);
    }

    #[test]
    fn other_output_is_ignored() {
        for line in [
            "",
            "2026-11-02 INFO started",
            "@bench not json",
            "[1, 2]",
            "{broken",
        ] {
            assert!(parse_line(line).is_none(), "{line:?}");
        }
    }

    #[test]
    fn an_exit_names_its_cause() {
        assert_eq!(
            exit_reason(&json!({ "event": "exited", "code": null, "error": "no such file" })),
            "the browser exited: no such file"
        );
        assert_eq!(
            exit_reason(&json!({ "event": "exited", "code": 1 })),
            "the browser exited (code 1)"
        );
    }
}
