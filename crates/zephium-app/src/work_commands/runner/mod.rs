//! Typed operations over a brokered copy. The ordinary shell gate is independent.
pub(crate) mod git_index;
pub(crate) mod github;
#[cfg(target_os = "macos")]
pub(crate) mod guard;
#[cfg(target_os = "macos")]
mod guard_git;
#[cfg(target_os = "macos")]
mod guard_github;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_github;
#[cfg(target_os = "macos")]
mod macos_python;
#[cfg(target_os = "macos")]
mod macos_spawn;
pub(crate) mod python;
mod review;
mod snapshot;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_github;
mod worker;
pub(crate) mod worker_slots;
use crate::work_files::WorkFileGrant;
use snapshot::Snapshot;
use std::{future::Future, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Failure {
    Unavailable,
    #[cfg(any(target_os = "macos", windows))]
    ToolUnavailable,
    Input,
    Limit,
    Storage,
    #[cfg(any(target_os = "macos", windows, test))]
    Launch,
    Stopped,
    #[cfg(any(target_os = "macos", windows))]
    StopUnconfirmed,
    Failed,
    Changed,
    Sensitive,
    PythonUnavailable,
    GitHubUnavailable,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    CustodyUnavailable,
    CleanupPending,
    WorkerBusy,
}
impl Failure {
    pub fn note(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "Confined Git inspection is not supported on this operating-system build"
            }
            Self::CustodyUnavailable => "Git inspection is unavailable without the qualified private runner guard; guarded file tools remain available",
            #[cfg(any(target_os = "macos", windows))]
            Self::ToolUnavailable => {
                "Confined Git inspection needs a supported local Git installation"
            }
            Self::Input => "The folder could not be copied safely into a bounded workspace",
            Self::Limit => "The copied workspace or command exceeded its limit",
            Self::Storage => "The private copied workspace could not be prepared",
            #[cfg(any(target_os = "macos", windows, test))]
            Self::Launch => "The confined process could not be started",
            Self::Stopped => "The confined process stopped",
            Self::WorkerBusy => "Runner worker capacity is occupied by running work or unconfirmed cleanup",
            Self::CleanupPending => "The runner operation or copied-workspace cleanup is not yet confirmed; do not automatically retry",
            #[cfg(any(target_os = "macos", windows))]
            Self::StopUnconfirmed => "Stop was requested, but process exit has not been confirmed",
            Self::Failed => "The operation in the copied workspace failed",
            Self::GitHubUnavailable => "Public GitHub CLI reads are unavailable without the qualified private runner guard, Apple silicon macOS build 25G83 and the pinned gh 2.96.0 runtime; browser reads remain available",
            Self::PythonUnavailable => "Python tests require the qualified private runner guard on Apple silicon macOS build 25G83 and the pinned Apple Command Line Tools Python 3.9 runtime; Windows awaits runtime packaging and native qualification",
            Self::Sensitive => "Recognizable credential content was withheld from the review",
            Self::Changed => "An approved file changed again; read it before reviewing these edits",
        }
    }
}

/// A stalled actor/store query is not permission to continue. Bound this await
/// independently of the process deadline so it cannot hold a select arm forever.
pub(crate) async fn cancellation_requested<C, CF>(cancelled: &C) -> bool
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
{
    tokio::time::timeout(std::time::Duration::from_millis(100), cancelled())
        .await
        .unwrap_or(true)
}

/// All results describe a copied workspace, never the source repository history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadOperation {
    #[cfg(all(test, target_os = "macos"))]
    Files,
    Status,
    Diff,
    Check,
}

pub(crate) struct Output {
    pub text: String,
    pub success: bool,
    pub exit: i32,
}

/// The first approved version and latest accepted version of one local file.
#[derive(Clone)]
pub(crate) struct ApprovedChange {
    pub path: std::path::PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
}

/// Review exactly the approved edits, in a fresh private copy with synthetic
/// baseline history. No original repository status, configuration or history.
pub(crate) async fn review<C, CF>(
    grant: &WorkFileGrant,
    root: &Path,
    changes: &[ApprovedChange],
    operation: ReadOperation,
    cancelled: C,
) -> Result<Output, Failure>
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
{
    review::run(grant, root, changes, operation, cancelled).await
}

#[cfg(not(target_os = "macos"))]
fn prepare_baseline_checked(
    snapshot: &Snapshot,
    changes: &[ApprovedChange],
    check: &dyn Fn() -> Result<(), Failure>,
) -> Result<(), Failure> {
    let mut seen = std::collections::BTreeSet::new();
    let mut total: usize = snapshot.files.values().map(Vec::len).sum();
    for change in changes {
        check()?;
        if !seen.insert(&change.path) || snapshot.files.get(&change.path) != Some(&change.after) {
            return Err(Failure::Changed);
        }
        total = total
            .checked_sub(change.after.len())
            .and_then(|total| total.checked_add(change.before.as_ref().map_or(0, Vec::len)))
            .ok_or(Failure::Limit)?;
        if total > snapshot::MAX_TOTAL {
            return Err(Failure::Limit);
        }
        match &change.before {
            Some(bytes) => snapshot.write(&change.path, bytes)?,
            None => std::fs::remove_file(snapshot.work.join(&change.path))
                .map_err(|_| Failure::Storage)?,
        }
    }
    Ok(())
}

/// Native recovery was qualified independently of command-child custody.
/// Sharing the measured OS/architecture check must not share command authority.
pub(crate) fn native_recovery_qualified() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::qualified()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub(crate) fn ensure_available() -> Result<(), Failure> {
    #[cfg(target_os = "macos")]
    if macos::qualified() {
        if guard::git_installed() {
            return Ok(());
        }
        // Preserve the typed implementation, but do not advertise a host-owned
        // timer as surviving host death. Reopen only after guard qualification.
        return Err(Failure::CustodyUnavailable);
    }
    // Native Windows token/ACL/network/alias qualification is a separate gate.
    Err(Failure::Unavailable)
}

#[derive(Clone, Copy)]
enum GitOperation {
    Initialize,
    CaptureIndex,
    CommitBaseline,
    IntentToAdd,
    Read(ReadOperation),
}
#[cfg(any(target_os = "macos", windows))]
impl GitOperation {
    fn arguments(self, snapshot: &Snapshot) -> Vec<std::ffi::OsString> {
        let mut args: Vec<std::ffi::OsString> = [
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "index.threads=1",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.quotePath=true",
            "-c",
            "core.autocrlf=false",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "user.name=Work",
            "-c",
            "user.email=snapshot@local.invalid",
            "-c",
            "commit.gpgsign=false",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push("-c".into());
        let mut hooks = std::ffi::OsString::from("core.hooksPath=");
        hooks.push(&snapshot.empty);
        args.push(hooks);
        let tail: &[&str] = match self {
            Self::Initialize => &[
                "init",
                "--quiet",
                "--initial-branch=work",
                "--template=",
                ".",
            ],
            Self::CaptureIndex => &["add", "--all", "--force", "--", "."],
            Self::CommitBaseline => &[
                "commit",
                "--quiet",
                "--allow-empty",
                "--no-verify",
                "--no-gpg-sign",
                "--message=Workspace baseline",
            ],
            Self::IntentToAdd => &[
                "add",
                "--intent-to-add",
                "--ignore-removal",
                "--force",
                "--",
                ".",
            ],
            #[cfg(all(test, target_os = "macos"))]
            Self::Read(ReadOperation::Files) => &["ls-files", "--cached"],
            Self::Read(ReadOperation::Status) => &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ],
            Self::Read(ReadOperation::Diff) => &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--",
            ],
            Self::Read(ReadOperation::Check) => {
                &["diff", "--check", "--no-ext-diff", "--no-textconv", "--"]
            }
        };
        args.extend(tail.iter().map(Into::into));
        args
    }
}

#[cfg(not(target_os = "macos"))]
async fn execute<C, CF>(
    snapshot: &Snapshot,
    operation: GitOperation,
    cancelled: &C,
) -> Result<Output, Failure>
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
{
    #[cfg(target_os = "macos")]
    {
        macos::git(snapshot, operation, cancelled).await
    }
    #[cfg(windows)]
    {
        windows::git(snapshot, operation, cancelled).await
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (snapshot, cancelled);
        match operation {
            GitOperation::Read(_operation) => Err(Failure::Unavailable),
            GitOperation::Initialize
            | GitOperation::CaptureIndex
            | GitOperation::CommitBaseline
            | GitOperation::IntentToAdd => Err(Failure::Unavailable),
        }
    }
}

#[cfg(any(target_os = "macos", windows, test))]
fn output_text(bytes: Vec<u8>) -> Result<String, Failure> {
    let text = String::from_utf8(bytes).map_err(|_| Failure::Failed)?;
    if zephium_agentic::contains_recognized_secret(&text) {
        return Err(Failure::Sensitive);
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r')
    {
        return Err(Failure::Failed);
    }
    Ok(text)
}

#[cfg(test)]
mod disclosure_tests {
    use super::*;
    #[test]
    fn full_diff_output_is_checked_before_evidence_clipping() {
        let output = format!("{}\n+ghp_inertFixtureToken", "ordinary\n".repeat(2500));
        assert_eq!(output_text(output.into_bytes()), Err(Failure::Sensitive));
        assert_eq!(
            output_text(b"+-----BEGIN OPENSSH PRIVATE KEY-----".to_vec()),
            Err(Failure::Sensitive)
        );
        assert_eq!(
            output_text(b"+ordinary change".to_vec()).unwrap(),
            "+ordinary change"
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    #[tokio::test]
    async fn native_runners_refuse_before_admitting_work_or_fetching() {
        assert_eq!(ensure_available(), Err(Failure::Unavailable));
        assert_eq!(python::ensure_available(), Err(Failure::PythonUnavailable));
        assert!(!native_recovery_qualified());
        assert!(!github::available());
        let result = github::run(&|| async { false }, || async {
            panic!("unsupported GitHub runner must not fetch remote data")
        })
        .await;
        assert!(matches!(result, Err(Failure::GitHubUnavailable)));
    }
}
