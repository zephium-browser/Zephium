//! Excluded cleanup of an exact newly-created public-test profile. This is not
//! a shipping admission/deletion adapter or a bypass of Shell bootstrap policy.
//! It uses the existing durable authorization, native absence and finalization
//! ports because the no-UI qualifier deliberately has no Browse bootstrap.

use std::{
    io::Write as _,
    path::Path,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use zephium_core::{
    ids::ProfileId,
    ports::{
        engine::{Engine, ProfileDataErasureOutcome},
        store::{
            ProfileDeletionAuthorizeOutcome, ProfileDeletionFinalizeOutcome, ProfileDeletionLoad,
            SessionLoad, Store, StoreShutdownOutcome,
        },
    },
    profiles::ProfileKind,
};

pub(super) struct Cleanup {
    store: Arc<zephium_store::SqliteStore>,
    profile: ProfileId,
    native: Arc<AtomicU8>,
    deadline: Instant,
    terminal: Option<bool>,
}

impl Cleanup {
    pub(super) fn start(
        store: Arc<zephium_store::SqliteStore>,
        engine: Arc<zephium_engine::WebviewEngine>,
        profile: ProfileId,
    ) -> Self {
        let native = Arc::new(AtomicU8::new(0));
        let deadline = Instant::now() + Duration::from_secs(12);
        let authorized = authorize(&store, profile);
        match authorized {
            Ok(Authorization::Native) => {
                let result = native.clone();
                engine.erase_profile_data(
                    profile,
                    Vec::new(),
                    Box::new(move |outcome| {
                        result.store(
                            match outcome {
                                ProfileDataErasureOutcome::Verified => 1,
                                ProfileDataErasureOutcome::Failed => 2,
                                ProfileDataErasureOutcome::TimedOut => 3,
                            },
                            Ordering::Release,
                        );
                    }),
                );
            }
            Ok(Authorization::Verified) => native.store(1, Ordering::Release),
            Err(()) => native.store(4, Ordering::Release),
        }
        Self {
            store,
            profile,
            native,
            deadline,
            terminal: None,
        }
    }

    pub(super) fn poll(&mut self) -> Option<bool> {
        if let Some(clean) = self.terminal {
            return Some(clean);
        }
        let native = self.native.load(Ordering::Acquire);
        if native == 0 && Instant::now() < self.deadline {
            return None;
        }
        let clean = native == 1
            && self
                .store
                .finalize_profile_deletion(self.profile, self.deadline)
                == ProfileDeletionFinalizeOutcome::Completed;
        self.terminal = Some(clean);
        let stage = match native {
            0 => "callback_pending",
            1 => "native_verified",
            2 => "native_failed",
            3 => "native_timeout",
            _ => "authorization_refused",
        };
        let _ = writeln!(std::io::stdout().lock(), "work-artifact-profile-cleanup: stage={stage}; native_absence_verified={}; store_finalized={clean}; content=redacted", native == 1);
        Some(clean)
    }
}

enum Authorization {
    Native,
    Verified,
}

fn authorize(store: &zephium_store::SqliteStore, profile: ProfileId) -> Result<Authorization, ()> {
    let ProfileDeletionLoad::Loaded(pending) = store.pending_profile_deletions() else {
        return Err(());
    };
    if let [pending] = pending.as_slice() {
        if pending.profile != profile {
            return Err(());
        }
        return Ok(if pending.native_erasure_verified {
            Authorization::Verified
        } else {
            Authorization::Native
        });
    }
    if !pending.is_empty() {
        return Err(());
    }
    let SessionLoad::Loaded { mut state, .. } = store.load_session() else {
        return Err(());
    };
    if state.profiles.len() != 2
        || !state.profiles.iter().any(|candidate| {
            candidate.id == profile
                && candidate.kind == ProfileKind::Named
                && candidate.name == "Public Work test"
        })
        || !state.items.is_empty()
        || !state.recently_closed.is_empty()
    {
        return Err(());
    }
    state.profiles.retain(|candidate| candidate.id != profile);
    state.spaces.retain(|space| space.profile != profile);
    if !matches!(
        store.authorize_profile_deletion(profile, state, Instant::now() + Duration::from_secs(2)),
        ProfileDeletionAuthorizeOutcome::Authorized
            | ProfileDeletionAuthorizeOutcome::AlreadyAuthorized
    ) {
        return Err(());
    }
    let ProfileDeletionLoad::Loaded(pending) = store.pending_profile_deletions() else {
        return Err(());
    };
    let [pending] = pending.as_slice() else {
        return Err(());
    };
    if pending.profile != profile || pending.native_erasure_verified {
        return Err(());
    }
    Ok(Authorization::Native)
}

pub(super) fn recover(directory: &Path) -> Result<(), super::ProbeFailure> {
    use super::ProbeFailure as Error;
    if !directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("zephium-public-work-"))
    {
        return Err(Error::Authority);
    }
    let store = Arc::new(zephium_store::SqliteStore::open(directory).map_err(|_| Error::Runtime)?);
    let SessionLoad::Loaded { state, .. } = store.load_session() else {
        return Err(Error::Authority);
    };
    let ProfileDeletionLoad::Loaded(pending) = store.pending_profile_deletions() else {
        return Err(Error::Authority);
    };
    if !state.profiles.iter().any(|profile| {
        profile.kind == ProfileKind::Default && profile.name == "Public test default"
    }) {
        return Err(Error::Authority);
    }
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.kind == ProfileKind::Named && profile.name == "Public Work test")
        .map(|profile| profile.id)
        .or(match pending.as_slice() {
            [pending] => Some(pending.profile),
            _ => None,
        })
        .ok_or(Error::Authority)?;
    zephium_engine::run_macos_agentic_work_application_probe(profile, move |engine| {
        let mut cleanup = Cleanup::start(store.clone(), engine.clone(), profile);
        let closed = Arc::new(AtomicU8::new(0));
        let mut shutdown = false;
        Ok(Box::new(move |failed| {
            if failed {
                return Some(Err("artifact_cleanup_native_host"));
            }
            let clean = cleanup.poll()?;
            if !clean {
                return Some(Err("artifact_cleanup_authority"));
            }
            if !shutdown {
                if store.shutdown_until(Instant::now() + Duration::from_secs(2))
                    != StoreShutdownOutcome::Clean
                {
                    return Some(Err("artifact_cleanup_store_shutdown"));
                }
                shutdown = true;
                let result = closed.clone();
                engine.shutdown(Box::new(move |clean| {
                    result.store(if clean { 1 } else { 2 }, Ordering::Release)
                }));
            }
            match closed.load(Ordering::Acquire) {
                1 => Some(Ok(())),
                2 => Some(Err("artifact_cleanup_engine_shutdown")),
                _ => None,
            }
        }))
    })
    .map_err(Error::Engine)
}
