//! Command-line discovery and legacy adapters. Ambient CLI execution is closed
//! until replaced by qualified, typed brokers; consent does not provide confinement.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

/// This gate applies to legacy ambient execution, not qualified typed runners.
/// A newly qualified operation must migrate to its own broker rather than open
/// this generic path (which can load login files, credentials and repository config).
pub(crate) fn ensure_available() -> Result<(), &'static str> {
    Err("Command-line connections are unavailable until their file, credential and network access is confined. Use the website instead.")
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;

/// The tools Zephium knows how to use, in the order Settings lists them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Cli {
    Gh,
    Git,
    Codex,
    Claude,
}
impl Cli {
    pub const ALL: [Cli; 4] = [Cli::Gh, Cli::Git, Cli::Codex, Cli::Claude];
    pub fn program(self) -> &'static str {
        match self {
            Cli::Gh => "gh",
            Cli::Git => "git",
            Cli::Codex => "codex",
            Cli::Claude => "claude",
        }
    }
    pub fn id(self) -> &'static str {
        self.program()
    }
    fn version_args(self) -> &'static [&'static str] {
        &["--version"]
    }
    fn status_args(self) -> Option<&'static [&'static str]> {
        match self {
            Cli::Gh => Some(&["auth", "status", "--json", "hosts"]),
            Cli::Git => Some(&["config", "--global", "user.name"]),
            Cli::Codex => Some(&["login", "status"]),
            Cli::Claude => Some(&["auth", "status"]),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliAuth {
    /// Signed in; the account's public name when the tool says it.
    SignedIn(Option<String>),
    SignedOut,
    /// The tool has no account (git): ready to use.
    NotNeeded(Option<String>),
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliStatus {
    pub cli: Cli,
    pub path: Option<PathBuf>,
    /// "2.97.0".
    pub version: Option<String>,
    pub auth: CliAuth,
}
impl CliStatus {
    pub fn usable(&self) -> bool {
        self.path.is_some() && matches!(self.auth, CliAuth::SignedIn(_) | CliAuth::NotNeeded(_))
    }
}

fn located() -> &'static Mutex<HashMap<&'static str, Option<PathBuf>>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<PathBuf>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn executable(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["exe", "com", "cmd", "bat"]
                    .iter()
                    .any(|supported| extension.eq_ignore_ascii_case(supported))
            })
}

#[cfg(windows)]
fn windows_program_names(program: &str, pathext: &str) -> Vec<String> {
    pathext
        .split(';')
        .map(str::trim)
        .filter(|extension| {
            [".exe", ".com", ".cmd", ".bat"]
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
        .map(|extension| format!("{program}{extension}"))
        .collect()
}

#[cfg(unix)]
fn program_names(program: &str) -> Vec<String> {
    vec![program.to_owned()]
}

#[cfg(windows)]
fn program_names(program: &str) -> Vec<String> {
    windows_program_names(
        program,
        &std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()),
    )
}

/// Native search folders, followed by common per-user tool installations.
fn usual_folders() -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    #[cfg(unix)]
    folders.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
            .into_iter()
            .map(PathBuf::from),
    );
    #[cfg(unix)]
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for folder in [".local/bin", ".cargo/bin", "bin", ".bun/bin", ".volta/bin"] {
            folders.push(home.join(folder));
        }
    }
    #[cfg(windows)]
    {
        // Do not discover tools through a relative or empty PATH entry.
        folders.retain(|folder| folder.is_absolute());
        if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
            for folder in [".local/bin", ".cargo/bin", ".bun/bin", ".volta/bin"] {
                folders.push(home.join(folder));
            }
        }
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            folders.push(appdata.join("npm"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            folders.push(local.join("Microsoft/WinGet/Links"));
        }
        folders.retain(|folder| folder.is_absolute());
    }
    folders
}

/// Runs `script` in the person's login shell, within a few seconds.
#[cfg(unix)]
fn login_shell(script: &str) -> Option<String> {
    ensure_available().ok()?;
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|s| s.is_absolute() && executable(s))?;
    let mut child = std::process::Command::new(&shell)
        .args(["-lc", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(4) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    use std::io::Read;
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

#[cfg(unix)]
fn shell_is_fish() -> bool {
    std::env::var("SHELL").is_ok_and(|s| s.ends_with("/fish"))
}

/// The login shell's answer, for tools installed through a version manager.
#[cfg(unix)]
fn from_login_shell(program: &str) -> Option<PathBuf> {
    let out = login_shell(&format!("command -v {program}"))?;
    let path = PathBuf::from(out.lines().last()?.trim());
    (path.is_absolute() && executable(&path)).then_some(path)
}

/// Where `cli` is installed, cached for the process. Blocking.
pub fn locate(cli: Cli) -> Option<PathBuf> {
    let program = cli.program();
    if let Some(found) = located().lock().ok().and_then(|c| c.get(program).cloned()) {
        return found;
    }
    let names = program_names(program);
    let found = usual_folders()
        .into_iter()
        .flat_map(|folder| names.iter().map(move |name| folder.join(name)))
        .find(|path| executable(path));
    #[cfg(unix)]
    let found = found.or_else(|| from_login_shell(program));
    if let Ok(mut cache) = located().lock() {
        cache.insert(program, found.clone());
    }
    found
}

/// Native tool search PATH, including login-shell paths on Unix.
/// Cached for the process. Blocking.
pub fn login_path() -> String {
    if ensure_available().is_err() {
        return std::env::join_paths(
            usual_folders()
                .into_iter()
                .filter(|path| path.is_absolute()),
        )
        .ok()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_default();
    }
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        #[cfg(unix)]
        let script = if shell_is_fish() {
            "string join : $PATH"
        } else {
            "printf %s \"$PATH\""
        };
        #[cfg(unix)]
        let mut folders: Vec<PathBuf> = login_shell(script)
            .map(|text| {
                text.trim()
                    .split(':')
                    .filter(|f| f.starts_with('/'))
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        #[cfg(windows)]
        let mut folders = Vec::new();
        for folder in usual_folders() {
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        std::env::join_paths(folders)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| {
                #[cfg(unix)]
                {
                    "/usr/bin:/bin".into()
                }
                #[cfg(windows)]
                {
                    std::env::var("PATH").unwrap_or_default()
                }
            })
    })
    .clone()
}

/// Forgets where tools were, so Settings sees a fresh install.
pub fn forget() {
    if let Ok(mut cache) = located().lock() {
        cache.clear();
    }
}

/// Runs a tool with bounded output and time. `stdin` is written then closed.
pub async fn run(
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    stdin: Option<&str>,
    timeout: Duration,
) -> Option<(bool, String, String)> {
    ensure_available().ok()?;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command
        .args(args)
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(folder) = program.parent() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut folders: Vec<PathBuf> = std::env::split_paths(&path).collect();
        folders.insert(0, folder.to_path_buf());
        if let Ok(joined) = std::env::join_paths(folders) {
            command.env("PATH", joined);
        }
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().ok()?;
    if let (Some(text), Some(mut input)) = (stdin, child.stdin.take()) {
        let text = text.to_owned();
        tokio::spawn(async move {
            let _ = input.write_all(text.as_bytes()).await;
        });
    }
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let work = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_reader = (&mut stdout).take(MAX_OUTPUT_BYTES as u64);
        let mut err_reader = (&mut stderr).take(64 * 1024);
        let (a, b) = tokio::join!(
            out_reader.read_to_end(&mut out),
            err_reader.read_to_end(&mut err),
        );
        a.ok()?;
        b.ok()?;
        let status = child.wait().await.ok()?;
        Some((
            status.success(),
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        ))
    };
    tokio::time::timeout(timeout, work).await.ok().flatten()
}

/// "2.97.0" from the first line of a `--version`.
pub fn version_of(cli: Cli, text: &str) -> Option<String> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    line.split_whitespace()
        .find(|word| {
            let word = word.trim_start_matches('v');
            word.chars().next().is_some_and(|c| c.is_ascii_digit()) && word.contains('.')
        })
        .map(|word| word.trim_start_matches('v').to_owned())
        .or_else(|| (cli == Cli::Codex).then(|| line.trim().to_owned()))
}

/// Parses each tool's own status answer.
pub fn auth_of(cli: Cli, ok: bool, out: &str) -> CliAuth {
    match cli {
        Cli::Gh => {
            let Ok(value) = serde_json::from_str::<Value>(out) else {
                return CliAuth::Unknown;
            };
            let accounts = value["hosts"]["github.com"].as_array();
            match accounts.and_then(|accounts| {
                accounts
                    .iter()
                    .find(|a| a["active"] == Value::Bool(true) && a["state"] == "success")
            }) {
                Some(account) => CliAuth::SignedIn(account["login"].as_str().map(str::to_owned)),
                None => CliAuth::SignedOut,
            }
        }
        Cli::Git => {
            let name = out.trim();
            CliAuth::NotNeeded((ok && !name.is_empty()).then(|| name.chars().take(64).collect()))
        }
        Cli::Codex => {
            if ok && out.contains("Logged in") {
                CliAuth::SignedIn(if out.contains("ChatGPT") {
                    Some("ChatGPT".into())
                } else if out.contains("API key") {
                    Some("API key".into())
                } else {
                    None
                })
            } else {
                CliAuth::SignedOut
            }
        }
        Cli::Claude => match serde_json::from_str::<Value>(out) {
            Ok(value) if value["loggedIn"] == Value::Bool(true) => CliAuth::SignedIn(
                value["email"]
                    .as_str()
                    .or_else(|| value["authMethod"].as_str())
                    .map(str::to_owned),
            ),
            Ok(_) => CliAuth::SignedOut,
            Err(_) => CliAuth::Unknown,
        },
    }
}

/// One tool's status, found and asked afresh.
pub async fn status(cli: Cli) -> CliStatus {
    let path = tokio::task::spawn_blocking(move || locate(cli))
        .await
        .ok()
        .flatten();
    let Some(program) = path.clone() else {
        return CliStatus {
            cli,
            path,
            version: None,
            auth: CliAuth::Unknown,
        };
    };
    let (version, auth) = tokio::join!(
        run(&program, cli.version_args(), None, None, PROBE_TIMEOUT),
        async {
            match cli.status_args() {
                Some(args) => run(&program, args, None, None, PROBE_TIMEOUT).await,
                None => None,
            }
        }
    );
    CliStatus {
        cli,
        path,
        version: version.and_then(|(_, out, _)| version_of(cli, &out)),
        auth: auth.map_or(CliAuth::Unknown, |(ok, out, err)| {
            auth_of(cli, ok, if out.trim().is_empty() { &err } else { &out })
        }),
    }
}

/// Every known tool, asked in parallel.
pub async fn statuses() -> Vec<CliStatus> {
    let (gh, git, codex, claude) = tokio::join!(
        status(Cli::Gh),
        status(Cli::Git),
        status(Cli::Codex),
        status(Cli::Claude)
    );
    vec![gh, git, codex, claude]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn startup_discovery_and_legacy_calls_cannot_execute_shell_or_gh() {
        use std::os::unix::fs::PermissionsExt;
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert!(ensure_available().is_err());
        let directory = tempfile::Builder::new()
            .prefix("work-cli-gate-")
            .tempdir_in("/private/tmp")
            .unwrap();
        let marker = directory.path().join("executed");
        let program = directory.path().join("fixture-cli");
        std::fs::write(
            &program,
            format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        struct Restore {
            shell: Option<std::ffi::OsString>,
            located: Option<Option<PathBuf>>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.shell {
                    Some(value) => std::env::set_var("SHELL", value),
                    None => std::env::remove_var("SHELL"),
                }
                let mut cache = located().lock().unwrap();
                match &self.located {
                    Some(value) => {
                        cache.insert("gh", value.clone());
                    }
                    None => {
                        cache.remove("gh");
                    }
                }
            }
        }
        let previous = located()
            .lock()
            .unwrap()
            .insert("gh", Some(program.clone()));
        let _restore = Restore {
            shell: std::env::var_os("SHELL"),
            located: previous,
        };
        std::env::set_var("SHELL", &program);
        assert!(login_shell("ignored").is_none());
        let _ = login_path();
        assert_eq!(
            super::super::helper::ConnectionHelper::new().gh_ready(),
            super::super::github_broker::available()
        );
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                assert!(run(&program, &[], None, None, Duration::from_secs(1))
                    .await
                    .is_none());
                let status = status(Cli::Gh).await;
                assert!(!status.usable());
                assert_eq!(status.auth, CliAuth::Unknown);
            });
        assert!(!marker.exists());
    }

    #[test]
    fn statuses_from_recorded_output() {
        let gh = r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"octo","tokenSource":"keyring","scopes":"repo","gitProtocol":"https"}]}}"#;
        assert_eq!(
            auth_of(Cli::Gh, true, gh),
            CliAuth::SignedIn(Some("octo".into()))
        );
        let failed = r#"{"hosts":{"github.com":[{"state":"error","active":true,"host":"github.com","login":"octo"}]}}"#;
        assert_eq!(auth_of(Cli::Gh, true, failed), CliAuth::SignedOut);
        assert_eq!(
            auth_of(Cli::Gh, true, r#"{"hosts":{}}"#),
            CliAuth::SignedOut
        );
        assert_eq!(
            auth_of(Cli::Codex, true, "Logged in using ChatGPT\n"),
            CliAuth::SignedIn(Some("ChatGPT".into()))
        );
        assert_eq!(
            auth_of(Cli::Codex, false, "Not logged in\n"),
            CliAuth::SignedOut
        );
        assert_eq!(
            auth_of(
                Cli::Claude,
                true,
                r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "a@b.c"}"#
            ),
            CliAuth::SignedIn(Some("a@b.c".into()))
        );
        assert_eq!(
            auth_of(Cli::Claude, true, r#"{"loggedIn": false}"#),
            CliAuth::SignedOut
        );
        assert_eq!(
            auth_of(Cli::Git, true, "Ada\n"),
            CliAuth::NotNeeded(Some("Ada".into()))
        );
        assert_eq!(
            version_of(Cli::Gh, "gh version 2.97.0 (2026-07-31)\nhttps://…"),
            Some("2.97.0".into())
        );
        assert_eq!(
            version_of(Cli::Git, "git version 2.54.0 (Apple Git-157)"),
            Some("2.54.0".into())
        );
        assert_eq!(
            version_of(Cli::Codex, "codex-cli 0.153.4"),
            Some("0.153.4".into())
        );
        assert_eq!(
            version_of(Cli::Claude, "2.1.283 (Claude Code)"),
            Some("2.1.283".into())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "Legacy ambient CLI execution remains closed pending qualified broker replacement"]
    async fn run_is_bounded_and_feeds_stdin() {
        let (ok, out, _) = run(
            Path::new("/bin/cat"),
            &[],
            None,
            Some("hello"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(ok);
        assert_eq!(out, "hello");
        assert!(run(
            Path::new("/bin/sleep"),
            &["5"],
            None,
            None,
            Duration::from_millis(200)
        )
        .await
        .is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_discovery_respects_native_extensions() {
        assert_eq!(
            windows_program_names("codex", ".COM;.EXE;.BAT;.CMD;.PS1;.VBS;../bad"),
            ["codex.COM", "codex.EXE", "codex.BAT", "codex.CMD"]
        );
        let root = tempfile::tempdir().unwrap();
        let shim = root.path().join("codex.cmd");
        std::fs::write(&shim, "@echo off\r\n").unwrap();
        assert!(executable(&shim));
        let script = root.path().join("codex.ps1");
        std::fs::write(&script, "").unwrap();
        assert!(!executable(&script));
        assert!(!executable(&root.path().join("missing.exe")));
        let folder = root.path().join("directory.exe");
        std::fs::create_dir(&folder).unwrap();
        assert!(!executable(&folder));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn run_accepts_windows_command_shims() {
        let root = tempfile::tempdir().unwrap();
        let shim = root.path().join("tool.cmd");
        std::fs::write(&shim, "@echo off\r\necho %~1\r\n").unwrap();
        let (ok, out, _) = run(&shim, &["two words"], None, None, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(ok);
        assert_eq!(out.trim(), "two words");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn run_is_bounded_and_feeds_stdin() {
        let shell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let (ok, out, _) = run(
            &shell,
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::Out.Write([Console]::In.ReadToEnd())",
            ],
            None,
            Some("hello"),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert!(ok);
        assert_eq!(out, "hello");
        assert!(run(
            &shell,
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 5"
            ],
            None,
            None,
            Duration::from_millis(200)
        )
        .await
        .is_none());
    }
}
