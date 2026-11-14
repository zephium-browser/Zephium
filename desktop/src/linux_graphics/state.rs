use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use zephium_core::linux_graphics::{
    CrashRecovery, RendererPath, RendererPreference, MAX_CRASH_FALLBACK_STEPS,
};
use zephium_private_fs::{ByteLimit, LockedPrivateNamespace, PrivateComponent, PrivateFsError};

const NAMESPACE: &str = "renderer";
const RECORD: &str = "state-v1.json";
const STAGING: &str = ".state-v1.next";
const MAX_STATE_BYTES: usize = 2048;
const VERSION: u8 = 2;
const MAX_FAILURES: usize = 8;
const FAILURES_TO_HOLD: usize = 2;
const MAX_CONFIG_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct StateSnapshot {
    pub preference: RendererPreference,
    pub recovery: CrashRecovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StateError {
    ConfigUnavailable,
    InvalidRecord,
    NotAutomatic,
    Filesystem(PrivateFsError),
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigUnavailable => formatter.write_str("renderer configuration is unavailable"),
            Self::InvalidRecord => formatter.write_str("renderer state has an invalid format"),
            Self::NotAutomatic => formatter.write_str("renderer preference is not automatic"),
            Self::Filesystem(error) => write!(formatter, "renderer state: {error}"),
        }
    }
}

impl From<PrivateFsError> for StateError {
    fn from(error: PrivateFsError) -> Self {
        Self::Filesystem(error)
    }
}

/// What decides whether a renderer path starts: driver class, GL renderer, driver
/// version, WebKitGTK version and session type, hashed so the record holds no names.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(super) struct Fingerprint(String);

impl Fingerprint {
    pub fn of(facts: &[&str]) -> Self {
        // FNV-1a: stable across builds and toolchains, unlike the std hasher.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for fact in facts {
            for byte in fact.bytes().take(4096).chain([0]) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        Self(format!("{hash:016x}"))
    }
}

impl TryFrom<String> for Fingerprint {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 16
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value))
        } else {
            Err("invalid graphics fingerprint")
        }
    }
}

impl From<Fingerprint> for String {
    fn from(value: Fingerprint) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PendingLaunch {
    path: RendererPath,
    fingerprint: Fingerprint,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FailedStart {
    fingerprint: Fingerprint,
    path: RendererPath,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedState {
    version: u8,
    preference: RendererPreference,
    #[serde(deserialize_with = "deserialize_required_option")]
    pending: Option<PendingLaunch>,
    fallback_steps: u8,
    #[serde(deserialize_with = "deserialize_required_option")]
    previous_incomplete_path: Option<RendererPath>,
    failures: Vec<FailedStart>,
}

/// The first format, kept readable so an existing preference survives the upgrade.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedStateV1 {
    version: u8,
    preference: RendererPreference,
    #[serde(deserialize_with = "deserialize_required_option")]
    pending: Option<RendererPath>,
    fallback_steps: u8,
    #[serde(deserialize_with = "deserialize_required_option")]
    previous_incomplete_path: Option<RendererPath>,
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: VERSION,
            preference: RendererPreference::Automatic,
            pending: None,
            fallback_steps: 0,
            previous_incomplete_path: None,
            failures: Vec::new(),
        }
    }
}

impl PersistedState {
    fn validate(&self) -> Result<(), StateError> {
        if self.version != VERSION
            || self.fallback_steps > MAX_CRASH_FALLBACK_STEPS
            || (self.fallback_steps == 0) != self.previous_incomplete_path.is_none()
            || self.failures.len() > MAX_FAILURES
            || (self.preference != RendererPreference::Automatic
                && (self.pending.is_some()
                    || self.fallback_steps != 0
                    || !self.failures.is_empty()))
        {
            return Err(StateError::InvalidRecord);
        }
        Ok(())
    }

    /// The failure history once an unfinished previous launch is counted.
    fn failures_after_pending(&self) -> Vec<FailedStart> {
        let mut failures = self.failures.clone();
        if let Some(pending) = &self.pending {
            failures.push(FailedStart {
                fingerprint: pending.fingerprint.clone(),
                path: pending.path,
            });
        }
        let excess = failures.len().saturating_sub(MAX_FAILURES);
        failures.drain(..excess);
        failures
    }

    fn snapshot(&self, fingerprint: &Fingerprint) -> (StateSnapshot, Vec<FailedStart>) {
        let failures = self.failures_after_pending();
        let mut recovery = CrashRecovery {
            fallback_steps: self.fallback_steps,
            previous_incomplete_path: self.previous_incomplete_path,
            held_path: held_path(&failures, fingerprint),
        };
        if let Some(pending) = &self.pending {
            recovery = recovery.record_incomplete(pending.path);
        }
        let snapshot = StateSnapshot {
            preference: self.preference,
            recovery,
        };
        (snapshot, failures)
    }
}

/// A path that failed to start twice with this fingerprint is not tried again.
fn held_path(failures: &[FailedStart], fingerprint: &Fingerprint) -> Option<RendererPath> {
    [
        RendererPath::Default,
        RendererPath::ForceShm,
        RendererPath::DisableDmabuf,
    ]
    .into_iter()
    .filter(|path| {
        failures
            .iter()
            .filter(|failure| failure.fingerprint == *fingerprint && failure.path == *path)
            .count()
            >= FAILURES_TO_HOLD
    })
    .map(RendererPath::step_down)
    .max()
}

pub(super) struct StateStore {
    namespace: LockedPrivateNamespace,
    record: PersistedState,
    recovery: CrashRecovery,
    failures: Vec<FailedStart>,
    fingerprint: Fingerprint,
}

impl StateStore {
    pub fn open_or_create(
        config_directory: &Path,
        fingerprint: Fingerprint,
    ) -> Result<Self, StateError> {
        ensure_config_directory(config_directory)?;
        let namespace = LockedPrivateNamespace::open_or_create(config_directory.join(NAMESPACE))?;
        let record = read_record(&namespace, &fingerprint)?;
        let (snapshot, failures) = record.snapshot(&fingerprint);
        Ok(Self {
            namespace,
            record,
            recovery: snapshot.recovery,
            failures,
            fingerprint,
        })
    }

    // Inspection never creates storage or records a browser launch.
    pub fn read_existing(
        config_directory: &Path,
        fingerprint: &Fingerprint,
    ) -> Result<StateSnapshot, StateError> {
        validate_config_path(config_directory)?;
        let namespace =
            match LockedPrivateNamespace::open_existing(config_directory.join(NAMESPACE)) {
                Ok(namespace) => namespace,
                Err(PrivateFsError::NotFound) => return Ok(StateSnapshot::default()),
                Err(error) => return Err(error.into()),
            };
        Ok(read_record(&namespace, fingerprint)?
            .snapshot(fingerprint)
            .0)
    }

    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            preference: self.record.preference,
            recovery: self.recovery,
        }
    }

    // Call only for the final automatic browser process, after any GLES re-exec.
    pub fn record_automatic_attempt(
        &mut self,
        path: RendererPath,
        recovery: CrashRecovery,
    ) -> Result<(), StateError> {
        if self.record.preference != RendererPreference::Automatic {
            return Err(StateError::NotAutomatic);
        }
        let mut next = self.record.clone();
        next.pending = Some(PendingLaunch {
            path,
            fingerprint: self.fingerprint.clone(),
        });
        next.fallback_steps = recovery.fallback_steps;
        next.previous_incomplete_path = recovery.previous_incomplete_path;
        next.failures = self.failures.clone();
        self.persist(next, recovery)
    }

    /// A start that showed its window clears this launch's step-down, and the
    /// failures of the path it used, but not those of a faster path it avoided.
    pub fn window_shown(&mut self) -> Result<(), StateError> {
        if self.record.pending.is_none() && self.record.fallback_steps == 0 {
            return Ok(());
        }
        let mut next = self.record.clone();
        if let Some(pending) = next.pending.take() {
            next.failures.retain(|failure| {
                failure.fingerprint != pending.fingerprint || failure.path != pending.path
            });
        }
        next.fallback_steps = 0;
        next.previous_incomplete_path = None;
        let recovery = CrashRecovery {
            held_path: self.recovery.held_path,
            ..CrashRecovery::default()
        };
        self.persist(next, recovery)?;
        self.failures = self.record.failures.clone();
        Ok(())
    }

    /// Forgets this fingerprint's failures, so the next automatic start tries
    /// the faster path again. This launch keeps the path it started with.
    pub fn retry_faster(&mut self) -> Result<(), StateError> {
        if self.record.preference != RendererPreference::Automatic {
            return Err(StateError::NotAutomatic);
        }
        let mut next = self.record.clone();
        next.failures = self.failures.clone();
        next.failures
            .retain(|failure| failure.fingerprint != self.fingerprint);
        next.fallback_steps = 0;
        next.previous_incomplete_path = None;
        self.persist(next, self.recovery)?;
        self.failures = self.record.failures.clone();
        Ok(())
    }

    pub fn set_preference(&mut self, preference: RendererPreference) -> Result<(), StateError> {
        if self.record.preference == preference {
            return Ok(());
        }
        self.persist(
            PersistedState {
                preference,
                ..PersistedState::default()
            },
            CrashRecovery::default(),
        )?;
        self.failures.clear();
        Ok(())
    }

    fn persist(&mut self, next: PersistedState, recovery: CrashRecovery) -> Result<(), StateError> {
        next.validate()?;
        let bytes = serde_json::to_vec(&next).map_err(|_| StateError::InvalidRecord)?;
        let directory = self.namespace.directory();
        let staging = component(STAGING)?;
        directory.remove_verified_regular(&staging)?;
        directory.write_new_synced(&staging, &bytes, limit()?)?;
        directory.replace_verified_regular(&staging, &component(RECORD)?)?;
        self.record = next;
        self.recovery = recovery;
        Ok(())
    }
}

fn read_record(
    namespace: &LockedPrivateNamespace,
    fingerprint: &Fingerprint,
) -> Result<PersistedState, StateError> {
    let Some(bytes) = namespace
        .directory()
        .read_bounded_regular(&component(RECORD)?, limit()?)?
    else {
        return Ok(PersistedState::default());
    };
    let record = match serde_json::from_slice::<PersistedState>(&bytes) {
        Ok(record) => record,
        Err(_) => {
            let old: PersistedStateV1 =
                serde_json::from_slice(&bytes).map_err(|_| StateError::InvalidRecord)?;
            if old.version != 1 {
                return Err(StateError::InvalidRecord);
            }
            // The first format did not say which hardware an unfinished launch used.
            PersistedState {
                version: VERSION,
                preference: old.preference,
                pending: old.pending.map(|path| PendingLaunch {
                    path,
                    fingerprint: fingerprint.clone(),
                }),
                fallback_steps: old.fallback_steps,
                previous_incomplete_path: old.previous_incomplete_path,
                failures: Vec::new(),
            }
        }
    };
    record.validate()?;
    Ok(record)
}

fn component(name: &'static str) -> Result<PrivateComponent, StateError> {
    PrivateComponent::new(name).map_err(|_| StateError::InvalidRecord)
}

fn limit() -> Result<ByteLimit, StateError> {
    ByteLimit::new(MAX_STATE_BYTES).map_err(Into::into)
}

pub(super) fn resolve_config_directory(identity: &str) -> Result<PathBuf, StateError> {
    resolve_config_directory_from(
        identity,
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

fn resolve_config_directory_from(
    identity: &str,
    config_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, StateError> {
    if identity.is_empty()
        || identity.len() > 128
        || matches!(identity, "." | "..")
        || !identity
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(StateError::ConfigUnavailable);
    }
    let directory = config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".config"))
        })
        .ok_or(StateError::ConfigUnavailable)?
        .join(identity);
    validate_config_path(&directory)?;
    Ok(directory)
}

fn validate_config_path(path: &Path) -> Result<(), StateError> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path.components().count() > MAX_CONFIG_DEPTH
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(StateError::ConfigUnavailable);
    }
    Ok(())
}

fn ensure_config_directory(path: &Path) -> Result<(), StateError> {
    validate_config_path(path)?;
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(PrivateFsError::Unsafe.into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::DirBuilder::new().mode(0o700).create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = fs::symlink_metadata(&current)
                            .map_err(|_| StateError::Filesystem(PrivateFsError::Io))?;
                        if !metadata.is_dir() || metadata.file_type().is_symlink() {
                            return Err(PrivateFsError::Unsafe.into());
                        }
                    }
                    Err(_) => return Err(PrivateFsError::Io.into()),
                }
            }
            Err(_) => return Err(PrivateFsError::Io.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

    fn config_directory(parent: &tempfile::TempDir) -> PathBuf {
        parent.path().join("config/app.zephium.dev")
    }

    fn amd() -> Fingerprint {
        Fingerprint::of(&[
            "MesaRadeonsi",
            "AMD Radeon RX 6600",
            "25.2.4",
            "2.54.2",
            "Wayland",
        ])
    }

    fn open(directory: &Path) -> Result<StateStore, StateError> {
        StateStore::open_or_create(directory, amd())
    }

    fn read(directory: &Path) -> Result<StateSnapshot, StateError> {
        StateStore::read_existing(directory, &amd())
    }

    fn write_record(directory: &Path, bytes: &[u8]) {
        let path = directory.join(NAMESPACE).join(RECORD);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn inspection_of_absent_storage_creates_nothing() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        assert_eq!(read(&directory).unwrap(), StateSnapshot::default());
        assert!(!parent.path().join("config").exists());
    }

    #[test]
    fn explicit_preferences_round_trip_without_launch_markers() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        for preference in [
            RendererPreference::Gpu,
            RendererPreference::Compatibility,
            RendererPreference::Safe,
            RendererPreference::Automatic,
        ] {
            let mut store = open(&directory).unwrap();
            store.set_preference(preference).unwrap();
            assert_eq!(store.snapshot().preference, preference);
            if preference != RendererPreference::Automatic {
                assert_eq!(
                    store.record_automatic_attempt(RendererPath::Default, CrashRecovery::default()),
                    Err(StateError::NotAutomatic)
                );
            }
            drop(store);
            let state = read(&directory).unwrap();
            assert_eq!(state.preference, preference);
            assert_eq!(state.recovery, CrashRecovery::default());
        }
        assert_eq!(
            fs::metadata(directory.join(NAMESPACE).join(RECORD))
                .unwrap()
                .mode()
                & 0o7777,
            0o600
        );
        assert_eq!(
            fs::metadata(directory.join(NAMESPACE)).unwrap().mode() & 0o7777,
            0o700
        );
    }

    #[test]
    fn failed_automatic_launches_step_down_and_success_retries_policy() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut store = open(&directory).unwrap();
        store
            .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
            .unwrap();
        assert_eq!(store.snapshot().recovery, CrashRecovery::default());
        drop(store);
        for (expected_steps, previous, next) in [
            (1, RendererPath::Default, RendererPath::ForceShm),
            (2, RendererPath::ForceShm, RendererPath::DisableDmabuf),
            (2, RendererPath::DisableDmabuf, RendererPath::DisableDmabuf),
        ] {
            let mut store = open(&directory).unwrap();
            let recovery = store.snapshot().recovery;
            assert_eq!(recovery.fallback_steps, expected_steps);
            assert_eq!(recovery.previous_incomplete_path, Some(previous));
            store.record_automatic_attempt(next, recovery).unwrap();
        }
        let mut store = open(&directory).unwrap();
        store.window_shown().unwrap();
        store.window_shown().unwrap();
        drop(store);
        assert_eq!(read(&directory).unwrap().recovery, CrashRecovery::default());
    }

    #[test]
    fn concurrent_launch_cannot_modify_or_consume_the_first_marker() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut first = open(&directory).unwrap();
        first
            .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
            .unwrap();
        assert!(matches!(
            open(&directory),
            Err(StateError::Filesystem(PrivateFsError::LockUnavailable))
        ));
        assert_eq!(
            read(&directory),
            Err(StateError::Filesystem(PrivateFsError::LockUnavailable))
        );
        first.window_shown().unwrap();
        drop(first);
        assert_eq!(read(&directory).unwrap().recovery, CrashRecovery::default());
    }

    #[test]
    fn report_only_reads_do_not_accumulate_failures() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut store = open(&directory).unwrap();
        store
            .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
            .unwrap();
        drop(store);
        let before = fs::read(directory.join(NAMESPACE).join(RECORD)).unwrap();
        for _ in 0..3 {
            assert_eq!(read(&directory).unwrap().recovery.fallback_steps, 1);
        }
        assert_eq!(
            fs::read(directory.join(NAMESPACE).join(RECORD)).unwrap(),
            before
        );
    }

    #[test]
    fn changed_preference_clears_automatic_recovery() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut store = open(&directory).unwrap();
        store
            .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
            .unwrap();
        store.set_preference(RendererPreference::Safe).unwrap();
        assert_eq!(store.snapshot().recovery, CrashRecovery::default());
        store.set_preference(RendererPreference::Automatic).unwrap();
        assert_eq!(store.snapshot(), StateSnapshot::default());
    }

    #[test]
    fn malformed_unknown_and_unbounded_records_are_refused() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        drop(open(&directory).unwrap());
        for bytes in [
            b"".as_slice(),
            b"{}",
            br#"{"version":2,"preference":"automatic","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[],"extra":true}"#,
            br#"{"version":2,"preference":"automatic","fallback_steps":0,"previous_incomplete_path":null,"failures":[]}"#,
            br#"{"version":3,"preference":"automatic","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[]}"#,
            br#"{"version":2,"preference":"automatic","pending":null,"fallback_steps":3,"previous_incomplete_path":"default","failures":[]}"#,
            br#"{"version":2,"preference":"automatic","pending":null,"fallback_steps":1,"previous_incomplete_path":null,"failures":[]}"#,
            br#"{"version":2,"preference":"safe","pending":{"path":"default","fingerprint":"0123456789abcdef"},"fallback_steps":0,"previous_incomplete_path":null,"failures":[]}"#,
            br#"{"version":2,"preference":"safe","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[{"fingerprint":"0123456789abcdef","path":"default"}]}"#,
            br#"{"version":2,"preference":"invented","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[]}"#,
            br#"{"version":2,"version":2,"preference":"automatic","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[]}"#,
            br#"{"version":2,"preference":"automatic","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[{"fingerprint":"ABCDEF0123456789","path":"default"}]}"#,
            br#"{"version":2,"preference":"automatic","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[{"fingerprint":"0123","path":"default"}]}"#,
            br#"{"version":2,"preference":"automatic","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[{"fingerprint":"0123456789abcdef","path":"default","when":1}]}"#,
            br#"{"version":2,"preference":"automatic","pending":{"path":"default"},"fallback_steps":0,"previous_incomplete_path":null,"failures":[]}"#,
            br#"{"version":1,"preference":"automatic","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"extra":true}"#,
            br#"{"version":1,"preference":"automatic","pending":null,"fallback_steps":1,"previous_incomplete_path":null}"#,
            br#"{"version":2,"preference":"automatic","pending":null,"fallback_steps":0,"previous_incomplete_path":null,"failures":[{"fingerprint":"0123456789abcdef","path":"force_shm"},{"fingerprint":"0123456789abcdef","path":"force_shm"},{"fingerprint":"0123456789abcdef","path":"force_shm"},{"fingerprint":"0123456789abcdef","path":"force_shm"},{"fingerprint":"0123456789abcdef","path":"force_shm"},{"fingerprint":"0123456789abcdef","path":"force_shm"},{"fingerprint":"0123456789abcdef","path":"force_shm"},{"fingerprint":"0123456789abcdef","path":"force_shm"},{"fingerprint":"0123456789abcdef","path":"force_shm"}]}"#,
        ] {
            write_record(&directory, bytes);
            assert!(matches!(open(&directory), Err(StateError::InvalidRecord)));
        }
        write_record(&directory, &vec![b' '; MAX_STATE_BYTES + 1]);
        assert!(matches!(
            open(&directory),
            Err(StateError::Filesystem(PrivateFsError::BoundExceeded))
        ));
    }

    #[test]
    fn interrupted_staging_is_replaced_without_changing_committed_state() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut store = open(&directory).unwrap();
        store.set_preference(RendererPreference::Gpu).unwrap();
        drop(store);
        let staging = directory.join(NAMESPACE).join(STAGING);
        fs::write(&staging, b"interrupted").unwrap();
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o600)).unwrap();
        let mut store = open(&directory).unwrap();
        assert_eq!(store.snapshot().preference, RendererPreference::Gpu);
        store.set_preference(RendererPreference::Safe).unwrap();
        assert!(!staging.exists());
    }

    #[test]
    fn unsafe_record_or_staging_does_not_claim_a_preference_write_succeeded() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut store = open(&directory).unwrap();
        store.set_preference(RendererPreference::Gpu).unwrap();
        let target = parent.path().join("untouched");
        fs::write(&target, b"preserve").unwrap();
        symlink(&target, directory.join(NAMESPACE).join(STAGING)).unwrap();
        assert!(store.set_preference(RendererPreference::Safe).is_err());
        assert_eq!(store.snapshot().preference, RendererPreference::Gpu);
        assert_eq!(fs::read(target).unwrap(), b"preserve");
    }

    #[test]
    fn records_with_links_or_public_permissions_are_refused() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut store = open(&directory).unwrap();
        store.set_preference(RendererPreference::Gpu).unwrap();
        drop(store);
        let record = directory.join(NAMESPACE).join(RECORD);
        let link = parent.path().join("linked-record");
        fs::hard_link(&record, &link).unwrap();
        assert!(matches!(
            read(&directory),
            Err(StateError::Filesystem(PrivateFsError::Unsafe))
        ));
        fs::remove_file(link).unwrap();
        fs::set_permissions(&record, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            read(&directory),
            Err(StateError::Filesystem(PrivateFsError::Unsafe))
        ));
    }

    #[test]
    fn symlinked_config_ancestor_is_refused_before_creating_children() {
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("target");
        fs::create_dir(&target).unwrap();
        let linked = parent.path().join("linked");
        symlink(&target, &linked).unwrap();
        assert!(matches!(
            StateStore::open_or_create(&linked.join("app.zephium.dev"), amd()),
            Err(StateError::Filesystem(PrivateFsError::Unsafe))
        ));
        assert!(!target.join("app.zephium.dev").exists());
    }

    #[test]
    fn configuration_follows_xdg_and_keeps_app_identities_separate() {
        let development = resolve_config_directory_from(
            "app.zephium.dev",
            Some("/xdg/config".into()),
            Some("/home/test".into()),
        )
        .unwrap();
        assert_eq!(development, Path::new("/xdg/config/app.zephium.dev"));
        assert_ne!(
            development,
            resolve_config_directory_from("app.zephium", Some("/xdg/config".into()), None).unwrap()
        );
        assert_eq!(
            resolve_config_directory_from(
                "app.zephium.dev",
                Some("relative".into()),
                Some("/home/test".into())
            )
            .unwrap(),
            Path::new("/home/test/.config/app.zephium.dev")
        );
        for identity in ["", ".", "..", "../../elsewhere", "id/child"] {
            assert_eq!(
                resolve_config_directory_from(identity, Some("/config".into()), None),
                Err(StateError::ConfigUnavailable)
            );
        }
        assert_eq!(
            resolve_config_directory_from("app.zephium", None, None),
            Err(StateError::ConfigUnavailable)
        );
    }

    #[test]
    fn configuration_and_failure_messages_do_not_expose_paths() {
        assert_eq!(
            validate_config_path(Path::new("/config/../elsewhere")),
            Err(StateError::ConfigUnavailable)
        );
        for error in [
            StateError::ConfigUnavailable,
            StateError::InvalidRecord,
            StateError::NotAutomatic,
            StateError::Filesystem(PrivateFsError::Unsafe),
        ] {
            let text = error.to_string();
            assert!(!text.contains('/'));
            assert!(!text.contains("home"));
        }
    }

    #[test]
    fn a_second_failure_with_the_same_graphics_holds_the_slower_path() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        // Launch 1 crashes on the fast path.
        let mut store = open(&directory).unwrap();
        store
            .record_automatic_attempt(RendererPath::Default, store.snapshot().recovery)
            .unwrap();
        drop(store);
        // Launch 2 steps down once and starts.
        let mut store = open(&directory).unwrap();
        let recovery = store.snapshot().recovery;
        assert_eq!(recovery.fallback_steps, 1);
        assert_eq!(recovery.held_path, None);
        store
            .record_automatic_attempt(RendererPath::ForceShm, recovery)
            .unwrap();
        store.window_shown().unwrap();
        drop(store);
        // Launch 3 tries the fast path again, as today, and crashes again.
        let mut store = open(&directory).unwrap();
        let recovery = store.snapshot().recovery;
        assert_eq!(recovery, CrashRecovery::default());
        store
            .record_automatic_attempt(RendererPath::Default, recovery)
            .unwrap();
        drop(store);
        // From launch 4 on, the slower path holds across successful starts.
        for _ in 0..3 {
            let mut store = open(&directory).unwrap();
            let recovery = store.snapshot().recovery;
            assert_eq!(recovery.held_path, Some(RendererPath::ForceShm));
            store
                .record_automatic_attempt(RendererPath::ForceShm, recovery)
                .unwrap();
            store.window_shown().unwrap();
        }
        assert_eq!(
            read(&directory).unwrap().recovery.held_path,
            Some(RendererPath::ForceShm)
        );
    }

    #[test]
    fn a_single_failure_followed_by_a_good_fast_start_is_forgotten() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut store = open(&directory).unwrap();
        store
            .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
            .unwrap();
        drop(store);
        let mut store = open(&directory).unwrap();
        store
            .record_automatic_attempt(RendererPath::ForceShm, store.snapshot().recovery)
            .unwrap();
        store.window_shown().unwrap();
        drop(store);
        let mut store = open(&directory).unwrap();
        store
            .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
            .unwrap();
        store.window_shown().unwrap();
        drop(store);
        // One more failure is again only the first.
        let mut store = open(&directory).unwrap();
        store
            .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
            .unwrap();
        drop(store);
        assert_eq!(read(&directory).unwrap().recovery.held_path, None);
    }

    #[test]
    fn new_graphics_retry_and_chosen_modes_release_the_hold() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let fail_twice = |directory: &Path| {
            for _ in 0..2 {
                let mut store = open(directory).unwrap();
                store
                    .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
                    .unwrap();
                drop(store);
                let mut store = open(directory).unwrap();
                store
                    .record_automatic_attempt(RendererPath::ForceShm, store.snapshot().recovery)
                    .unwrap();
                store.window_shown().unwrap();
            }
        };
        fail_twice(&directory);
        assert_eq!(
            read(&directory).unwrap().recovery.held_path,
            Some(RendererPath::ForceShm)
        );
        // A driver update is different graphics; the old hold stays with the old driver.
        let updated = Fingerprint::of(&[
            "MesaRadeonsi",
            "AMD Radeon RX 6600",
            "25.3.0",
            "2.54.2",
            "Wayland",
        ]);
        assert_ne!(updated, amd());
        assert_eq!(
            StateStore::read_existing(&directory, &updated)
                .unwrap()
                .recovery
                .held_path,
            None
        );
        let x11 = Fingerprint::of(&[
            "MesaRadeonsi",
            "AMD Radeon RX 6600",
            "25.2.4",
            "2.54.2",
            "X11",
        ]);
        assert_ne!(x11, amd());

        let mut store = open(&directory).unwrap();
        store.retry_faster().unwrap();
        // This launch keeps what it started with; the next one tries again.
        assert_eq!(
            store.snapshot().recovery.held_path,
            Some(RendererPath::ForceShm)
        );
        drop(store);
        assert_eq!(read(&directory).unwrap().recovery.held_path, None);

        fail_twice(&directory);
        let mut store = open(&directory).unwrap();
        store.set_preference(RendererPreference::Gpu).unwrap();
        store.set_preference(RendererPreference::Automatic).unwrap();
        drop(store);
        assert_eq!(read(&directory).unwrap().recovery, CrashRecovery::default());

        let mut store = open(&directory).unwrap();
        store.set_preference(RendererPreference::Safe).unwrap();
        assert_eq!(store.retry_faster(), Err(StateError::NotAutomatic));
    }

    #[test]
    fn failure_history_is_bounded_to_the_most_recent_starts() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        for index in 0..(MAX_FAILURES * 2) {
            let other = Fingerprint::of(&["Unknown", &index.to_string()]);
            let mut store = StateStore::open_or_create(&directory, other).unwrap();
            store
                .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
                .unwrap();
        }
        let store = open(&directory).unwrap();
        assert_eq!(store.failures.len(), MAX_FAILURES);
        drop(store);
        assert!(
            fs::metadata(directory.join(NAMESPACE).join(RECORD))
                .unwrap()
                .len()
                < MAX_STATE_BYTES as u64
        );
    }

    #[test]
    fn a_concurrent_launch_cannot_count_a_failure_for_a_running_one() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        let mut first = open(&directory).unwrap();
        first
            .record_automatic_attempt(RendererPath::Default, CrashRecovery::default())
            .unwrap();
        for _ in 0..3 {
            assert!(matches!(
                open(&directory),
                Err(StateError::Filesystem(PrivateFsError::LockUnavailable))
            ));
        }
        first.window_shown().unwrap();
        drop(first);
        let store = open(&directory).unwrap();
        assert!(store.failures.is_empty());
        assert_eq!(store.snapshot().recovery, CrashRecovery::default());
    }

    #[test]
    fn a_first_format_record_keeps_its_preference_and_unfinished_launch() {
        let parent = tempfile::tempdir().unwrap();
        let directory = config_directory(&parent);
        drop(open(&directory).unwrap());
        write_record(
            &directory,
            br#"{"version":1,"preference":"automatic","pending":"default","fallback_steps":0,"previous_incomplete_path":null}"#,
        );
        let store = open(&directory).unwrap();
        assert_eq!(store.snapshot().recovery.fallback_steps, 1);
        assert_eq!(store.failures.len(), 1);
        drop(store);
        write_record(
            &directory,
            br#"{"version":1,"preference":"compatibility","pending":null,"fallback_steps":0,"previous_incomplete_path":null}"#,
        );
        assert_eq!(
            read(&directory).unwrap().preference,
            RendererPreference::Compatibility
        );
    }

    #[test]
    fn fingerprints_are_stable_hex_and_change_with_each_fact() {
        let base = [
            "MesaRadeonsi",
            "AMD Radeon RX 6600",
            "25.2.4",
            "2.54.2",
            "Wayland",
        ];
        let fingerprint = Fingerprint::of(&base);
        assert_eq!(fingerprint, Fingerprint::of(&base));
        assert_eq!(String::from(fingerprint.clone()).len(), 16);
        for index in 0..base.len() {
            let mut changed = base;
            changed[index] = "other";
            assert_ne!(Fingerprint::of(&changed), fingerprint);
        }
        // Field boundaries count: moving text between facts is a different fingerprint.
        assert_ne!(Fingerprint::of(&["ab", "c"]), Fingerprint::of(&["a", "bc"]));
    }
}
