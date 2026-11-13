use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(any(not(target_os = "macos"), test))]
use zephium_core::ids::ProfileId;
use zephium_core::ports::engine::ProfileDataErasureOutcome;

const ERASURE_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(any(not(target_os = "macos"), test))]
const REMOVE_ATTEMPTS: usize = 40;
#[cfg(any(not(target_os = "macos"), test))]
const REMOVE_RETRY_DELAY: Duration = Duration::from_millis(50);

type ErasureDone = Box<dyn FnOnce(ProfileDataErasureOutcome) + Send>;

pub(crate) fn metadata_is_direct_directory(metadata: &fs::Metadata) -> bool {
    #[cfg(target_os = "windows")]
    let is_reparse_point = {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT. Junctions and mount points are not
        // necessarily reported by FileType::is_symlink().
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(target_os = "windows"))]
    let is_reparse_point = false;

    metadata.is_dir() && !metadata.file_type().is_symlink() && !is_reparse_point
}

fn direct_directory_identity(path: &Path) -> io::Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata_is_direct_directory(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "engine data path is not a direct owned directory",
        ));
    }
    #[cfg(target_os = "windows")]
    for ancestor in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(ancestor)?;
        if !metadata_is_direct_directory(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "engine data path has a redirecting/non-directory ancestor: {}",
                    ancestor.display()
                ),
            ));
        }
    }
    path.canonicalize()
}

/// Make a completed unlink/recursive removal durable on Unix before higher
/// layers clear their deletion journal. File deletion and SQLite durability
/// are separate ordering domains: without syncing the containing directory,
/// a sudden power loss could preserve the journal deletion while resurrecting
/// the directory entry that contained private data.
#[cfg(all(unix, any(not(target_os = "macos"), test)))]
fn sync_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata_is_direct_directory(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "durability barrier target is not a direct directory",
        ));
    }

    fs::File::open(path)?.sync_all()
}

#[cfg(any(not(target_os = "macos"), test))]
fn sync_removed_child(parent: &Path) -> bool {
    #[cfg(unix)]
    {
        sync_directory(parent).is_ok()
    }
    #[cfg(target_os = "windows")]
    {
        // Win32 does not document FlushFileBuffers for directory handles.
        // Absence is revalidated here, but power-loss durability remains a
        // packaged real-OS release gate instead of relying on an unsupported
        // pseudo-fsync that would fail every deletion.
        let _ = parent;
        true
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn sync_removed_root(root: &Path) -> io::Result<()> {
    let parent = root.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "engine data root has no containing directory",
        )
    })?;
    #[cfg(unix)]
    {
        sync_directory(parent)
    }
    #[cfg(target_os = "windows")]
    {
        // See `sync_removed_child`: Windows cleanup is crash-resumable and
        // exact-process-gated, but no unsupported directory flush is claimed.
        let _ = parent;
        Ok(())
    }
}

/// Exactly-once public completion plus the per-attempt admission flag kept by
/// the UI-thread host. Timing out reports once but deliberately keeps the slot
/// occupied; only a later native terminal callback releases admission, and it
/// cannot invoke the public completion a second time.
pub(crate) struct Completion {
    done: Mutex<Option<ErasureDone>>,
    #[cfg(target_os = "windows")]
    terminal: Mutex<(Option<ProfileDataErasureOutcome>, Vec<ErasureDone>)>,
    active: Arc<AtomicBool>,
    settled: Mutex<bool>,
    settled_changed: Condvar,
    #[cfg(test)]
    watchdog_exited: AtomicBool,
}

impl Completion {
    pub(crate) fn start(done: ErasureDone, active: Arc<AtomicBool>) -> Arc<Self> {
        Self::start_with_timeout(done, active, ERASURE_TIMEOUT)
    }

    fn start_with_timeout(
        done: ErasureDone,
        active: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Arc<Self> {
        let completion = Arc::new(Self {
            done: Mutex::new(Some(done)),
            #[cfg(target_os = "windows")]
            terminal: Mutex::new((None, Vec::new())),
            active,
            settled: Mutex::new(false),
            settled_changed: Condvar::new(),
            #[cfg(test)]
            watchdog_exited: AtomicBool::new(false),
        });
        let watchdog = completion.clone();
        let watchdog_task = move || {
            let settled = watchdog
                .settled
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (settled, wait) = watchdog
                .settled_changed
                .wait_timeout_while(settled, timeout, |settled| !*settled)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let timed_out = !*settled && wait.timed_out();
            drop(settled);
            if timed_out {
                // Reporting a caller-visible timeout does not prove that the
                // native async operation stopped. Keep the admission flag set
                // until a real platform terminal callback calls `finish`.
                watchdog.report_unsettled(ProfileDataErasureOutcome::TimedOut);
            }
            #[cfg(test)]
            watchdog.watchdog_exited.store(true, Ordering::Release);
        };
        if let Err(error) = std::thread::Builder::new()
            .name("zephium-erasure-watchdog".into())
            .spawn(watchdog_task)
        {
            // Without the watchdog there is no bounded proof that a native
            // clear operation will ever call back. Report the same fail-closed
            // terminal condition immediately; keep `active` set until a real
            // platform completion arrives so a retry cannot fabricate absence.
            eprintln!("privacy: cannot start profile-erasure watchdog: {error}");
            completion.report_unsettled(ProfileDataErasureOutcome::TimedOut);
        }
        completion
    }

    /// Group real native terminals. Watchdog reports never release a sibling's physical debt.
    #[cfg(target_os = "windows")]
    #[cfg_attr(
        all(target_os = "windows", not(feature = "agentic-browser")),
        allow(dead_code)
    )]
    pub(crate) fn split(parent: Arc<Self>, count: usize) -> Vec<Arc<Self>> {
        let state = Arc::new(Mutex::new((count, true)));
        (0..count)
            .map(|_| {
                let child = Self::start(Box::new(|_| {}), Arc::new(AtomicBool::new(true)));
                let group = state.clone();
                let parent = parent.clone();
                child.observe_terminal(Box::new(move |outcome| {
                    let terminal = {
                        let mut state = group.lock().unwrap_or_else(|p| p.into_inner());
                        state.0 = state.0.saturating_sub(1);
                        state.1 &= outcome == ProfileDataErasureOutcome::Verified;
                        (state.0 == 0).then_some(state.1)
                    };
                    if let Some(clean) = terminal {
                        parent.finish(if clean {
                            ProfileDataErasureOutcome::Verified
                        } else {
                            ProfileDataErasureOutcome::Failed
                        });
                    }
                }));
                child
            })
            .collect()
    }
    #[cfg(target_os = "windows")]
    #[cfg_attr(
        all(target_os = "windows", not(feature = "agentic-browser")),
        allow(dead_code)
    )]
    pub(crate) fn terminal_outcome(&self) -> Option<ProfileDataErasureOutcome> {
        self.terminal.lock().unwrap_or_else(|p| p.into_inner()).0
    }

    /// Join the real native terminal; a public watchdog report is never cleanup proof.
    #[cfg(target_os = "windows")]
    #[cfg_attr(
        all(target_os = "windows", not(feature = "agentic-browser")),
        allow(dead_code)
    )]
    pub(crate) fn forward_terminal(&self, target: Arc<Self>) {
        self.observe_terminal(Box::new(move |outcome| target.finish(outcome)));
    }

    #[cfg(target_os = "windows")]
    #[cfg_attr(
        all(target_os = "windows", not(feature = "agentic-browser")),
        allow(dead_code)
    )]
    fn observe_terminal(&self, observer: ErasureDone) {
        let mut terminal = self.terminal.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(outcome) = terminal.0 {
            drop(terminal);
            observer(outcome);
        } else {
            terminal.1.push(observer);
        }
    }

    #[cfg(target_os = "windows")]
    fn record_terminal(&self, outcome: ProfileDataErasureOutcome) {
        let observers = {
            let mut terminal = self.terminal.lock().unwrap_or_else(|p| p.into_inner());
            if terminal.0.is_some() {
                return;
            }
            terminal.0 = Some(outcome);
            std::mem::take(&mut terminal.1)
        };
        for observer in observers {
            observer(outcome);
        }
    }

    pub(crate) fn attempt_flag(&self) -> Arc<AtomicBool> {
        self.active.clone()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn finish(&self, outcome: ProfileDataErasureOutcome) {
        // Serialize settlement with the watchdog. If native completion wins
        // this lock, the caller receives its terminal outcome; if the
        // watchdog wins, native settlement only releases admission and never
        // invokes the callback a second time.
        let done = {
            let mut done = self
                .done
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.active.store(false, Ordering::Release);
            done.take()
        };
        {
            let mut settled = self
                .settled
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *settled = true;
            self.settled_changed.notify_all();
        }
        #[cfg(target_os = "windows")]
        self.record_terminal(outcome);
        if let Some(done) = done {
            done(outcome);
        }
    }

    /// Terminal failure paths must not wait for an arbitrary public callback
    /// before invoking the composition-root fatal action. Settle exactly once,
    /// wake the watchdog, and invoke the callback on a detached worker.
    pub(crate) fn finish_detached(&self, outcome: ProfileDataErasureOutcome) {
        let done = {
            let mut done = self
                .done
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.active.store(false, Ordering::Release);
            done.take()
        };
        {
            let mut settled = self
                .settled
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *settled = true;
            self.settled_changed.notify_all();
        }
        #[cfg(target_os = "windows")]
        self.record_terminal(outcome);
        if let Some(done) = done {
            let _ = std::thread::Builder::new()
                .name("zephium-erasure-completion".into())
                .spawn(move || done(outcome));
        }
    }

    /// Report an outcome without claiming the platform operation settled.
    /// The attempt stays in flight and retries remain denied.
    pub(crate) fn report_unsettled(&self, outcome: ProfileDataErasureOutcome) {
        let done = self
            .done
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(done) = done {
            done(outcome);
        }
    }

    #[cfg(test)]
    fn watchdog_has_exited(&self) -> bool {
        self.watchdog_exited.load(Ordering::Acquire)
    }
}

pub(crate) fn canonical_owned_root(path: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(path)?;
    let canonical = direct_directory_identity(path)?;
    #[cfg(target_os = "windows")]
    windows::restrict_directory(&canonical)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(canonical)
}

#[cfg(any(not(target_os = "macos"), test))]
pub(crate) fn profile_path(root: &Path, profile: ProfileId) -> PathBuf {
    // ProfileId's Display is a canonical, separator-free ULID. Keeping path
    // derivation here prevents deletion callers from supplying arbitrary
    // relative paths.
    root.join(profile.to_string())
}

#[cfg(any(not(target_os = "macos"), test))]
pub(crate) fn prepare_profile_directory(root: &Path, profile: ProfileId) -> io::Result<PathBuf> {
    validate_existing_root(root)?;
    let path = profile_path(root, profile);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // The canonical parent already exists; create exactly one typed
            // child so a final-component symlink can never be traversed by a
            // recursive directory creator.
            if let Err(create_error) = fs::create_dir(&path) {
                if create_error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(create_error);
                }
            }
        }
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata_is_direct_directory(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile data path is not an owned directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    let canonical = path.canonicalize()?;
    if canonical.parent() != Some(root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "profile data escaped its owned root",
        ));
    }
    #[cfg(target_os = "windows")]
    windows::restrict_directory(&canonical)?;
    Ok(canonical)
}

#[cfg(any(not(target_os = "macos"), test))]
fn validate_existing_root(root: &Path) -> io::Result<()> {
    if direct_directory_identity(root)? != root {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "engine data root changed after startup",
        ));
    }
    Ok(())
}

#[cfg(any(not(target_os = "macos"), test))]
fn remove_one_profile_directory(root: &Path, profile: ProfileId) -> bool {
    let path = profile_path(root, profile);
    for attempt in 0..REMOVE_ATTEMPTS {
        match direct_directory_identity(root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return sync_removed_root(root).is_ok();
            }
            Err(_) => return false,
            Ok(canonical) if canonical == root => {}
            Ok(_) => return false,
        }

        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return sync_removed_child(root);
            }
            Err(_) => return false,
            // Refuse ambiguous filesystem objects rather than following or
            // deleting an attacker-controlled link target.
            Ok(metadata) if !metadata_is_direct_directory(&metadata) => {
                return false;
            }
            Ok(_) => {
                let Ok(canonical) = path.canonicalize() else {
                    if attempt + 1 < REMOVE_ATTEMPTS {
                        std::thread::sleep(REMOVE_RETRY_DELAY);
                        continue;
                    }
                    return false;
                };
                if canonical.parent() != Some(root) {
                    return false;
                }
                // Revalidate the exact object immediately before every
                // recursive delete. A stored canonical path is not proof that
                // another process did not replace it with a junction later.
                let pre_delete_identity = match direct_directory_identity(&canonical) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
                    Err(_) if attempt + 1 < REMOVE_ATTEMPTS => {
                        std::thread::sleep(REMOVE_RETRY_DELAY);
                        continue;
                    }
                    Err(_) => return false,
                    Ok(identity) => identity,
                };
                if pre_delete_identity != canonical || canonical.parent() != Some(root) {
                    return false;
                }
                let _ = fs::remove_dir_all(&canonical);
            }
        }

        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return sync_removed_child(root);
            }
            _ if attempt + 1 < REMOVE_ATTEMPTS => {
                std::thread::sleep(REMOVE_RETRY_DELAY);
            }
            _ => return false,
        }
    }
    false
}

/// Exercise exact root-removal identity and durability behavior independently
/// of a platform runtime manager. Production profile erasure deletes only
/// typed profile children; Windows runtime-generation roots are now owned by
/// `zephium_core::webview2::RuntimeGeneration` and use its provenance gate.
#[cfg(test)]
pub(crate) fn remove_owned_root_once(root: &Path) -> io::Result<bool> {
    match direct_directory_identity(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            sync_removed_root(root)?;
            return Ok(true);
        }
        Err(error) => return Err(error),
        Ok(canonical) if canonical == root => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "engine data root identity changed before deletion",
            ));
        }
    }
    // A second metadata read keeps the safety check adjacent to the destructive
    // call. Fully race-free recursive deletion still ultimately depends on OS
    // handle-based APIs, but no known reparse point is ever followed here.
    if direct_directory_identity(root)? != root {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "engine data root changed immediately before deletion",
        ));
    }
    match fs::remove_dir_all(root) {
        Ok(()) => {
            sync_removed_root(root)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            sync_removed_root(root)?;
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

#[cfg(any(not(target_os = "macos"), test))]
pub(crate) fn remove_profile_directories_verified(roots: &[PathBuf], profile: ProfileId) -> bool {
    // Do not short-circuit: a failure in one engine root must not prevent a
    // best-effort removal and verification of the other independent root.
    let mut verified = true;
    for root in roots {
        verified = remove_one_profile_directory(root, profile) && verified;
    }
    verified
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn windows_profile_paths_require_acl_settlement_before_admission() {
        let source = include_str!("erasure.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        for function in ["canonical_owned_root", "prepare_profile_directory"] {
            let body = source
                .split(&format!("fn {function}("))
                .nth(1)
                .unwrap()
                .split("\n}\n")
                .next()
                .unwrap();
            let secured = body
                .find("windows::restrict_directory(&canonical)?;")
                .unwrap();
            assert!(secured < body.find("Ok(canonical)").unwrap());
            assert!(body[..secured].ends_with("#[cfg(target_os = \"windows\")]\n    "));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_profile_acl_is_private_inheritable_and_preserves_existing_data() {
        use ::windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
        use ::windows::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SetSecurityInfo,
            SE_FILE_OBJECT,
        };
        use ::windows::Win32::Security::{
            EqualSid, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
            IsWellKnownSid, WinLocalSystemSid, ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION,
            OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
            PSID, SE_DACL_PROTECTED,
        };
        use ::windows::Win32::Storage::FileSystem::{
            FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, READ_CONTROL, WRITE_DAC,
        };
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;

        fn open(path: &Path) -> fs::File {
            fs::OpenOptions::new()
                .access_mode(READ_CONTROL.0 | WRITE_DAC.0)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
                .open(path)
                .unwrap()
        }
        fn assert_acl(file: &fs::File, protected: bool, flags: u8) {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let mut acl = std::ptr::null_mut::<ACL>();
            let mut owner = PSID::default();
            // SAFETY: fixture-owned handle and initialized descriptor outputs.
            unsafe {
                GetSecurityInfo(
                    HANDLE(file.as_raw_handle()),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
                    Some(&mut owner),
                    None,
                    Some(&mut acl),
                    None,
                    Some(&mut sd),
                )
                .ok()
                .unwrap();
                let mut control = 0;
                let mut revision = 0;
                GetSecurityDescriptorControl(sd, &mut control, &mut revision).unwrap();
                assert_eq!(control & SE_DACL_PROTECTED.0 != 0, protected);
                assert!(!acl.is_null());
                assert_eq!((*acl).AceCount, 2);
                let mut found_owner = false;
                let mut found_system = false;
                for index in 0..2 {
                    let mut ace = std::ptr::null_mut();
                    GetAce(acl, index, &mut ace).unwrap();
                    let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                    assert_eq!(ace.Header.AceType, 0);
                    assert_eq!(ace.Header.AceFlags, flags);
                    assert_eq!(ace.Mask, FILE_ALL_ACCESS.0);
                    let sid = PSID((&ace.SidStart as *const u32).cast_mut().cast());
                    found_owner |= EqualSid(sid, owner).is_ok();
                    found_system |= IsWellKnownSid(sid, WinLocalSystemSid).as_bool();
                }
                assert!(found_owner && found_system);
                let _ = LocalFree(Some(HLOCAL(sd.0)));
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let root = canonical_owned_root(&temp.path().join("profiles")).unwrap();
        let id = ProfileId::from(343);
        let path = root.join(id.to_string());
        fs::create_dir(&path).unwrap();
        let file = open(&path);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: fixed SDDL deliberately widens only this disposable fixture;
        // all native allocations remain live through their use and are freed.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                ::windows::core::w!("D:P(A;OICI;FA;;;WD)"),
                1,
                &mut sd,
                None,
            )
            .unwrap();
            let mut acl = std::ptr::null_mut();
            let mut present = false.into();
            let mut defaulted = false.into();
            GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted).unwrap();
            SetSecurityInfo(
                HANDLE(file.as_raw_handle()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(acl),
                None,
            )
            .ok()
            .unwrap();
            let _ = LocalFree(Some(HLOCAL(sd.0)));
        }
        drop(file);
        fs::write(path.join("cookie-fixture"), b"retained").unwrap();
        assert_eq!(prepare_profile_directory(&root, id).unwrap(), path);
        assert_acl(&open(&path), true, 3);
        assert_acl(&open(&path.join("cookie-fixture")), false, 0x10);
        assert_eq!(fs::read(path.join("cookie-fixture")).unwrap(), b"retained");
        assert_eq!(prepare_profile_directory(&root, id).unwrap(), path);
        let ordinary_file = root.join("not-a-directory");
        fs::write(&ordinary_file, b"keep").unwrap();
        assert!(super::windows::restrict_directory(&ordinary_file).is_err());
        assert_eq!(fs::read(ordinary_file).unwrap(), b"keep");
    }
    #[test]
    fn completion_is_exactly_once_and_releases_attempt() {
        let (tx, rx) = mpsc::channel();
        let completion = Completion::start_with_timeout(
            Box::new(move |outcome| tx.send(outcome).unwrap()),
            Arc::new(AtomicBool::new(true)),
            Duration::from_secs(1),
        );
        let active = completion.attempt_flag();
        completion.finish(ProfileDataErasureOutcome::Verified);
        completion.finish(ProfileDataErasureOutcome::Failed);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)).unwrap(),
            ProfileDataErasureOutcome::Verified
        );
        assert!(rx.recv_timeout(Duration::from_millis(25)).is_err());
        assert!(!active.load(Ordering::Acquire));
        let deadline = std::time::Instant::now() + Duration::from_millis(200);
        while !completion.watchdog_has_exited() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            completion.watchdog_has_exited(),
            "terminal completion must cancel and wake its watchdog"
        );
    }

    #[test]
    fn timeout_reports_once_but_keeps_native_attempt_in_flight() {
        let (tx, rx) = mpsc::channel();
        let completion = Completion::start_with_timeout(
            Box::new(move |outcome| tx.send(outcome).unwrap()),
            Arc::new(AtomicBool::new(true)),
            Duration::from_millis(10),
        );
        let active = completion.attempt_flag();
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(200)).unwrap(),
            ProfileDataErasureOutcome::TimedOut
        );
        assert!(active.load(Ordering::Acquire));
        completion.finish(ProfileDataErasureOutcome::Verified);
        assert!(!active.load(Ordering::Acquire));
        assert!(rx.recv_timeout(Duration::from_millis(25)).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn grouped_erasure_timeout_retains_debt_and_joins_a_late_native_terminal() {
        let (tx, rx) = mpsc::channel();
        let parent = Completion::start_with_timeout(
            Box::new(move |outcome| {
                tx.send(outcome).unwrap();
            }),
            Arc::new(AtomicBool::new(true)),
            Duration::from_millis(10),
        );
        let children = Completion::split(parent.clone(), 2);
        children[0].finish(ProfileDataErasureOutcome::Verified);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(200)).unwrap(),
            ProfileDataErasureOutcome::TimedOut
        );
        assert!(parent.is_active());
        assert_eq!(parent.terminal_outcome(), None);
        let joined = Completion::start(Box::new(|_| {}), Arc::new(AtomicBool::new(true)));
        parent.forward_terminal(joined.clone());
        children[1].finish(ProfileDataErasureOutcome::Verified);
        assert!(!parent.is_active());
        assert_eq!(
            joined.terminal_outcome(),
            Some(ProfileDataErasureOutcome::Verified)
        );
        children[1].finish(ProfileDataErasureOutcome::Failed);
        assert_eq!(
            joined.terminal_outcome(),
            Some(ProfileDataErasureOutcome::Verified)
        );
        assert!(rx.recv_timeout(Duration::from_millis(25)).is_err());
        let late = Completion::start(Box::new(|_| {}), Arc::new(AtomicBool::new(true)));
        parent.forward_terminal(late.clone());
        assert_eq!(
            late.terminal_outcome(),
            Some(ProfileDataErasureOutcome::Verified)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn grouped_erasure_failure_still_waits_for_every_native_terminal() {
        let parent = Completion::start(Box::new(|_| {}), Arc::new(AtomicBool::new(true)));
        let children = Completion::split(parent.clone(), 2);
        children[0].finish(ProfileDataErasureOutcome::Failed);
        assert!(parent.is_active());
        children[1].finish(ProfileDataErasureOutcome::Verified);
        assert_eq!(
            parent.terminal_outcome(),
            Some(ProfileDataErasureOutcome::Failed)
        );
        assert!(!parent.is_active());
    }

    #[test]
    fn owned_profile_directory_is_removed_and_absence_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let root = canonical_owned_root(&temp.path().join("profiles")).unwrap();
        let profile = ProfileId::from(7);
        let path = prepare_profile_directory(&root, profile).unwrap();
        fs::create_dir(path.join("nested")).unwrap();
        fs::write(path.join("nested/data"), b"private").unwrap();

        assert!(remove_profile_directories_verified(
            std::slice::from_ref(&root),
            profile
        ));
        assert!(!profile_path(&root, profile).exists());
        assert!(remove_profile_directories_verified(&[root], profile));
    }

    #[test]
    fn owned_root_removal_revalidates_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let root = canonical_owned_root(&temp.path().join("private-runtime")).unwrap();
        fs::write(root.join("runtime-state"), b"private").unwrap();

        assert!(remove_owned_root_once(&root).unwrap());
        assert!(!root.exists());
        assert!(remove_owned_root_once(&root).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_profile_is_never_followed_or_reported_erased() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = canonical_owned_root(&temp.path().join("profiles")).unwrap();
        let target = temp.path().join("outside");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("secret"), b"keep").unwrap();
        let profile = ProfileId::from(9);
        symlink(&target, profile_path(&root, profile)).unwrap();

        assert!(!remove_profile_directories_verified(&[root], profile));
        assert_eq!(fs::read(target.join("secret")).unwrap(), b"keep");
    }
}
