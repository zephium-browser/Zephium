use super::super::super::{identity, native, security, sync_directory};
use super::super::{ANCHOR, LEASE};
use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
#[cfg(feature = "windows-namespace-validation")]
fn work_storage_uses_the_selected_data_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("zephium-data");
    crate::platform::windows::prepare_user_data_root(&root).unwrap();
    let anchor =
        NativeWorkStorageAnchor::prepare_in_data_root(&root, NativeApplication::Product, None)
            .unwrap();
    assert!(anchor
        .database_path()
        .starts_with(root.canonicalize().unwrap()));
}

// All live tests own a random session below the fixed QA application. Neither
// Product nor an existing user's QA session is opened or removed.
pub(super) struct OwnedSession {
    session: NativeSession,
    anchor: Option<NativeWorkStorageAnchor>,
    original: crate::DirectoryIdentity,
    cleaned: bool,
    _random: tempfile::TempDir,
}

impl OwnedSession {
    pub(super) fn new() -> Self {
        let random = tempfile::Builder::new()
            .prefix("typed-")
            .rand_bytes(16)
            .tempdir()
            .unwrap();
        let label = random.path().file_name().unwrap().to_str().unwrap();
        let session = NativeSession::new(label).unwrap();
        let anchor =
            NativeWorkStorageAnchor::prepare(NativeApplication::ExtensionQa, Some(&session))
                .unwrap();
        let original = anchor.inner.identity();
        Self {
            session,
            anchor: Some(anchor),
            original,
            cleaned: false,
            _random: random,
        }
    }

    pub(super) fn anchor(&self) -> &NativeWorkStorageAnchor {
        self.anchor.as_ref().unwrap()
    }

    fn cleanup(&mut self) -> Result<(), PrivateFsError> {
        let expected = known_profile()?
            .join(ANCHOR)
            .join(NativeApplication::ExtensionQa.component())
            .join("qa-sessions")
            .join(format!("session-{}", self.session.0));
        let anchor = match self.anchor.take() {
            Some(anchor) => anchor,
            None => NativeWorkStorageAnchor {
                inner: NativeStorageAnchor::prepare_under_existing_session(
                    &known_profile()?,
                    &self.session,
                )?,
            },
        };
        if anchor.inner.path != expected || anchor.inner.identity() != self.original {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        // Original parent/root handles still deny rename. Only the four fixed
        // leaves in this owned session can be unlinked, including a hostile link.
        let parent = &anchor
            .inner
            .directories
            .last()
            .ok_or(PrivateFsError::Unsafe)?
            .file;
        if crate::DirectoryIdentity(identity(parent, Some(true))?.0) != self.original {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        security::native_storage::verify(parent)?;
        let entries = std::fs::read_dir(&expected).map_err(|_| PrivateFsError::Unsafe)?;
        let mut count = 0;
        for entry in entries {
            count += 1;
            let entry = entry.map_err(|_| PrivateFsError::Unsafe)?;
            let name = entry.file_name();
            if count > 4
                || ![DATABASE, WAL, SHM, LEASE]
                    .iter()
                    .any(|fixed| name == *fixed)
            {
                return Err(PrivateFsError::Unsafe);
            }
            if name != LEASE {
                std::fs::remove_file(entry.path()).map_err(|_| PrivateFsError::Unsafe)?;
            }
        }
        // The original lease denies deletion. Release it only after removing
        // owned payloads, then delete the exact original lock through the still
        // pinned root. A competing owner or replacement makes cleanup refuse.
        let lock_identity = anchor.inner.lock_identity;
        drop(anchor.inner.lock);
        let lock = native::open(
            parent,
            LEASE,
            Some(false),
            native::READ | native::METADATA,
            7,
            native::OPEN,
            None,
        )?;
        super::super::verify_lock(&lock)?;
        if identity(&lock, Some(false))?.0 != lock_identity {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        native::delete(&lock)?;
        drop(lock);
        let mut directories = anchor.inner.directories;
        let leaf = directories.pop().ok_or(PrivateFsError::Unsafe)?;
        let name = leaf.name.clone().ok_or(PrivateFsError::Unsafe)?;
        drop(leaf);
        let parent = &directories.last().ok_or(PrivateFsError::Unsafe)?.file;
        let exact = native::open(
            parent,
            &name,
            Some(true),
            native::READ | native::METADATA,
            7,
            native::OPEN,
            None,
        )?;
        native::exact_name(&exact, &name)?;
        if crate::DirectoryIdentity(identity(&exact, Some(true))?.0) != self.original {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        security::native_storage::verify(&exact)?;
        native::delete(&exact)?;
        drop(exact);
        sync_directory(parent)?;
        match std::fs::symlink_metadata(expected) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.cleaned = true;
                Ok(())
            }
            _ => Err(PrivateFsError::SettlementUnknown),
        }
    }

    pub(super) fn finish(mut self) {
        self.cleanup().unwrap();
    }
}

impl Drop for OwnedSession {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = self.cleanup();
        }
    }
}

#[test]
fn fixed_known_folder_entry_keeps_general_namespace_closed() {
    let fixture = OwnedSession::new();
    let anchor = fixture.anchor();
    anchor.verify().unwrap();
    let database = anchor.create_or_hold_database().unwrap();
    database.verify(anchor).unwrap();
    assert!(std::fs::rename(
        anchor.database_path(),
        anchor.database_path().with_file_name("replacement")
    )
    .is_err());
    database.verify(anchor).unwrap();
    anchor.sync().unwrap();
    assert!(matches!(anchor.hold_wal(), Err(PrivateFsError::NotFound)));
    assert!(matches!(anchor.hold_shm(), Err(PrivateFsError::NotFound)));
    #[cfg(not(feature = "windows-namespace-validation"))]
    {
        assert!(matches!(
            NativeStorageAnchor::prepare(NativeApplication::Product, None),
            Err(PrivateFsError::PrimitiveUnavailable)
        ));
        let arbitrary = anchor
            .database_path()
            .with_file_name("not-a-work-namespace");
        assert!(matches!(
            crate::LockedPrivateNamespace::open_or_create(&arbitrary),
            Err(PrivateFsError::PrimitiveUnavailable)
        ));
        assert!(!arbitrary.exists());
    }
    drop(database);
    fixture.finish();
}

#[test]
fn fixed_database_refuses_orphan_sidecars_and_hard_links() {
    let fixture = OwnedSession::new();
    let anchor = fixture.anchor();
    let database = anchor.database_path();
    let wal = database.with_file_name(WAL);
    std::fs::write(&wal, b"owned orphan").unwrap();
    assert!(matches!(
        anchor.create_or_hold_database(),
        Err(PrivateFsError::Unsafe)
    ));
    assert!(!database.exists());
    assert_eq!(std::fs::read(&wal).unwrap(), b"owned orphan");
    std::fs::remove_file(&wal).unwrap();
    let guard = anchor.create_or_hold_database().unwrap();
    std::fs::hard_link(&database, &wal).unwrap();
    assert!(anchor.hold_wal().is_err());
    assert!(guard.verify(anchor).is_err());
    assert!(matches!(anchor.verify(), Err(PrivateFsError::Quarantined)));
    drop(guard);
    fixture.finish();
}

#[test]
fn fixed_database_refuses_real_reparse_without_touching_target() {
    let fixture = OwnedSession::new();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("sentinel");
    std::fs::write(&target, b"unchanged").unwrap();
    std::os::windows::fs::symlink_file(&target, fixture.anchor().database_path()).unwrap();
    assert!(fixture.anchor().create_or_hold_database().is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"unchanged");
    fixture.finish();
    assert_eq!(std::fs::read(&target).unwrap(), b"unchanged");
}

const CHILD_SESSION: &str = "ZEPHIUM_FIXED_WORK_CHILD_SESSION";

#[cfg(feature = "windows-work-test-fixtures")]
#[test]
fn test_session_owner_requires_fresh_root_and_retires_exact_scope() {
    let random = tempfile::Builder::new()
        .prefix("owner-")
        .rand_bytes(16)
        .tempdir()
        .unwrap();
    let session = NativeSession::new(random.path().file_name().unwrap().to_str().unwrap()).unwrap();
    let owner = super::super::NativeWorkStorageTestSession::create(&session).unwrap();
    assert_eq!(owner.session(), &session);
    assert!(matches!(
        super::super::NativeWorkStorageTestSession::create(&session),
        Err(PrivateFsError::AlreadyExists)
    ));
    let anchor =
        NativeWorkStorageAnchor::prepare(NativeApplication::ExtensionQa, Some(owner.session()))
            .unwrap();
    let path = anchor.database_path();
    anchor
        .create_or_hold_database()
        .unwrap()
        .verify(&anchor)
        .unwrap();
    drop(anchor);
    owner.retire().unwrap();
    assert_eq!(
        std::fs::symlink_metadata(path.parent().unwrap())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn typed_entry_child() {
    let Ok(label) = std::env::var(CHILD_SESSION) else {
        return;
    };
    let session = NativeSession::new(&label).unwrap();
    match NativeWorkStorageAnchor::prepare(NativeApplication::ExtensionQa, Some(&session)) {
        Ok(anchor) => {
            let database = anchor.create_or_hold_database().unwrap();
            database.verify(&anchor).unwrap();
            let mut output = std::io::stdout().lock();
            output.write_all(b"typed-entry:owned\n").unwrap();
            output.flush().unwrap();
            drop(output);
            let mut input = [0];
            let _ = std::io::stdin().read(&mut input);
        }
        Err(PrivateFsError::LockUnavailable) => {
            let mut output = std::io::stdout().lock();
            output.write_all(b"typed-entry:blocked\n").unwrap();
            output.flush().unwrap();
        }
        Err(error) => panic!("fixed typed child admission: {error:?}"),
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_child(session: &NativeSession) -> (OwnedChild, mpsc::Receiver<&'static str>) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "platform::windows::native_storage::work::tests::typed_entry_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_SESSION, &session.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    let output = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines().take(8) {
            let Ok(line) = line else {
                break;
            };
            let state = if line.contains("typed-entry:owned") {
                Some("owned")
            } else if line.contains("typed-entry:blocked") {
                Some("blocked")
            } else {
                None
            };
            if let Some(state) = state {
                let _ = sender.send(state);
                // Keep draining the owned test harness through EOF. Closing
                // stdout at readiness makes its final report fail BrokenPipe.
            }
        }
    });
    (OwnedChild(child), receiver)
}

#[test]
fn fixed_entry_excludes_other_process_and_recovers_terminated_owner() {
    let mut fixture = OwnedSession::new();
    drop(fixture.anchor.take());
    let (mut owner, owner_ready) = spawn_child(&fixture.session);
    assert_eq!(
        owner_ready.recv_timeout(Duration::from_secs(15)).unwrap(),
        "owned"
    );
    let (mut contender, contender_ready) = spawn_child(&fixture.session);
    assert_eq!(
        contender_ready
            .recv_timeout(Duration::from_secs(15))
            .unwrap(),
        "blocked"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = contender.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "contender exit deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    let recovered =
        NativeWorkStorageAnchor::prepare(NativeApplication::ExtensionQa, Some(&fixture.session))
            .unwrap();
    assert_eq!(recovered.inner.identity(), fixture.original);
    recovered
        .create_or_hold_database()
        .unwrap()
        .verify(&recovered)
        .unwrap();
    fixture.anchor = Some(recovered);
    fixture.finish();
}
