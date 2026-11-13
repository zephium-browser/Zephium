//! Bounded external-owner release before journal-authorized profile erasure.
use std::sync::{
    mpsc::{sync_channel, SyncSender},
    Arc,
};
use zephium_core::ids::ProfileId;

/// Completion proves every external writer was retired and its artifacts erased.
pub type CleanupDone = Box<dyn FnOnce(bool) + Send>;

/// Injected owner of files and credentials outside the Store actor.
pub trait ProfileCleanup: Send + Sync {
    /// Admits bounded cleanup. Refusal and false completion retain the journal.
    fn release(&self, profile: ProfileId, done: CleanupDone) -> bool;
}

/// Serializes blocking owner cleanup away from the application actor. Retries
/// remain bounded even when an operating-system credential operation stalls.
pub struct CleanupWorker {
    sender: SyncSender<(ProfileId, CleanupDone)>,
}
impl CleanupWorker {
    /// Starts one worker with a bounded profile-sized mailbox.
    pub fn start(clean: impl Fn(ProfileId) -> bool + Send + 'static) -> std::io::Result<Arc<Self>> {
        let (sender, receiver) =
            sync_channel::<(ProfileId, CleanupDone)>(zephium_core::session::MAX_SESSION_PROFILES);
        std::thread::Builder::new()
            .name("profile-cleanup".into())
            .spawn(move || {
                while let Ok((profile, done)) = receiver.recv() {
                    let success =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| clean(profile)))
                            .unwrap_or(false);
                    let _ =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(success)));
                }
            })?;
        Ok(Arc::new(Self { sender }))
    }
}
impl ProfileCleanup for CleanupWorker {
    fn release(&self, profile: ProfileId, done: CleanupDone) -> bool {
        self.sender.try_send((profile, done)).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cleanup_admission_is_bounded_while_an_owner_is_blocked_and_failures_do_not_prove_erasure() {
        let (entered, waiting) = std::sync::mpsc::channel();
        let (resume, gate) = std::sync::mpsc::channel();
        let first = std::sync::atomic::AtomicBool::new(true);
        let worker = CleanupWorker::start(move |_| {
            if first.swap(false, std::sync::atomic::Ordering::AcqRel) {
                entered.send(()).unwrap();
                gate.recv().unwrap();
            }
            false
        })
        .unwrap();
        let (done, proof) = std::sync::mpsc::channel();
        let first_done = done.clone();
        assert!(worker.release(
            ProfileId::from(1),
            Box::new(move |success| first_done.send(success).unwrap())
        ));
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..zephium_core::session::MAX_SESSION_PROFILES {
            let done = done.clone();
            assert!(worker.release(
                ProfileId::from(1),
                Box::new(move |success| done.send(success).unwrap())
            ));
        }
        let overflow = worker.release(
            ProfileId::from(1),
            Box::new(|_| panic!("refused work cannot complete")),
        );
        resume.send(()).unwrap();
        assert!(!overflow);
        for _ in 0..=zephium_core::session::MAX_SESSION_PROFILES {
            assert!(!proof.recv_timeout(Duration::from_secs(5)).unwrap());
        }
    }
}
