use super::*;
use zephium_core::work::{port::*, WorkError};

pub(super) struct Permit(Arc<AtomicUsize>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
impl SqliteStore {
    pub(super) fn dispatch_work_document(
        &self,
        profile: ProfileId,
        request: WorkRequest,
        completion: WorkCompletion,
    ) -> Result<(), WorkError> {
        // Catch foreign callback destructors even on early refusal.
        let mut completion = Some(completion);
        let result = (|| {
            request.validate()?;
            let lifecycle = self
                .lifecycle
                .try_read()
                .map_err(|_| WorkError::Unavailable)?;
            if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
                return Err(WorkError::Shutdown);
            }
            let counter = self
                .work_document_admission
                .get_or_init(|| Arc::new(AtomicUsize::new(0)))
                .clone();
            counter
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    (n < 4).then_some(n + 1)
                })
                .map_err(|_| WorkError::Capacity)?;
            let permit = Permit(counter);
            let callback = completion.take().ok_or(WorkError::Unavailable)?;
            match self.tx.try_send(Cmd::WorkDocument(
                profile,
                Box::new(request),
                permit,
                callback,
            )) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let (error, command) = match error {
                        mpsc::TrySendError::Full(c) => (WorkError::Capacity, c),
                        mpsc::TrySendError::Disconnected(c) => (WorkError::Shutdown, c),
                    };
                    let _ =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(command)));
                    Err(error)
                }
            }
        })();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(completion)));
        result
    }
}
