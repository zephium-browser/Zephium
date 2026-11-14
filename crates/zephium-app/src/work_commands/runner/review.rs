//! The worker owns the entire copied workspace, including acknowledged cleanup.
//! Granted folders are read-only inputs. Cancellation is checked before each
//! child admission; no worker publishes changes back to a granted folder.
use super::worker::{self, Control, Limits};
#[cfg(test)]
use super::worker_slots::Permit;
use super::worker_slots::Slots;
#[cfg(not(target_os = "macos"))]
use super::GitOperation;
#[cfg(any(test, not(target_os = "macos")))]
use super::Snapshot;
use super::{ApprovedChange, Failure, Output, ReadOperation};
use crate::work_files::WorkFileGrant;
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc, OnceLock},
    time::Duration,
};
#[cfg(test)]
const CLEANUP_GRACE: Duration = Duration::from_millis(300);
fn slots() -> &'static Arc<Slots> {
    static SLOTS: OnceLock<Arc<Slots>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Slots::new(4)))
}
#[cfg(test)]
fn launch_owned(
    permit: Permit,
    work: impl FnOnce() -> Result<Output, Failure> + Send + 'static,
) -> tokio::task::JoinHandle<Result<Output, Failure>> {
    super::worker_slots::launch(permit, work, |result| {
        use super::worker::WorkerError;
        !result.as_ref().is_err_and(|failure| failure.unsettled())
    })
}

pub(super) async fn run<C, CF>(
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
    super::ensure_available()?;
    if changes.is_empty() || changes.len() > 2_000 {
        return Err(Failure::Input);
    }
    let (grant, root, changes) = (grant.clone(), root.to_owned(), changes.to_vec());
    worker::run(
        slots(),
        &cancelled,
        Limits::standard(Duration::from_secs(90)),
        move |control| async move { owned(grant, root, changes, operation, &control).await },
    )
    .await
}

#[cfg(target_os = "macos")]
async fn owned(
    grant: WorkFileGrant,
    root: PathBuf,
    changes: Vec<ApprovedChange>,
    operation: ReadOperation,
    control: &Control,
) -> Result<Output, Failure> {
    control.check()?;
    if grant
        .resolve(root.to_str().ok_or(Failure::Input)?, false)
        .map_err(|_| Failure::Input)?
        != root
    {
        return Err(Failure::Input);
    }
    let selected: Vec<_> = changes.iter().map(|change| change.path.clone()).collect();
    // Only bytes cross to the guard. No copied directory exists in the host,
    // so a host crash before handoff cannot orphan a prepared workspace.
    let after = super::snapshot::collect_checked(&root, &selected, &|| control.check())?;
    let mut before = after.clone();
    let mut seen = std::collections::BTreeSet::new();
    for change in &changes {
        control.check()?;
        if !seen.insert(&change.path) || after.get(&change.path) != Some(&change.after) {
            return Err(Failure::Changed);
        }
        match &change.before {
            Some(bytes) => {
                before.insert(change.path.clone(), bytes.clone());
            }
            None => {
                before.remove(&change.path);
            }
        }
    }
    super::snapshot::Snapshot::validate_files(&before, &|| control.check())?;
    control.native();
    let output = super::guard::git(&before, &after, operation, &|| async {
        control.cancelled.load(Ordering::Acquire)
    })
    .await?;
    control.filesystem();
    for change in &changes {
        control.check()?;
        if crate::work_files::snapshot_bytes(&root.join(&change.path), 1024 * 1024)
            .ok()
            .as_ref()
            != Some(&change.after)
        {
            return Err(Failure::Changed);
        }
    }
    control.check()?;
    Ok(output)
}

#[cfg(not(target_os = "macos"))]
async fn owned(
    grant: WorkFileGrant,
    root: PathBuf,
    changes: Vec<ApprovedChange>,
    operation: ReadOperation,
    control: &Control,
) -> Result<Output, Failure> {
    control.check()?;
    let selected: Vec<_> = changes.iter().map(|change| change.path.clone()).collect();
    let snapshot = Snapshot::capture_checked(&grant, &root, &selected, &|| control.check())?;
    let result = inspect(&snapshot, &root, &changes, operation, control).await;
    control.filesystem();
    // Explicit close is the acknowledgement. Best-effort Drop is never used
    // as evidence that cancellation finished removing the private copy.
    let closed = snapshot.close();
    worker::closed(result, closed)
}
#[cfg(not(target_os = "macos"))]
async fn inspect(
    snapshot: &Snapshot,
    root: &Path,
    changes: &[ApprovedChange],
    operation: ReadOperation,
    control: &Control,
) -> Result<Output, Failure> {
    super::prepare_baseline_checked(snapshot, changes, &|| control.check())?;
    for operation in [
        GitOperation::Initialize,
        GitOperation::CaptureIndex,
        GitOperation::CommitBaseline,
    ] {
        control.check()?;
        control.native();
        if !super::execute(snapshot, operation, &|| async {
            control.cancelled.load(Ordering::Acquire)
        })
        .await?
        .success
        {
            return Err(Failure::Failed);
        }
    }
    control.filesystem();
    for change in changes {
        control.check()?;
        snapshot.write(&change.path, &change.after)?;
    }
    control.check()?;
    control.native();
    if !super::execute(snapshot, GitOperation::IntentToAdd, &|| async {
        control.cancelled.load(Ordering::Acquire)
    })
    .await?
    .success
    {
        return Err(Failure::Failed);
    }
    control.check()?;
    let output = super::execute(snapshot, GitOperation::Read(operation), &|| async {
        control.cancelled.load(Ordering::Acquire)
    })
    .await?;
    control.filesystem();
    for change in changes {
        control.check()?;
        if crate::work_files::snapshot_bytes(&root.join(&change.path), 1024 * 1024)
            .ok()
            .as_ref()
            != Some(&change.after)
        {
            return Err(Failure::Changed);
        }
    }
    control.check()?;
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::super::worker::await_worker;
    use super::*;
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize},
        time::Instant,
    };
    fn output() -> Output {
        Output {
            text: String::new(),
            success: true,
            exit: 0,
        }
    }
    #[tokio::test]
    async fn cancellation_waits_for_worker_cleanup_before_reporting_stopped() {
        let control = Arc::new(Control::new(Duration::from_secs(2)));
        let current = control.clone();
        let cleaned = Arc::new(AtomicBool::new(false));
        let ack = cleaned.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let snapshot = Snapshot::fixture(&[("inert.txt", b"inert")]);
            while !current.cancelled.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(2));
            }
            snapshot.close()?;
            ack.store(true, Ordering::Release);
            Err::<Output, Failure>(Failure::Stopped)
        });
        let result = await_worker(
            worker,
            &control,
            &|| async { true },
            Duration::from_secs(2),
            CLEANUP_GRACE,
        )
        .await;
        assert!(matches!(result, Err(Failure::Stopped)));
        assert!(cleaned.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn stalled_prelaunch_filesystem_work_is_pending_then_cancels_without_a_child() {
        let control = Arc::new(Control::new(Duration::from_millis(20)));
        let current = control.clone();
        let launches = Arc::new(AtomicUsize::new(0));
        let launched = launches.clone();
        let cleaned = Arc::new(AtomicBool::new(false));
        let ack = cleaned.clone();
        let slots = Arc::new(Slots::new(1));
        let worker = launch_owned(slots.reserve().unwrap(), move || {
            let snapshot = Snapshot::fixture(&[("inert.txt", b"inert")]);
            // An injected blocking read: no source path outside this owned fixture.
            std::thread::sleep(Duration::from_millis(180));
            let result = current.check().map(|_| {
                launched.fetch_add(1, Ordering::SeqCst);
                output()
            });
            snapshot.close()?;
            ack.store(true, Ordering::Release);
            result
        });
        let began = Instant::now();
        let result = await_worker(
            worker,
            &control,
            &|| async { false },
            Duration::from_secs(2),
            Duration::from_millis(20),
        )
        .await;
        assert!(matches!(result, Err(Failure::CleanupPending)));
        assert!(began.elapsed() < Duration::from_millis(150));
        assert!(!cleaned.load(Ordering::Acquire));
        assert!(slots.reserve().is_err());
        tokio::time::timeout(Duration::from_secs(2), async {
            while !cleaned.load(Ordering::Acquire) || slots.outstanding() != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(launches.load(Ordering::Acquire), 0);
        assert!(slots.reserve().is_ok());
    }
    #[tokio::test]
    async fn deadline_keeps_limit_reason_after_worker_acknowledges_stop() {
        let control = Arc::new(Control::new(Duration::from_millis(20)));
        let current = control.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let snapshot = Snapshot::fixture(&[("inert.txt", b"inert")]);
            while !current.cancelled.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(2));
            }
            snapshot.close()?;
            Err::<Output, Failure>(Failure::Stopped)
        });
        assert!(matches!(
            await_worker(
                worker,
                &control,
                &|| async { false },
                Duration::from_secs(2),
                CLEANUP_GRACE
            )
            .await,
            Err(Failure::Limit)
        ));
    }
    #[tokio::test]
    async fn unconfirmed_cleanup_keeps_capacity_reserved_after_worker_returns() {
        let slots = Arc::new(Slots::new(1));
        let worker = launch_owned(slots.reserve().unwrap(), || Err(Failure::CleanupPending));
        assert!(matches!(
            worker.await.unwrap(),
            Err(Failure::CleanupPending)
        ));
        assert!(slots.reserve().is_err());
    }
    #[tokio::test]
    async fn failed_cleanup_acknowledgement_is_never_reported_as_stopped() {
        let control = Arc::new(Control::new(Duration::from_secs(2)));
        let worker =
            tokio::task::spawn_blocking(|| Err::<Output, Failure>(Failure::CleanupPending));
        assert!(matches!(
            await_worker(
                worker,
                &control,
                &|| async { true },
                Duration::from_secs(2),
                CLEANUP_GRACE
            )
            .await,
            Err(Failure::CleanupPending)
        ));
    }
}
