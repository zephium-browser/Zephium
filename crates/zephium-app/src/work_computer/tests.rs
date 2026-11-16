//! The tools against a recording host: read-before-edit, exact and unique
//! edits, approvals that decide what is written, and command classes that
//! are exactly the policy's.
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::json;
use zephium_core::work::runtime::*;
use zephium_core::work::{WorkError, WorkStepId};

use super::*;
use crate::work_files::WorkFileGrant;

#[derive(Default)]
struct Recorder {
    begun: Mutex<Vec<(WorkStepKindV1, Option<WorkLocalStepV1>)>>,
    settled: Mutex<Vec<(WorkStepStatus, Option<String>)>>,
    decisions: Mutex<Vec<Decision>>,
    asked: Mutex<u32>,
    progress_calls: Mutex<u32>,
    evidence_count: Mutex<u32>,
    on_cancel_check: Mutex<Option<Box<dyn FnMut() -> bool + Send>>>,
}
impl Recorder {
    fn deciding(decisions: &[Decision]) -> Self {
        Self {
            decisions: Mutex::new(decisions.iter().rev().copied().collect()),
            ..Default::default()
        }
    }
    fn kinds(&self) -> Vec<WorkStepKindV1> {
        self.begun
            .lock()
            .unwrap()
            .iter()
            .map(|(k, _)| k.clone())
            .collect()
    }
}
impl ComputerHost for Recorder {
    fn recovery_profile(&self) -> Option<String> {
        Some("computer-test-profile".into())
    }
    fn begin(
        &self,
        kind: WorkStepKindV1,
        local: Option<WorkLocalStepV1>,
    ) -> HostFuture<'_, Result<WorkStepId, WorkError>> {
        self.begun.lock().unwrap().push((kind, local));
        Box::pin(async { Ok(WorkStepId::generate()) })
    }
    fn settle(
        &self,
        _: WorkStepId,
        status: WorkStepStatus,
        settled: Settled,
        note: Option<String>,
    ) -> HostFuture<'_, Result<Option<String>, WorkError>> {
        if !matches!(&settled, Settled::Nothing) {
            *self.evidence_count.lock().unwrap() += 1;
        }
        if let Settled::Command(command) = &settled {
            command.validate().unwrap();
        }
        self.settled.lock().unwrap().push((status, note));
        let key = match settled {
            Settled::Nothing => None,
            _ => Some(format!("s{}", self.settled.lock().unwrap().len())),
        };
        Box::pin(async move { Ok(key) })
    }
    fn progress(&self, _: WorkStepId, _: WorkCommandOutputV1) -> HostFuture<'_, ()> {
        *self.progress_calls.lock().unwrap() += 1;
        Box::pin(async {})
    }
    fn decision(&self, _: WorkStepId) -> HostFuture<'_, Result<Decision, WorkError>> {
        *self.asked.lock().unwrap() += 1;
        let decision = self
            .decisions
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(Decision::Stopped);
        Box::pin(async move { Ok(decision) })
    }
    fn folder_approved<'a>(&'a self, _: &'a str) -> HostFuture<'a, bool> {
        Box::pin(async { false })
    }
    fn cancelled(&self) -> HostFuture<'_, bool> {
        let cancelled = self
            .on_cancel_check
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|check| check());
        Box::pin(async move { cancelled })
    }
}

fn project() -> (
    tempfile::TempDir,
    crate::work_files::TestGrantHome,
    ComputerTools,
    PathBuf,
) {
    let home = crate::work_files::test_tempdir();
    let home_scope = crate::work_files::TestGrantHome::new(home.path().to_path_buf());
    let root = home.path().join("code/app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/out.rs"), "fn add() {}\n").unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn add(a: i32) -> i32 {\n    a + 2\n}\n\npub fn twice(a: i32) -> i32 {\n    a + 2 + a\n}\n",
    )
    .unwrap();
    let (grant, refused) = WorkFileGrant::admit(&[root.to_string_lossy().into_owned()]);
    assert!(refused.is_empty());
    let tools = ComputerTools::new(grant, &root.to_string_lossy()).unwrap();
    (home, home_scope, tools, root)
}

/// Runs a test body while its thread-local home fixture is active.
fn serially(body: impl std::future::Future<Output = ()>) {
    let _serial = crate::WORK_RUNTIME_TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap()
        .block_on(body);
}

#[test]
fn edits_need_a_read_a_unique_match_and_an_approval() {
    serially(async {
        let (_home, _home_scope, tools, root) = project();
        let host =
            Recorder::deciding(&[Decision::Declined, Decision::Approved, Decision::Approved]);
        let edit = |old: &str, new: &str| json!({"path": "src/lib.rs", "old": old, "new": new});

        let blind = tools
            .call(&host, "edit", &edit("a + 2\n}", "a + 1\n}"))
            .await;
        assert!(blind.is_error && blind.content.contains("Read src/lib.rs before editing"));

        let read = tools
            .call(&host, "read", &json!({"path": "src/lib.rs"}))
            .await;
        assert!(!read.is_error);
        assert!(read
            .content
            .starts_with("1: pub fn add(a: i32) -> i32 {\n2:     a + 2\n"));
        assert!(read.content.ends_with("(source s1)"), "{}", read.content);

        let twice = tools.call(&host, "edit", &edit("a + 2", "a + 1")).await;
        assert!(
            twice.is_error && twice.content.contains("occurs 2 times"),
            "{}",
            twice.content
        );
        let missing = tools.call(&host, "edit", &edit("a + 3", "a + 1")).await;
        assert!(missing.is_error && missing.content.contains("not found"));

        let declined = tools
            .call(&host, "edit", &edit("    a + 2\n}\n\n", "    a + 1\n}\n\n"))
            .await;
        assert!(declined.is_error && declined.content.contains("declined"));
        assert!(std::fs::read_to_string(root.join("src/lib.rs"))
            .unwrap()
            .contains("    a + 2\n}\n\npub"));

        let applied = tools
            .call(&host, "edit", &edit("    a + 2\n}\n\n", "    a + 1\n}\n\n"))
            .await;
        assert_eq!(applied.content, "Changed src/lib.rs (+1 −1).");
        // The applied change is the new known text: a second edit needs no re-read.
        let all = tools
            .call(&host, "edit", &json!({"path": "src/lib.rs", "old": "a: i32", "new": "value: i32", "replace_all": true}))
            .await;
        assert!(!all.is_error, "{}", all.content);
        let text = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert_eq!(text.matches("value: i32").count(), 2);
        assert!(text.contains("    a + 1\n}"));

        let kinds = host.kinds();
        assert!(matches!(
            kinds.last(),
            Some(WorkStepKindV1::WriteFile { .. })
        ));
        assert!(kinds
            .iter()
            .any(|k| matches!(k, WorkStepKindV1::EditFile { .. })));

        let diffs =
            tools.diffs(&[("src/lib.rs".to_owned(), "Adds one, not two".to_owned())].into());
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "src/lib.rs");
        assert_eq!(diffs[0].language, "rust");
        assert_eq!(diffs[0].summary, "Adds one, not two");
        assert_eq!((diffs[0].added, diffs[0].removed), (3, 3));
    });
}

#[test]
fn a_file_changed_since_it_was_read_is_read_again() {
    serially(async {
        let (_home, _home_scope, tools, root) = project();
        let host = Recorder::deciding(&[Decision::Approved]);
        tools
            .call(&host, "read", &json!({"path": "src/lib.rs"}))
            .await;
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn add(a: i32) -> i32 {\n    a + 5\n}\n",
        )
        .unwrap();
        let stale = tools
            .call(
                &host,
                "edit",
                &json!({"path": "src/lib.rs", "old": "a + 5", "new": "a + 1"}),
            )
            .await;
        assert!(
            stale.is_error && stale.content.contains("changed since you read it"),
            "{}",
            stale.content
        );
        assert_eq!(*host.asked.lock().unwrap(), 0);
    });
}

#[test]
fn writes_create_freely_but_replace_only_what_was_read() {
    serially(async {
        let (_home, _home_scope, tools, root) = project();
        let host = Recorder::deciding(&[Decision::Approved, Decision::Approved]);
        let created = tools
            .call(
                &host,
                "write",
                &json!({"path": "docs/NOTES.md", "text": "# Notes\n"}),
            )
            .await;
        assert!(created.is_error, "a missing parent folder is refused");
        let created = tools
            .call(
                &host,
                "write",
                &json!({"path": "NOTES.md", "text": "# Notes\n"}),
            )
            .await;
        assert_eq!(created.content, "Changed NOTES.md (+1 −0).");
        let overwrite = tools
            .call(
                &host,
                "write",
                &json!({"path": "src/lib.rs", "text": "x\n"}),
            )
            .await;
        assert!(overwrite.is_error && overwrite.content.contains("Read it before replacing it"));
        let outside = tools
            .call(&host, "write", &json!({"path": "/etc/hosts", "text": "x"}))
            .await;
        assert!(outside.is_error && outside.content.contains("not available to Work"));
        let escape = tools
            .call(&host, "read", &json!({"path": "../secret"}))
            .await;
        assert!(escape.is_error);
        assert_eq!(
            std::fs::read_to_string(root.join("NOTES.md")).unwrap(),
            "# Notes\n"
        );
    });
}

#[test]
fn finding_files_respects_gitignore() {
    serially(async {
        let (_home, _home_scope, tools, _root) = project();
        let host = Recorder::default();
        let found = tools.call(&host, "glob", &json!({"pattern": "*.rs"})).await;
        assert_eq!(found.content, "src/lib.rs");
        let lines = tools
            .call(
                &host,
                "grep",
                &json!({"pattern": "fn \\w+", "output": "lines"}),
            )
            .await;
        assert!(
            lines.content.contains("src/lib.rs:1:pub fn add"),
            "{}",
            lines.content
        );
        assert!(!lines.content.contains("target"));
        let none = tools
            .call(&host, "grep", &json!({"pattern": "nothing here"}))
            .await;
        assert_eq!(none.content, "No matches.");
        let bad = tools
            .call(&host, "grep", &json!({"pattern": "(unclosed"}))
            .await;
        assert!(bad.is_error);
    });
}

#[test]
#[ignore = "Requires the currently unavailable confined Computer writer or runner; retained for qualification"]
fn commands_keep_their_approval_classes() {
    serially(async {
        let (_home, _home_scope, tools, root) = project();
        let host = Recorder::deciding(&[Decision::Declined]);
        let listed = tools
            .call(&host, "bash", &json!({"command": "ls src"}))
            .await;
        assert!(
            listed.content.starts_with("exit 0 · "),
            "{}",
            listed.content
        );
        assert!(listed.content.contains("lib.rs"));
        assert_eq!(
            *host.asked.lock().unwrap(),
            0,
            "reading commands run at once"
        );
        let removed = tools
            .call(&host, "bash", &json!({"command": "rm src/lib.rs"}))
            .await;
        assert!(removed.is_error && removed.content.contains("declined"));
        assert!(root.join("src/lib.rs").exists());
        let multi = tools
            .call(&host, "bash", &json!({"command": "ls\nrm -rf ."}))
            .await;
        assert!(multi.is_error && multi.content.contains("one line"));
        let begun = host.begun.lock().unwrap();
        for (kind, local) in begun.iter() {
            let WorkStepKindV1::RunCommand { command, cwd, .. } = kind else {
                continue;
            };
            let policy = local.as_ref().unwrap().policy.as_ref().unwrap();
            let roots = [std::fs::canonicalize(&root).unwrap()];
            let expected =
                crate::work_commands::policy::classify(command, std::path::Path::new(cwd), &roots);
            assert_eq!((policy.class, policy.reason), expected, "{command}");
        }
    });
}

#[test]
#[ignore = "Requires the currently unavailable confined Computer writer or runner; retained for qualification"]
fn test_output_is_summarised() {
    serially(async {
        let (_home, _home_scope, tools, _root) = project();
        let host = Recorder::deciding(&[Decision::Approved]);
        #[cfg(windows)]
        let command = "Write-Output 'running 2 tests'; Write-Output 'test a ... ok'; Write-Output 'test b ... FAILED'; Write-Output 'test result: FAILED. 1 passed; 1 failed; 0 ignored'";
        #[cfg(not(windows))]
        let command = "printf 'running 2 tests\\ntest a ... ok\\ntest b ... FAILED\\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\\n'";
        let ran = tools
            .call(&host, "bash", &json!({"command": command}))
            .await;
        assert!(
            ran.content.contains("tests: 1 passed, 1 failed (b)"),
            "{}",
            ran.content
        );
        assert_eq!(tools.tests().unwrap().failed, 1);
        assert_eq!(
            host.settled.lock().unwrap().last().unwrap().1.as_deref(),
            Some("1 failed")
        );
    });
}

#[test]
fn unavailable_capabilities_refuse_before_approval_and_keep_reads_useful() {
    serially(async {
        let (_home, _home_scope, tools, root) = project();
        let host = Recorder::deciding(&[Decision::Approved]);
        for (name, args) in [
            ("bash", json!({"command": "touch escaped"})),
            ("delegate", json!({"goal": "touch escaped"})),
        ] {
            let result = tools.call(&host, name, &args).await;
            assert!(
                result.is_error && result.content.contains("unavailable"),
                "{}",
                result.content
            );
        }
        assert_eq!(*host.asked.lock().unwrap(), 0);
        assert!(host.begun.lock().unwrap().is_empty());
        assert!(!root.join("escaped").exists());
        let result = tools
            .call(&host, "read", &json!({"path": "src/lib.rs"}))
            .await;
        assert!(!result.is_error && result.content.contains("a + 2"));
        std::fs::write(root.join(".env"), "fixture-private").unwrap();
        std::fs::hard_link(root.join(".env"), root.join("notes.txt")).unwrap();
        for path in [".env", "notes.txt"] {
            let result = tools.call(&host, "read", &json!({"path": path})).await;
            assert!(result.is_error && !result.content.contains("fixture-private"));
        }
        let result = tools
            .call(
                &host,
                "grep",
                &json!({"pattern": "fixture-private", "output": "lines"}),
            )
            .await;
        assert!(!result.content.contains("fixture-private"));
    });
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Native qualified Seatbelt review; inert approved file fixtures only"]
fn reviews_only_actual_approved_before_after_edits() {
    serially(async {
        crate::work_commands::runner::ensure_available().unwrap();
        let (_home, _home_scope, tools, root) = project();
        let host =
            Recorder::deciding(&[Decision::Approved, Decision::Approved, Decision::Approved]);
        let empty = tools
            .call(&host, "review_changes", &json!({"operation":"diff"}))
            .await;
        assert!(!empty.is_error && empty.content.contains("No approved edits"));
        assert!(host.kinds().is_empty());
        assert!(
            !tools
                .call(&host, "read", &json!({"path":"src/lib.rs"}))
                .await
                .is_error
        );
        assert!(
            !tools
                .call(
                    &host,
                    "edit",
                    &json!({"path":"src/lib.rs", "old":"    a + 2\n}", "new":"    a + 1  \n}"})
                )
                .await
                .is_error
        );
        assert!(
            !tools
                .call(
                    &host,
                    "write",
                    &json!({"path":"new.txt", "text":"new approved file\n"})
                )
                .await
                .is_error
        );
        let before = std::fs::read(root.join("src/lib.rs")).unwrap();
        let patch = tools
            .call(&host, "review_changes", &json!({"operation":"diff"}))
            .await;
        assert!(!patch.is_error, "{}", patch.content);
        assert!(patch.content.contains("-    a + 2") && patch.content.contains("+    a + 1"));
        assert!(patch.content.contains("+new approved file"));
        assert!(
            patch.content.contains("copied workspace")
                && patch.content.contains("not original repository")
        );
        let check = tools
            .call(&host, "review_changes", &json!({"operation":"check"}))
            .await;
        assert!(
            check.is_error && check.content.contains("trailing whitespace"),
            "{}",
            check.content
        );
        assert_eq!(std::fs::read(root.join("src/lib.rs")).unwrap(), before);
        assert_eq!(
            *host.asked.lock().unwrap(),
            2,
            "read-only review adds no mutation approval"
        );
        std::fs::write(root.join("src/lib.rs"), "later external edit\n").unwrap();
        let stale = tools
            .call(&host, "review_changes", &json!({"operation":"diff"}))
            .await;
        assert!(stale.is_error && stale.content.contains("changed again"));
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "later external edit\n"
        );
        std::fs::write(root.join("src/lib.rs"), &before).unwrap();
        let path = root.join("src/lib.rs");
        let mut checks = 0;
        *host.on_cancel_check.lock().unwrap() = Some(Box::new(move || {
            checks += 1;
            if checks == 2 {
                std::fs::write(&path, "concurrent external edit\n").unwrap();
            }
            false
        }));
        let raced = tools
            .call(&host, "review_changes", &json!({"operation":"diff"}))
            .await;
        assert!(raced.is_error && raced.content.contains("changed again"));
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "concurrent external edit\n"
        );
        *host.on_cancel_check.lock().unwrap() = None;
        std::fs::write(root.join("src/lib.rs"), &before).unwrap();
        assert!(!tools.call(&host, "write", &json!({"path":".gitattributes", "text":"* -diff -whitespace filter=untrusted working-tree-encoding=UTF-16\n"})).await.is_error);
        let attributes = tools
            .call(&host, "review_changes", &json!({"operation":"diff"}))
            .await;
        assert!(attributes.is_error && attributes.content.contains("copied safely"));
        assert_eq!(*host.asked.lock().unwrap(), 3);
    });
}

#[cfg(target_os = "macos")]
#[test]
fn recovery_tools_require_approval_and_restore_without_overwrite() {
    serially(async {
        let (_home, _home_scope, tools, root) = project();
        let storage = tempfile::Builder::new()
            .prefix("work-recovery-app-")
            .tempdir_in("/private/tmp")
            .unwrap();
        crate::work_files::trash::install_root(storage.path().canonicalize().unwrap());
        let file = root.join("trash-fixture.txt");
        std::fs::write(&file, "approved text").unwrap();
        let host = Recorder::deciding(&[Decision::Declined, Decision::Stopped, Decision::Approved]);
        if !crate::work_commands::runner::native_recovery_qualified() {
            // Recovery opens only on a macOS build whose native fixtures were
            // reviewed; anywhere else Trash is refused before any approval.
            assert!(
                !tools
                    .call(&host, "read", &json!({"path":"trash-fixture.txt"}))
                    .await
                    .is_error
            );
            let refused = tools
                .call(&host, "trash", &json!({"path":"trash-fixture.txt"}))
                .await;
            assert!(
                refused.is_error && refused.content.contains("not available"),
                "{}",
                refused.content
            );
            assert_eq!(*host.asked.lock().unwrap(), 0);
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "approved text");
            return;
        }
        let unread = tools
            .call(&host, "trash", &json!({"path":"trash-fixture.txt"}))
            .await;
        assert!(unread.is_error);
        assert_eq!(*host.asked.lock().unwrap(), 0);
        assert!(
            !tools
                .call(&host, "read", &json!({"path":"trash-fixture.txt"}))
                .await
                .is_error
        );
        assert!(
            tools
                .call(&host, "trash", &json!({"path":"trash-fixture.txt"}))
                .await
                .is_error
        );
        assert!(
            tools
                .call(&host, "trash", &json!({"path":"trash-fixture.txt"}))
                .await
                .is_error
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "approved text");
        assert_eq!(*host.asked.lock().unwrap(), 2);
        assert!(host
            .settled
            .lock()
            .unwrap()
            .iter()
            .any(|(status, _)| *status == WorkStepStatus::Cancelled));
        // Hold the sole blocking lane, submit an approved Trash request, then
        // drop its future before the worker can begin. No delayed removal may run.
        let (release, held) = std::sync::mpsc::channel::<()>();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = held.recv();
        });
        let cancelling_host = Recorder::deciding(&[Decision::Approved]);
        let request = json!({"path":"trash-fixture.txt"});
        let mut queued = Box::pin(tools.call(&cancelling_host, "trash", &request));
        tokio::select! {
            _ = &mut queued => panic!("the blocking lane is held"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(30)) => {}
        }
        assert_eq!(*cancelling_host.asked.lock().unwrap(), 1);
        drop(queued);
        drop(release);
        blocker.await.unwrap();
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "approved text");
        assert!(!storage.path().join(".zephium-work-recovery").exists());

        let restored = root
            .join("restored-fixture.txt")
            .canonicalize()
            .unwrap_or_else(|_| root.canonicalize().unwrap().join("restored-fixture.txt"));
        let receipt = crate::work_files::trash::seed_recovery_fixture(
            storage.path(),
            "computer-test-profile",
            restored.clone(),
            "saved fixture",
        );
        let rows = tools.call(&host, "recoveries", &json!({})).await;
        assert!(!rows.is_error && rows.content.contains(&receipt));
        let result = tools
            .call(&host, "restore_saved", &json!({"receipt":receipt}))
            .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(std::fs::read_to_string(&restored).unwrap(), "saved fixture");
        assert_eq!(*host.asked.lock().unwrap(), 3);
        std::fs::write(&restored, "newer text").unwrap();
        assert!(
            tools
                .call(&host, "restore_saved", &json!({"receipt":receipt}))
                .await
                .is_error
        );
        assert_eq!(std::fs::read_to_string(&restored).unwrap(), "newer text");
        assert_eq!(*host.asked.lock().unwrap(), 3);

        // Move uses the same approval and cancel-on-drop boundary. No queued
        // worker may move a file after its calling future has been dropped.
        let moved_source = root.join("move-fixture.txt");
        std::fs::write(&moved_source, "inert move text").unwrap();
        let move_host = Recorder::deciding(&[Decision::Declined, Decision::Approved]);
        let move_args = json!({"from":"move-fixture.txt","to":"moved-fixture.txt"});
        assert!(
            tools
                .call(&move_host, "move_file", &move_args)
                .await
                .is_error
        );
        assert_eq!(*move_host.asked.lock().unwrap(), 0);
        assert!(
            !tools
                .call(&move_host, "read", &json!({"path":"move-fixture.txt"}))
                .await
                .is_error
        );
        assert!(
            tools
                .call(&move_host, "move_file", &move_args)
                .await
                .is_error
        );
        assert_eq!(*move_host.asked.lock().unwrap(), 1);
        assert!(matches!(
            move_host.kinds().last(),
            Some(WorkStepKindV1::MoveFile { .. })
        ));
        let (release, held) = std::sync::mpsc::channel::<()>();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = held.recv();
        });
        let mut queued = Box::pin(tools.call(&move_host, "move_file", &move_args));
        tokio::select! {
            _ = &mut queued => panic!("the blocking lane is held"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(30)) => {}
        }
        assert_eq!(*move_host.asked.lock().unwrap(), 2);
        drop(queued);
        drop(release);
        blocker.await.unwrap();
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&moved_source).unwrap(),
            "inert move text"
        );
        assert!(!root.join("moved-fixture.txt").exists());
    });
}

#[test]
fn recognizable_secret_arguments_are_rejected_before_recording() {
    serially(async {
        let (_home, _home_scope, tools, _) = project();
        let host = Recorder::default();
        for (name, args) in [
            (
                "write",
                json!({"path":"new.txt","text":"ghp_inertFixtureToken"}),
            ),
            ("read", json!({"path":"ghp_inertFixtureFilename.txt"})),
            ("grep", json!({"pattern":"-----BEGIN PRIVATE KEY-----"})),
        ] {
            let reply = tools.call(&host, name, &args).await;
            assert!(reply.is_error);
            assert!(!reply.content.contains("ghp_"));
            assert!(!reply.content.contains("-----BEGIN"));
        }
        assert!(host.kinds().is_empty());
        assert!(host.settled.lock().unwrap().is_empty());
        assert_eq!(*host.asked.lock().unwrap(), 0);
    });
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "Native qualified current-index inspection over an inert repository"]
fn current_index_tool_records_scoped_diff_and_honest_failures() {
    serially(async {
        use sha1::{Digest, Sha1};
        use std::io::Write;
        let (_home, _home_scope, tools, root) = project();
        let path = root.join("src/lib.rs");
        let before = std::fs::read(&path).unwrap();
        let mut object = format!("blob {}\0", before.len()).into_bytes();
        object.extend(&before);
        let oid: [u8; 20] = Sha1::digest(&object).into();
        let hex = oid
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let directory = root.join(".git/objects").join(&hex[..2]);
        std::fs::create_dir_all(&directory).unwrap();
        let mut compressed =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        compressed.write_all(&object).unwrap();
        std::fs::write(directory.join(&hex[2..]), compressed.finish().unwrap()).unwrap();
        let mut index = b"DIRC".to_vec();
        index.extend(2u32.to_be_bytes());
        index.extend(1u32.to_be_bytes());
        let start = index.len();
        let mut fixed = [0; 40];
        fixed[24..28].copy_from_slice(&0o100644u32.to_be_bytes());
        index.extend(fixed);
        index.extend(oid);
        index.extend(10u16.to_be_bytes());
        index.extend(b"src/lib.rs\0");
        while !(index.len() - start).is_multiple_of(8) {
            index.push(0);
        }
        index.extend(Sha1::digest(&index));
        std::fs::write(root.join(".git/index"), index).unwrap();
        std::fs::write(root.join(".git/config"), "not valid Git configuration [").unwrap();
        std::fs::write(&path, "pub fn changed() {}\n").unwrap();
        let host = Recorder::default();
        let args = json!({"paths":["src/lib.rs"],"operation":"diff"});
        let reply = tools.call(&host, "git_changes", &args).await;
        assert!(!reply.is_error, "{}", reply.content);
        assert!(reply.content.contains("working-tree") || reply.content.contains("Working-tree"));
        assert!(reply.content.contains("current index"));
        assert!(reply.content.contains("+pub fn changed() {}"));
        assert!(reply.content.contains("Excludes staged-versus-HEAD"));
        assert_eq!(*host.asked.lock().unwrap(), 0);
        assert_eq!(host.settled.lock().unwrap()[0].0, WorkStepStatus::Succeeded);
        let reply = tools
            .call(
                &host,
                "git_changes",
                &json!({"paths":[".env"],"operation":"diff"}),
            )
            .await;
        assert!(reply.is_error);
        assert_eq!(host.settled.lock().unwrap()[1].0, WorkStepStatus::Failed);
        *host.on_cancel_check.lock().unwrap() = Some(Box::new(|| true));
        let reply = tools.call(&host, "git_changes", &args).await;
        assert!(reply.is_error);
        assert_eq!(host.settled.lock().unwrap()[2].0, WorkStepStatus::Cancelled);
    });
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "Native qualified Python tool receipts and held-output privacy; inert sources only"]
fn selected_python_tests_record_truthful_outcomes_without_live_output() {
    serially(async {
        let (_home, _home_scope, tools, root) = project();
        let host = Recorder::default();
        let path = root.join("test_inert.py");
        let args = json!({"tests":["test_inert.py"]});
        std::fs::write(&path,"import unittest\nclass Inert(unittest.TestCase):\n def test_value(self): self.assertEqual(2+2,4)\n").unwrap();
        let result = tools.call(&host, "python_tests", &args).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("1 passed"));
        assert_eq!(host.settled.lock().unwrap()[0].0, WorkStepStatus::Succeeded);
        std::fs::write(&path,"import unittest\nclass Inert(unittest.TestCase):\n def test_value(self): self.assertEqual(2+2,5)\n").unwrap();
        let result = tools.call(&host, "python_tests", &args).await;
        assert!(result.is_error && result.content.contains("1 failed"));
        assert_eq!(host.settled.lock().unwrap()[1].0, WorkStepStatus::Failed);
        assert_eq!(*host.evidence_count.lock().unwrap(), 2);
        std::fs::write(&path,"import sys,time,unittest\nclass Inert(unittest.TestCase):\n def test_output(self):\n  sys.stdout.write('ghp_');sys.stdout.flush();time.sleep(.01);sys.stdout.write('inertFixtureToken');sys.stdout.flush()\n").unwrap();
        let result = tools.call(&host, "python_tests", &args).await;
        assert!(result.is_error && !result.content.contains("ghp_"));
        assert_eq!(host.settled.lock().unwrap()[2].0, WorkStepStatus::Failed);
        assert_eq!(*host.evidence_count.lock().unwrap(), 2);
        std::fs::write(&path, "# No test cases\n").unwrap();
        let result = tools.call(&host, "python_tests", &args).await;
        assert!(result.is_error && result.content.contains("No tests discovered"));
        assert_eq!(host.settled.lock().unwrap()[3].0, WorkStepStatus::Failed);
        *host.on_cancel_check.lock().unwrap() = Some(Box::new(|| true));
        let result = tools.call(&host, "python_tests", &args).await;
        assert!(result.is_error);
        assert_eq!(host.settled.lock().unwrap()[4].0, WorkStepStatus::Cancelled);
        assert_eq!(*host.progress_calls.lock().unwrap(), 0);
        assert_eq!(*host.asked.lock().unwrap(), 0);
    });
}
