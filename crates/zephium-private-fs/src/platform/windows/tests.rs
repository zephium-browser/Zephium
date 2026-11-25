//! Live Windows validation. These tests must run on local NTFS as an ordinary
//! user; a cross-target `cargo check` only validates their Rust compilation.

use super::*;

#[test]
fn namespace_activation_matches_the_explicit_validation_gate() {
    assert_eq!(
        admit_namespace_support().is_ok(),
        cfg!(feature = "windows-namespace-validation")
    );
    #[cfg(not(feature = "windows-namespace-validation"))]
    assert!(matches!(
        crate::LockedPrivateNamespace::open_or_create("Z:\\missing\\namespace"),
        Err(PrivateFsError::PrimitiveUnavailable)
    ));
}

#[cfg(feature = "windows-namespace-validation")]
mod validation {
    use super::*;
    use crate::{ByteLimit, LockedPrivateNamespace, PrivateComponent, TreeRemovalLimits};
    use std::io::{Read, Seek, Write};
    use tempfile::TempDir;

    fn fixture() -> (TempDir, File, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("namespace");
        assert!(create_private_root(&root).unwrap());
        let (directory, _) = open_directory(&root).unwrap();
        (temp, directory, root)
    }

    fn component(name: &str) -> PrivateComponent {
        PrivateComponent::new(name).unwrap()
    }

    #[test]
    fn directory_reopen_keeps_held_identity_after_ambient_replacement() {
        let (temp, directory, root) = fixture();
        let expected = identity(&directory, Some(true)).unwrap();
        let security = security::snapshot(&directory).unwrap();
        let moved = temp.path().join("moved");
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir(&root).unwrap();
        let reopened = native::reopen_writable(&directory).unwrap();
        assert_eq!(identity(&reopened, Some(true)).unwrap(), expected);
        assert!(security::snapshot(&reopened).unwrap() == security);
        let ambient = absolute_directory(&root, false).unwrap();
        assert_ne!(identity(&ambient, Some(true)).unwrap(), expected);
        drop(create_new_regular(&reopened, &moved, "owned").unwrap());
        assert!(moved.join("owned").is_file());
        assert!(!root.join("owned").exists());
    }

    #[test]
    fn directory_reopen_refuses_sealed_write_without_changing_security() {
        let (_temp, directory, _root) = fixture();
        set_directory_mode(&directory, DirectoryMode::Sealed).unwrap();
        let expected = identity(&directory, Some(true)).unwrap();
        let security = security::snapshot(&directory).unwrap();
        assert!(matches!(
            native::reopen_writable(&directory),
            Err(PrivateFsError::PrimitiveUnavailable)
        ));
        assert_eq!(identity(&directory, Some(true)).unwrap(), expected);
        assert!(security::snapshot(&directory).unwrap() == security);
        assert_eq!(security::mode(&directory), Ok(true));
        set_directory_mode(&directory, DirectoryMode::Writable).unwrap();
        assert_eq!(
            identity(&native::reopen_writable(&directory).unwrap(), Some(true)).unwrap(),
            expected
        );
    }

    #[test]
    fn unlinked_held_file_preserves_settlement_identity_without_new_admission() {
        let (_temp, directory, root) = fixture();
        let (file, expected) = create_new_regular(&directory, &root, "payload").unwrap();
        let (held, observed) =
            open_regular(&directory, &root, "payload", OpenPurpose::Read).unwrap();
        assert_eq!(observed, expected);
        native::delete(&file).unwrap();
        drop(file);
        let file = held;
        assert!(relative_name_is_absent(&directory, "payload"));
        assert_eq!(identity(&file, Some(false)), Err(PrivateFsError::Unsafe));
        assert!(same_open_identity(&file, expected));
        let mut wrong = expected;
        wrong.file_id[0] ^= 1;
        assert!(!same_open_identity(&file, wrong));
    }

    #[test]
    fn uncertain_create_is_clean_only_when_the_exact_name_is_absent() {
        let (_temp, directory, root) = fixture();
        assert_eq!(
            native::classify_create_error(&directory, "missing", PrivateFsError::Io),
            PrivateFsError::Io
        );
        drop(create_new_regular(&directory, &root, "residue").unwrap());
        assert_eq!(
            native::classify_create_error(&directory, "residue", PrivateFsError::Io),
            PrivateFsError::SettlementUnknown
        );
        assert_eq!(
            native::classify_create_error(&directory, "residue", PrivateFsError::AlreadyExists),
            PrivateFsError::AlreadyExists
        );
    }

    #[test]
    fn held_parent_operations_cannot_be_redirected_by_replacing_its_old_path() {
        let (temp, directory, root) = fixture();
        let moved = temp.path().join("moved");
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir(&root).unwrap();
        let (file, _) = create_new_regular(&directory, &root, "payload").unwrap();
        drop(file);
        assert!(moved.join("payload").is_file());
        assert!(!root.join("payload").exists());
    }

    #[test]
    fn directory_enumeration_is_complete_sorted_and_bounded_across_batches() {
        let (_temp, directory, root) = fixture();
        let expected: Vec<_> = (0..300)
            .map(|index| format!("{index:03}-{}", "a".repeat(230)))
            .collect();
        for name in &expected {
            drop(create_new_regular(&directory, &root, name).unwrap());
        }
        assert_eq!(list_names(&directory, &root, 300).unwrap(), expected);
        assert_eq!(
            list_names(&directory, &root, 299),
            Err(PrivateFsError::BoundExceeded)
        );
        assert!(!directory_is_empty(&directory).unwrap());
    }

    #[test]
    fn inherited_unprotected_payload_acl_is_rejected() {
        let (_temp, directory, root) = fixture();
        std::fs::write(root.join("ambient"), b"payload").unwrap();
        assert!(open_regular(&directory, &root, "ambient", OpenPurpose::Read).is_err());
        assert!(inspect_child(&directory, &root, "ambient").is_err());
    }

    #[test]
    #[ignore = "requires Windows Developer Mode symlink permission; mandatory before enabling the adapter"]
    fn reparse_file_directory_and_namespace_root_are_rejected() {
        let (temp, directory, root) = fixture();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("payload"), b"outside").unwrap();
        std::os::windows::fs::symlink_file(outside.join("payload"), root.join("file-link"))
            .unwrap();
        std::os::windows::fs::symlink_dir(&outside, root.join("dir-link")).unwrap();
        assert!(open_regular(&directory, &root, "file-link", OpenPurpose::Read).is_err());
        assert!(open_child_directory(&directory, &root, "dir-link").is_err());
        assert!(inspect_child(&directory, &root, "file-link").is_err());
        assert!(create_new_regular(&directory, &root, "file-link").is_err());
        let alias = temp.path().join("root-link");
        std::os::windows::fs::symlink_dir(&root, &alias).unwrap();
        assert!(LockedPrivateNamespace::open_or_create(alias).is_err());
        assert_eq!(std::fs::read(outside.join("payload")).unwrap(), b"outside");
    }

    #[test]
    fn relative_create_exact_names_and_hardlinks_fail_closed() {
        let (_temp, directory, root) = fixture();
        let (file, _) = create_new_regular(&directory, &root, "Original.txt").unwrap();
        assert!(matches!(
            create_new_regular(&directory, &root, "Original.txt"),
            Err(PrivateFsError::AlreadyExists)
        ));
        assert!(matches!(
            open_regular(&directory, &root, "original.txt", OpenPurpose::Read),
            Err(PrivateFsError::IdentityAmbiguous)
        ));
        for name in [
            "../escape",
            "a\\b",
            "file:stream",
            "trailing.",
            "trailing ",
            ".",
            "..",
        ] {
            assert!(
                create_new_regular(&directory, &root, name).is_err(),
                "accepted {name}"
            );
        }
        std::fs::hard_link(root.join("Original.txt"), root.join("link.txt")).unwrap();
        assert!(identity(&file, Some(false)).is_err());
        assert!(open_regular(&directory, &root, "Original.txt", OpenPurpose::Read).is_err());
    }

    #[test]
    fn relative_rename_is_exclusive_and_replacement_keeps_held_old_identity() {
        let (_temp, directory, root) = fixture();
        let (mut first, first_id) = create_new_regular(&directory, &root, "first").unwrap();
        let (mut second, second_id) = create_new_regular(&directory, &root, "second").unwrap();
        first.write_all(b"first").unwrap();
        second.write_all(b"second").unwrap();
        sync_regular(&first).unwrap();
        sync_regular(&second).unwrap();
        assert_eq!(
            atomic_publish_noreplace(&directory, &root, "first", "second"),
            Err(PrivateFsError::AlreadyExists)
        );
        assert_eq!(
            open_regular(&directory, &root, "second", OpenPurpose::Read)
                .unwrap()
                .1,
            second_id
        );
        atomic_replace(&directory, &root, "first", "second").unwrap();
        sync_directory(&directory).unwrap();
        assert!(relative_name_is_absent(&directory, "first"));
        assert_eq!(
            open_regular(&directory, &root, "second", OpenPurpose::Read)
                .unwrap()
                .1,
            first_id
        );
        second.rewind().unwrap();
        let mut bytes = Vec::new();
        second.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"second");
    }

    #[test]
    fn sealing_flushes_and_reopening_for_recovery_does_not_resolve_a_path() {
        let (_temp, directory, root) = fixture();
        assert!(create_directory(&directory, &root, "child").unwrap());
        let (child, _) = open_child_directory(&directory, &root, "child").unwrap();
        let (mut file, _) = create_new_regular(&child, &root.join("child"), "payload").unwrap();
        file.write_all(b"payload").unwrap();
        sync_regular(&file).unwrap();
        set_regular_mode(&file, RegularMode::Sealed).unwrap();
        sync_regular(&file).unwrap();
        assert!(security::mode(&file).unwrap());
        drop(file);
        set_directory_mode(&child, DirectoryMode::Sealed).unwrap();
        sync_directory(&child).unwrap();
        drop(child);
        let (sealed, _) = open_sealed_child_directory(&directory, &root, "child").unwrap();
        assert!(open_child_directory(&directory, &root, "child").is_err());
        set_directory_mode(&sealed, DirectoryMode::Writable).unwrap();
        sync_directory(&sealed).unwrap();
        remove_regular(&sealed, &root.join("child"), "payload").unwrap();
        drop(sealed);
        remove_directory(&directory, &root, "child").unwrap();
        sync_directory(&directory).unwrap();
    }

    #[test]
    fn lock_contention_is_nonblocking_and_reacquisition_works() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("namespace");
        let first = LockedPrivateNamespace::open_or_create(&root).unwrap();
        assert!(matches!(
            LockedPrivateNamespace::open_or_create(&root),
            Err(PrivateFsError::LockUnavailable)
        ));
        drop(first);
        let second = LockedPrivateNamespace::open_or_create(&root).unwrap();
        second.directory().sync().unwrap();
    }

    #[test]
    fn namespace_lock_child() {
        let Some(root) = std::env::var_os("ZEPHIUM_PRIVATE_FS_LOCK_TEST_ROOT") else {
            return;
        };
        if let Some(slot) = std::env::var_os("ZEPHIUM_PRIVATE_FS_LOCK_TEST_SLOT") {
            first_creation_child(std::path::Path::new(&root), &slot);
            return;
        }
        let expected_busy = std::env::var_os("ZEPHIUM_PRIVATE_FS_LOCK_TEST_BUSY").is_some();
        let result = LockedPrivateNamespace::open_or_create(std::path::PathBuf::from(root));
        if expected_busy {
            assert!(matches!(result, Err(PrivateFsError::LockUnavailable)));
        } else {
            result.unwrap().directory().sync().unwrap();
        }
    }

    #[test]
    fn namespace_lock_is_exclusive_across_processes_and_released_on_owner_drop() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("namespace");
        let owner = LockedPrivateNamespace::open_or_create(&root).unwrap();
        let child = |busy: bool| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "platform::windows::tests::validation::namespace_lock_child",
                    "--nocapture",
                ])
                .env("ZEPHIUM_PRIVATE_FS_LOCK_TEST_ROOT", &root)
                .env_remove("ZEPHIUM_PRIVATE_FS_LOCK_TEST_BUSY");
            if busy {
                command.env("ZEPHIUM_PRIVATE_FS_LOCK_TEST_BUSY", "1");
            }
            let mut process = command.spawn().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                if let Some(status) = process.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = process.kill();
                    let _ = process.wait();
                    panic!("namespace lock child did not complete within 15 seconds");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        child(true);
        drop(owner);
        child(false);
    }

    #[test]
    fn public_namespace_sealed_tree_publication_and_bounded_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("namespace");
        let namespace = LockedPrivateNamespace::open_or_create(&path).unwrap();
        let root = namespace.directory();
        let incoming = root
            .create_new_private_child(&component("incoming"))
            .unwrap();
        incoming
            .write_new_synced(
                &component("payload"),
                b"payload",
                ByteLimit::new(32).unwrap(),
            )
            .unwrap();
        incoming
            .seal_verified_regular(&component("payload"))
            .unwrap();
        let sealed = incoming.seal().unwrap();
        let published = sealed.publish_noreplace(root, &component("ready")).unwrap();
        drop(published);
        drop(namespace);
        let reopened = LockedPrivateNamespace::open_or_create(&path).unwrap();
        let tree = reopened
            .directory()
            .open_private_child_any_mode(&component("ready"))
            .unwrap();
        let report = tree
            .remove_tree_bounded(TreeRemovalLimits::new(1, 1).unwrap())
            .unwrap();
        assert_eq!(report.regular_files_removed(), 1);
        assert_eq!(report.directories_removed(), 1);
        assert_eq!(report.directories_unsealed(), 1);
        assert_eq!(report.directory_syncs(), 1);
    }

    #[test]
    fn cleanup_reports_post_mutation_failure_and_can_resume_after_reopen() {
        let (_temp, directory, root) = fixture();
        create_directory(&directory, &root, "incoming").unwrap();
        let path = root.join("incoming");
        let (child, id) = open_child_directory(&directory, &root, "incoming").unwrap();
        let (file, _) = create_new_regular(&child, &path, "payload").unwrap();
        drop(file);
        let error = super::super::super::tree_removal::remove_tree_bounded(
            &child,
            &path,
            id,
            DirectoryMode::Writable,
            &directory,
            &root,
            "incoming",
            1,
            1,
            super::super::super::TreeRemovalFaults {
                fail_at_mutation: Some(1),
                ..Default::default()
            },
        )
        .err()
        .unwrap();
        assert!(error.mutation_started);
        assert_eq!(error.error, PrivateFsError::Io);
        drop(child);
        let (child, id) = open_child_directory(&directory, &root, "incoming").unwrap();
        let report = super::super::super::tree_removal::remove_tree_bounded(
            &child,
            &path,
            id,
            DirectoryMode::Writable,
            &directory,
            &root,
            "incoming",
            1,
            1,
            Default::default(),
        )
        .unwrap_or_else(|error| panic!("{:?}", error.error));
        assert_eq!(report.directories_removed, 1);
        assert!(relative_name_is_absent(&directory, "incoming"));
    }

    // Own only handles returned by this fixture's spawn. Failure cleanup uses
    // that OS handle, never a process selected by name or a reused PID.
    struct FixtureProcess(std::process::Child);

    impl FixtureProcess {
        fn wait(&mut self) -> std::process::ExitStatus {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                if let Some(status) = self.0.try_wait().unwrap() {
                    return status;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "fixture child deadline"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    impl Drop for FixtureProcess {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
                while std::time::Instant::now() < deadline {
                    if matches!(self.0.try_wait(), Ok(Some(_))) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                eprintln!(
                    "private-fs-fixture: child_cleanup_not_acknowledged pid={}",
                    self.0.id()
                );
            }
        }
    }

    fn wait_fixture_file(path: &std::path::Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !path.is_file() {
            assert!(
                std::time::Instant::now() < deadline,
                "fixture handshake deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn lock_process(root: &std::path::Path, slot: Option<&str>) -> FixtureProcess {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "platform::windows::tests::validation::namespace_lock_child",
                "--nocapture",
            ])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .env("ZEPHIUM_PRIVATE_FS_LOCK_TEST_ROOT", root)
            .env_remove("ZEPHIUM_PRIVATE_FS_LOCK_TEST_BUSY")
            .env_remove("ZEPHIUM_PRIVATE_FS_LOCK_TEST_SLOT");
        if let Some(slot) = slot {
            command.env("ZEPHIUM_PRIVATE_FS_LOCK_TEST_SLOT", slot);
        }
        FixtureProcess(command.spawn().unwrap())
    }

    fn first_creation_child(root: &std::path::Path, slot: &std::ffi::OsStr) {
        let slot = slot.to_str().unwrap();
        assert!(matches!(slot, "0" | "1"));
        let control = root.parent().unwrap();
        std::fs::write(control.join(format!("ready-{slot}")), b"ready").unwrap();
        wait_fixture_file(&control.join("go"));
        let admitted = LockedPrivateNamespace::open_or_create(root);
        let outcome = match &admitted {
            Ok(_) => "owned".to_owned(),
            Err(PrivateFsError::LockUnavailable) => "busy".to_owned(),
            Err(PrivateFsError::SettlementUnknown) => "uncertain".to_owned(),
            Err(PrivateFsError::IdentityAmbiguous) => "ambiguous".to_owned(),
            Err(error) => format!("refused:{error:?}"),
        };
        let pending = control.join(format!("outcome-{slot}-pending"));
        std::fs::write(&pending, outcome).unwrap();
        std::fs::rename(pending, control.join(format!("outcome-{slot}"))).unwrap();
        if admitted.is_ok() {
            // Parent must terminate this exact child while its lease is held,
            // proving OS owner loss rather than normal Rust destructor cleanup.
            wait_fixture_file(&control.join("release"));
        }
    }

    #[test]
    fn simultaneous_first_lock_creation_and_terminated_owner_recover_exactly() {
        const MARKER: &[u8] = b"zephium-private-fs\nlock-format=1\n";
        for _ in 0..4 {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("first-namespace");
            assert!(!root.exists());
            let mut children = [
                lock_process(&root, Some("0")),
                lock_process(&root, Some("1")),
            ];
            for slot in 0..2 {
                wait_fixture_file(&temp.path().join(format!("ready-{slot}")));
            }
            std::fs::write(temp.path().join("go"), b"go").unwrap();
            let outcomes = [0, 1].map(|slot| {
                let path = temp.path().join(format!("outcome-{slot}"));
                wait_fixture_file(&path);
                std::fs::read_to_string(path).unwrap()
            });
            eprintln!("private-fs-fixture: first_creation_outcomes={outcomes:?}");
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|outcome| *outcome == "owned")
                    .count(),
                1,
                "first-creation outcomes: {outcomes:?}"
            );
            // Opening the staging handle across the winner's atomic rename
            // can conservatively refuse exact-name authority before any lease
            // exists (IdentityAmbiguous's documented activation contract).
            // Refusal is acceptable only with one proven owner and exact,
            // unstranded recovery below; unrelated failures still fail here.
            assert!(
                outcomes.iter().all(|outcome| matches!(
                    outcome.as_str(),
                    "owned" | "busy" | "uncertain" | "ambiguous"
                )),
                "first-creation outcomes: {outcomes:?}"
            );
            assert!(matches!(
                LockedPrivateNamespace::open_or_create(&root),
                Err(PrivateFsError::LockUnavailable)
            ));
            let (directory, _) = open_directory(&root).unwrap();
            let before = open_regular(
                &directory,
                &root,
                ".zephium-private-fs-lock-v1",
                OpenPurpose::Read,
            )
            .unwrap()
            .1;
            for (child, outcome) in children.iter_mut().zip(outcomes) {
                if outcome == "owned" {
                    child.0.kill().unwrap();
                    assert!(!child.wait().success());
                } else {
                    assert!(child.wait().success());
                }
            }
            let recovered = LockedPrivateNamespace::open_or_create(&root).unwrap();
            recovered.directory().sync().unwrap();
            drop(recovered);
            assert_eq!(
                std::fs::read(root.join(".zephium-private-fs-lock-v1")).unwrap(),
                MARKER
            );
            assert!(!root.join(".zephium-private-fs-lock-staging-v1").exists());
            assert_eq!(
                open_regular(
                    &directory,
                    &root,
                    ".zephium-private-fs-lock-v1",
                    OpenPurpose::Read
                )
                .unwrap()
                .1,
                before
            );
            assert!(lock_process(&root, None).wait().success());
        }
    }

    #[test]
    fn canonical_and_staging_lock_residue_admit_only_exact_protocol_bytes() {
        const MARKER: &[u8] = b"zephium-private-fs\nlock-format=1\n";
        const CANONICAL: &str = ".zephium-private-fs-lock-v1";
        const STAGING: &str = ".zephium-private-fs-lock-staging-v1";
        let mut payloads: Vec<Vec<u8>> = (0..=MARKER.len())
            .map(|length| MARKER[..length].to_vec())
            .collect();
        payloads.extend([b"unrelated payload".to_vec(), [MARKER, b"extra"].concat()]);
        for name in [CANONICAL, STAGING] {
            for payload in &payloads {
                let (_temp, directory, root) = fixture();
                let (mut file, before) = create_new_regular(&directory, &root, name).unwrap();
                file.write_all(payload).unwrap();
                sync_regular(&file).unwrap();
                sync_directory(&directory).unwrap();
                drop(file);
                let admitted = LockedPrivateNamespace::open_or_create(&root);
                if payload == MARKER || (name == STAGING && MARKER.starts_with(payload)) {
                    let owner = admitted.unwrap();
                    drop(owner);
                    assert_eq!(std::fs::read(root.join(CANONICAL)).unwrap(), MARKER);
                    assert!(!root.join(STAGING).exists());
                    assert_eq!(
                        open_regular(&directory, &root, CANONICAL, OpenPurpose::Read)
                            .unwrap()
                            .1,
                        before
                    );
                    LockedPrivateNamespace::open_or_create(&root).unwrap();
                } else {
                    assert!(matches!(admitted, Err(PrivateFsError::Unsafe)));
                    assert_eq!(std::fs::read(root.join(name)).unwrap(), *payload);
                    assert_eq!(
                        open_regular(&directory, &root, name, OpenPurpose::Read)
                            .unwrap()
                            .1,
                        before
                    );
                    assert!(!root
                        .join(if name == CANONICAL {
                            STAGING
                        } else {
                            CANONICAL
                        })
                        .exists());
                }
            }
        }
    }

    fn make_junction(link: &std::path::Path, target: &std::path::Path) {
        use std::os::windows::process::CommandExt;
        assert!(!link.exists());
        let mut child = FixtureProcess(
            std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:ZEPHIUM_TEST_JUNCTION_LINK -Value $env:ZEPHIUM_TEST_JUNCTION_TARGET | Out-Null"])
                .env("ZEPHIUM_TEST_JUNCTION_LINK", link)
                .env("ZEPHIUM_TEST_JUNCTION_TARGET", target)
                .creation_flags(0x0800_0000)
                .spawn().unwrap(),
        );
        assert!(
            child.wait().success(),
            "ordinary-account junction creation failed"
        );
        assert!(std::fs::symlink_metadata(link).unwrap().file_attributes() & 0x400 != 0);
    }

    #[test]
    fn native_junction_leaf_root_and_ancestor_are_refused_without_target_mutation() {
        let (temp, directory, root) = fixture();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let sentinel = outside.join("sentinel");
        std::fs::write(&sentinel, b"outside unchanged").unwrap();
        let leaf = root.join("junction");
        let alias = temp.path().join("root-junction");
        make_junction(&leaf, &outside);
        make_junction(&alias, &root);
        assert!(open_child_directory(&directory, &root, "junction").is_err());
        assert!(inspect_child(&directory, &root, "junction").is_err());
        assert!(create_new_regular(&directory, &root, "junction").is_err());
        assert!(LockedPrivateNamespace::open_or_create(&alias).is_err());
        assert!(LockedPrivateNamespace::open_or_create(leaf.join("new-namespace")).is_err());
        assert!(!outside.join("new-namespace").exists());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside unchanged");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
        // Remove each junction itself, never recursively traverse its target.
        std::fs::remove_dir(&leaf).unwrap();
        std::fs::remove_dir(&alias).unwrap();
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside unchanged");
    }

    #[test]
    fn custom_data_root_creation_is_private_and_existing_root_is_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("outer").join("inner");
        let resolved = prepare_user_data_root(&root).unwrap();
        assert_eq!(resolved, root.canonicalize().unwrap());
        let (directory, _) = open_directory(&resolved).unwrap();
        let before = security::snapshot(&directory).unwrap();
        assert_eq!(prepare_user_data_root(&resolved).unwrap(), resolved);
        assert_eq!(security::snapshot(&directory).unwrap(), before);
    }
}
