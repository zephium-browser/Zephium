use super::*;
use std::path::{Path, PathBuf};

#[cfg(windows)]
const WRITE_FIRST: &str =
    "Set-Content -LiteralPath result.txt -Value first -NoNewline -Encoding ascii";
#[cfg(not(windows))]
const WRITE_FIRST: &str = "printf first > result.txt";
#[cfg(windows)]
const APPEND_SECOND: &str =
    "Add-Content -LiteralPath result.txt -Value second -NoNewline -Encoding ascii";
#[cfg(not(windows))]
const APPEND_SECOND: &str = "printf second >> result.txt";
#[cfg(windows)]
const REMOVE_RESULT: &str = "Remove-Item -LiteralPath result.txt";
#[cfg(not(windows))]
const REMOVE_RESULT: &str = "rm result.txt";
#[cfg(windows)]
const SLOW_COMMAND: &str = "[Console]::Write('started'); Start-Sleep -Seconds 30";
#[cfg(not(windows))]
const SLOW_COMMAND: &str = "printf started; trap '' TERM; sleep 30 & wait";
#[cfg(windows)]
const QUIT_COMMAND: &str = "[Console]::Write('started'); Start-Sleep -Seconds 30; Set-Content -LiteralPath replayed -Value replayed";
#[cfg(not(windows))]
const QUIT_COMMAND: &str = "printf started; sleep 30; touch replayed";

struct Home {
    dir: tempfile::TempDir,
    _grant_home: crate::work_files::TestGrantHome,
}
impl Home {
    fn new() -> Self {
        let dir = crate::work_files::test_tempdir();
        let grant_home = crate::work_files::TestGrantHome::new(dir.path().to_path_buf());
        std::fs::create_dir(dir.path().join("project")).unwrap();
        Self {
            dir,
            _grant_home: grant_home,
        }
    }
    fn project(&self) -> PathBuf {
        self.dir.path().join("project").canonicalize().unwrap()
    }
}
fn local_begin(work: WorkId, expected: WorkRevision, root: &Path) -> WorkCommandV1 {
    let mut command = begin(work, expected, 60);
    if let WorkRuntimeIntent::BeginAgent { grant, .. } = &mut command.intent {
        grant.folders = vec![root.to_string_lossy().into_owned()];
    }
    command
}
fn run(root: &Path, command: &str, timeout: Option<u32>) -> WorkAgentFetch {
    WorkAgentFetch::RunCommand {
        cwd: root.to_string_lossy().into_owned(),
        command: command.into(),
        timeout_secs: timeout,
    }
}
fn finish_local() -> WorkAgentTurnOutput {
    let mut result = finishing();
    result.artifacts[0].evidence = vec![0];
    result
}

#[test]
#[ignore = "Requires the currently closed native writer/command confinement gate; retained for qualification"]
fn work_local_file_and_command_approval_journal() {
    let _serial = crate::WORK_RUNTIME_TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(work_local_file_and_command_approval_journal_inner());
}

async fn work_local_file_and_command_approval_journal_inner() {
    let home = Home::new();
    let root = home.project();
    let file = root.join("version.txt").to_string_lossy().into_owned();
    let moved = root.join("renamed.txt").to_string_lossy().into_owned();
    std::fs::write(&file, "version=1\nname=demo\n").unwrap();
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let work = new_work(
        &mut shell,
        &queue,
        &handle,
        "Update the local fixture and test it",
    )
    .await;
    let script = Script::default();
    script.play([
        output(vec![
            WorkAgentFetch::ReadFile {
                path: file.clone(),
                offset: Some(1),
                limit: Some(2),
            },
            WorkAgentFetch::List {
                path: root.to_string_lossy().into_owned(),
                depth: Some(3),
            },
            WorkAgentFetch::SearchFiles {
                path: root.to_string_lossy().into_owned(),
                query: "version=[0-9]".into(),
                glob: Some("*.txt".into()),
                regex: Some(true),
            },
            run(&root, "pwd", None),
        ]),
        output(vec![WorkAgentFetch::EditFile {
            path: file.clone(),
            old: String::new(),
            new: String::new(),
            replacements: vec![
                WorkFileReplacementV1 {
                    old: "version=1".into(),
                    new: "version=2".into(),
                },
                WorkFileReplacementV1 {
                    old: "name=demo".into(),
                    new: "name=fixture".into(),
                },
            ],
        }]),
        output(vec![WorkAgentFetch::MoveFile {
            from: file.clone(),
            to: moved.clone(),
        }]),
        output(vec![WorkAgentFetch::DeleteFile {
            path: moved.clone(),
        }]),
        output(vec![run(&root, WRITE_FIRST, None)]),
        output(vec![
            run(&root, APPEND_SECOND, None),
            run(&root, REMOVE_RESULT, None),
        ]),
        finish_local(),
    ]);
    let service = WorkAgentService::new(handle.clone());
    let sources = Sources::default();
    let decisions = async {
        let mut count = 0;
        let started = Instant::now();
        while count < 5 {
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "approval journal did not settle"
            );
            let state = projection(&handle, profile, work).await;
            if let Some(execution) = state.executions.last() {
                if let Some(step) = execution.steps.iter().find(|s| {
                    s.status == WorkStepStatus::Running
                        && s.kind.proposes_write()
                        && s.kind.file_decision().is_none()
                        && (!matches!(s.kind, WorkStepKindV1::RunCommand { .. })
                            || s.local
                                .as_ref()
                                .and_then(|l| l.policy.as_ref())
                                .is_some_and(|p| p.scope != WorkCommandApprovalScopeV1::None))
                }) {
                    let approve = !matches!(&step.kind,WorkStepKindV1::RunCommand{command,..} if command == REMOVE_RESULT);
                    if let WorkStepKindV1::EditFile { .. } = step.kind {
                        assert_eq!(
                            std::fs::read_to_string(&file).unwrap(),
                            "version=1\nname=demo\n"
                        );
                        assert!(step
                            .local
                            .as_ref()
                            .unwrap()
                            .proposal
                            .as_ref()
                            .unwrap()
                            .contains("version=2"));
                    }
                    command(
                        &handle,
                        profile,
                        work,
                        WorkRuntimeIntent::ApproveStep {
                            execution: execution.id,
                            step: step.id,
                            approve,
                            for_run: false,
                        },
                    )
                    .await;
                    count += 1;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let (result, ()) = drive(&mut shell, &queue, async {
        tokio::join!(
            service.run(
                profile,
                local_begin(work, WorkRevision::INITIAL, &root),
                None,
                WorkAgentProviders {
                    turn: &script,
                    search: &sources
                },
                |probe, request| page(probe, request, false),
                |_| {}
            ),
            decisions
        )
    })
    .await;
    let state = result.unwrap();
    let execution = &state.executions[0];
    assert_eq!(execution.status, WorkExecutionStatus::NeedsReview);
    assert_eq!(execution.folder_approvals.len(), 1);
    assert_eq!(execution.command_evidence.len(), 3);
    assert_eq!(execution.file_evidence.len(), 6);
    assert_eq!(
        std::fs::read_to_string(root.join("result.txt")).unwrap(),
        "firstsecond"
    );
    assert!(!Path::new(&moved).exists());
    assert!(execution
        .steps
        .iter()
        .any(|s| s.note.as_deref() == Some("Declined") && s.status == WorkStepStatus::Failed));
    assert!(execution
        .file_evidence
        .iter()
        .filter(|r| matches!(
            r.file.kind,
            WorkFileKindV1::Written | WorkFileKindV1::Moved | WorkFileKindV1::Deleted
        ))
        .all(|r| r.file.before_digest.is_some()));
    execution
        .validate(state.work.plan.as_ref().unwrap(), state.work.revision)
        .unwrap();
}

#[test]
#[ignore = "Requires the currently closed native writer/command confinement gate; retained for qualification"]
fn work_local_command_timeout_and_stop_settle_with_output() {
    let _serial = crate::WORK_RUNTIME_TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(work_local_command_timeout_and_stop_settle_with_output_inner());
}

async fn work_local_command_timeout_and_stop_settle_with_output_inner() {
    let timeout_secs = if cfg!(windows) { 5 } else { 1 };
    let home = Home::new();
    let root = home.project();
    for stop in [false, true] {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let work = new_work(&mut shell, &queue, &handle, "Run the fixture command").await;
        let script = Script::default();
        script.play([
            output(vec![run(
                &root,
                SLOW_COMMAND,
                Some(if stop { 30 } else { timeout_secs }),
            )]),
            finish_local(),
        ]);
        let sources = Sources::default();
        let service = WorkAgentService::new(handle.clone());
        let decide = async {
            let (execution, step) = tokio::time::timeout(
                Duration::from_secs(10),
                running_step(&handle, profile, work, |k| {
                    matches!(k, WorkStepKindV1::RunCommand { .. })
                }),
            )
            .await
            .expect("command step did not start");
            command(
                &handle,
                profile,
                work,
                WorkRuntimeIntent::ApproveStep {
                    execution,
                    step,
                    approve: true,
                    for_run: false,
                },
            )
            .await;
            if stop {
                let waiting = Instant::now();
                loop {
                    assert!(
                        waiting.elapsed() < Duration::from_secs(10),
                        "command output did not arrive"
                    );
                    let state = projection(&handle, profile, work).await;
                    if state.executions[0].steps.iter().any(|s| {
                        s.local
                            .as_ref()
                            .and_then(|l| l.output.as_ref())
                            .is_some_and(|o| o.text.contains("started"))
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                let started = Instant::now();
                command(
                    &handle,
                    profile,
                    work,
                    WorkRuntimeIntent::Cancel {
                        execution,
                        intervention: None,
                    },
                )
                .await;
                loop {
                    let state = projection(&handle, profile, work).await;
                    if state.executions[0]
                        .steps
                        .iter()
                        .find(|s| s.id == step)
                        .unwrap()
                        .status
                        != WorkStepStatus::Running
                    {
                        break;
                    }
                    assert!(started.elapsed() < Duration::from_secs(3));
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        };
        let (result, ()) = drive(&mut shell, &queue, async {
            tokio::join!(
                service.run(
                    profile,
                    local_begin(work, WorkRevision::INITIAL, &root),
                    None,
                    WorkAgentProviders {
                        turn: &script,
                        search: &sources
                    },
                    |probe, request| page(probe, request, false),
                    |_| {}
                ),
                decide
            )
        })
        .await;
        let state = result.unwrap();
        let execution = &state.executions[0];
        let step = execution
            .steps
            .iter()
            .find(|s| matches!(s.kind, WorkStepKindV1::RunCommand { .. }))
            .unwrap();
        assert_eq!(step.status, WorkStepStatus::Failed);
        let note = if stop {
            "Stopped".into()
        } else {
            format!("Stopped after {timeout_secs} s")
        };
        assert_eq!(step.note.as_deref(), Some(note.as_str()));
        assert!(execution.command_evidence[0]
            .command
            .text
            .contains("started"));
    }
}

#[test]
#[ignore = "Requires the currently closed native writer/command confinement gate; retained for qualification"]
fn work_local_quit_recovers_command_as_failed_without_replay() {
    let _serial = crate::WORK_RUNTIME_TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(work_local_quit_recovers_command_as_failed_without_replay_inner());
}

async fn work_local_quit_recovers_command_as_failed_without_replay_inner() {
    let home = Home::new();
    let root = home.project();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, profile) = fixture(store.clone());
    let work = new_work(&mut shell, &queue, &handle, "Run a local command").await;
    let script = Script::default();
    script.play([output(vec![run(&root, QUIT_COMMAND, None)])]);
    let sources = Sources::default();
    let service = WorkAgentService::new(handle.clone());
    drive(&mut shell,&queue,async{
        tokio::select! {
            _=service.run(profile,local_begin(work,WorkRevision::INITIAL,&root),None,WorkAgentProviders{turn:&script,search:&sources},|probe,request|page(probe,request,false),|_|{})=>panic!("command should still be running"),
            _=async{
                let (execution,step)=tokio::time::timeout(Duration::from_secs(10), running_step(&handle,profile,work,|k|matches!(k,WorkStepKindV1::RunCommand{..}))).await.expect("command step did not start");command(&handle,profile,work,WorkRuntimeIntent::ApproveStep{execution,step,approve:true,for_run:false}).await;
                let waiting=Instant::now();
                loop {assert!(waiting.elapsed()<Duration::from_secs(10), "command output did not arrive"); let state=projection(&handle,profile,work).await;if state.executions[0].steps.iter().any(|s|s.local.as_ref().and_then(|l|l.output.as_ref()).is_some_and(|o|o.text.contains("started"))){break;}tokio::time::sleep(Duration::from_millis(20)).await;}
            }=>{}
        }
    }).await;
    drop(service);
    drop(handle);
    drop(queue);
    drop(shell);
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        zephium_core::ports::store::StoreShutdownOutcome::Clean
    );
    drop(store);
    let store = Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, _) = fixture(store);
    let state = drive(&mut shell, &queue, projection(&handle, profile, work)).await;
    let execution = state.executions[0].id;
    drive(
        &mut shell,
        &queue,
        command(
            &handle,
            profile,
            work,
            WorkRuntimeIntent::AcknowledgeInterruption { execution },
        ),
    )
    .await;
    let state = drive(&mut shell, &queue, projection(&handle, profile, work)).await;
    let step = state.executions[0]
        .steps
        .iter()
        .find(|s| matches!(s.kind, WorkStepKindV1::RunCommand { .. }))
        .unwrap();
    assert_eq!(step.status, WorkStepStatus::Failed);
    assert_eq!(step.note.as_deref(), Some("Stopped when the app quit"));
    assert!(!root.join("replayed").exists());
}

#[test]
fn work_local_inspection_survives_while_unsafe_capabilities_fail_without_approval() {
    let _serial = crate::WORK_RUNTIME_TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let home = Home::new();
            let root = home.project();
            let file = root.join("version.txt").to_string_lossy().into_owned();
            let moved = root.join("moved.txt").to_string_lossy().into_owned();
            std::fs::write(&file, "version=1\n").unwrap();
            std::fs::write(root.join(".env"), "fixture-secret-value\n").unwrap();
            let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
            let (mut shell, queue, handle, profile) = fixture(store);
            let work = new_work(&mut shell, &queue, &handle, "Inspect the local fixture").await;
            let script = Script::default();
            script.play([
                output(vec![
                    WorkAgentFetch::ReadFile {
                        path: file.clone(),
                        offset: None,
                        limit: None,
                    },
                    WorkAgentFetch::ReadFile {
                        path: root.join(".env").to_string_lossy().into_owned(),
                        offset: None,
                        limit: None,
                    },
                    WorkAgentFetch::List {
                        path: root.to_string_lossy().into_owned(),
                        depth: Some(1),
                    },
                    WorkAgentFetch::SearchFiles {
                        path: root.to_string_lossy().into_owned(),
                        query: "version".into(),
                        glob: None,
                        regex: None,
                    },
                ]),
                output(vec![
                    WorkAgentFetch::SearchFiles {
                        path: root.to_string_lossy().into_owned(),
                        query: "fixture-secret".into(),
                        glob: None,
                        regex: None,
                    },
                    WorkAgentFetch::MoveFile {
                        from: file.clone(),
                        to: moved.clone(),
                    },
                ]),
                output(vec![
                    WorkAgentFetch::DeleteFile { path: file.clone() },
                    run(&root, WRITE_FIRST, None),
                ]),
                finish_local(),
            ]);
            let service = WorkAgentService::new(handle.clone());
            let sources = Sources::default();
            // No approval task runs: all unavailable operations must settle by themselves.
            let result = drive(&mut shell, &queue, async {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    service.run(
                        profile,
                        local_begin(work, WorkRevision::INITIAL, &root),
                        None,
                        WorkAgentProviders {
                            turn: &script,
                            search: &sources,
                        },
                        |probe, request| page(probe, request, false),
                        |_| {},
                    ),
                )
                .await
                .expect("an unavailable operation waited for approval")
            })
            .await
            .unwrap();
            let execution = &result.executions[0];
            assert!(execution.folder_approvals.is_empty());
            assert!(execution.command_evidence.is_empty());
            let rejected: Vec<_> = execution
                .steps
                .iter()
                .filter(|step| step.kind.proposes_write())
                .collect();
            assert_eq!(
                rejected.len(),
                3,
                "steps: {:?}; notices: {:?}",
                execution
                    .steps
                    .iter()
                    .map(|step| (&step.kind, &step.note))
                    .collect::<Vec<_>>(),
                script
                    .seen
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|seen| &seen.notices)
                    .collect::<Vec<_>>()
            );
            for step in rejected {
                assert_eq!(step.status, WorkStepStatus::Failed);
                assert!(
                    step.note
                        .as_deref()
                        .is_some_and(|note| note.contains("unavailable")),
                    "{:?}",
                    step.note
                );
                assert!(step.kind.file_decision().is_none());
                assert!(step
                    .local
                    .as_ref()
                    .and_then(|local| local.proposal.as_ref())
                    .is_none());
            }
            assert!(execution
                .file_evidence
                .iter()
                .any(|record| record.file.kind == WorkFileKindV1::Text
                    && record.file.text.contains("version=1")));
            assert!(execution
                .file_evidence
                .iter()
                .any(|record| record.file.kind == WorkFileKindV1::Directory
                    && record.file.text.contains("version.txt")));
            assert!(
                execution
                    .file_evidence
                    .iter()
                    .any(|record| record.file.kind == WorkFileKindV1::Search
                        && record.file.bytes == 1)
            );
            assert!(execution
                .file_evidence
                .iter()
                .any(|record| record.file.kind == WorkFileKindV1::Search
                    && record.file.bytes == 0
                    && record.file.text.is_empty()));
            assert!(!serde_json::to_string(&execution.file_evidence)
                .unwrap()
                .contains("fixture-secret-value"));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "version=1\n");
            assert!(!Path::new(&moved).exists());
            assert!(!root.join("result.txt").exists());
        });
}
