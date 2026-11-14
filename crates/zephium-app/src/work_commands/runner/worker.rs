//! A bounded caller observes an owned worker; it never owns its cleanup permit.
use super::{worker_slots::Slots, Failure};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub filesystem: Duration,
    pub total: Duration,
    pub grace: Duration,
}
impl Limits {
    pub fn standard(total: Duration) -> Self {
        Self {
            filesystem: Duration::from_secs(10),
            total,
            grace: Duration::from_millis(300),
        }
    }
}
pub(super) trait WorkerError: From<Failure> + Send + 'static {
    fn stopped(&self) -> bool;
    fn unsettled(&self) -> bool;
}
impl WorkerError for Failure {
    fn stopped(&self) -> bool {
        *self == Self::Stopped
    }
    fn unsettled(&self) -> bool {
        match self {
            Self::CleanupPending => true,
            #[cfg(any(target_os = "macos", windows))]
            Self::StopUnconfirmed => true,
            _ => false,
        }
    }
}
pub(super) struct Control {
    pub cancelled: Arc<AtomicBool>,
    started: Instant,
    filesystem_deadline: AtomicU64,
    filesystem_limit: Duration,
}
impl Control {
    pub fn new(limit: Duration) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            filesystem_deadline: AtomicU64::new(limit.as_millis() as u64),
            filesystem_limit: limit,
        }
    }
    pub fn filesystem(&self) {
        self.filesystem_deadline.store(
            self.started.elapsed().as_millis() as u64 + self.filesystem_limit.as_millis() as u64,
            Ordering::Release,
        );
    }
    pub fn native(&self) {
        self.filesystem_deadline.store(0, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), Failure> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Failure::Stopped);
        }
        let deadline = self.filesystem_deadline.load(Ordering::Acquire);
        if deadline != 0 && self.started.elapsed().as_millis() >= deadline as u128 {
            return Err(Failure::Limit);
        }
        Ok(())
    }
}
struct CancelOnDrop(Arc<Control>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::Release);
    }
}
pub(super) async fn run<T, E, C, CF, W, WF>(
    slots: &Arc<Slots>,
    cancelled: &C,
    limits: Limits,
    work: W,
) -> Result<T, E>
where
    T: Send + 'static,
    E: WorkerError,
    C: Fn() -> CF,
    CF: Future<Output = bool>,
    W: FnOnce(Arc<Control>) -> WF + Send + 'static,
    WF: Future<Output = Result<T, E>> + 'static,
{
    if super::cancellation_requested(cancelled).await {
        return Err(Failure::Stopped.into());
    }
    let permit = slots.reserve().map_err(|_| E::from(Failure::WorkerBusy))?;
    let control = Arc::new(Control::new(limits.filesystem));
    let _cancel = CancelOnDrop(control.clone());
    let owned = control.clone();
    let runtime = tokio::runtime::Handle::current();
    let worker = super::worker_slots::launch(
        permit,
        move || runtime.block_on(work(owned)),
        |result| result.as_ref().err().is_none_or(|error| !error.unsettled()),
    );
    await_worker(worker, &control, cancelled, limits.total, limits.grace).await
}
pub(super) async fn await_worker<T, E, C, CF>(
    mut worker: tokio::task::JoinHandle<Result<T, E>>,
    control: &Control,
    cancelled: &C,
    limit: Duration,
    grace: Duration,
) -> Result<T, E>
where
    T: Send + 'static,
    E: WorkerError,
    C: Fn() -> CF,
    CF: Future<Output = bool>,
{
    let deadline = tokio::time::sleep(limit);
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    let reason = loop {
        tokio::select! {
                   result=&mut worker=>return result.unwrap_or_else(|_|Err(Failure::CleanupPending.into())),
                   _=&mut deadline=>break Failure::Limit,
                   _=tick.tick()=>{if super::cancellation_requested(cancelled).await{break Failure::Stopped}
        if let Err(error)=control.check(){break error}}
               }
    };
    control.cancelled.store(true, Ordering::Release);
    match tokio::time::timeout(grace, &mut worker).await {
        Ok(Ok(Ok(_))) => Err(reason.into()),
        Ok(Ok(Err(error))) if error.stopped() => Err(reason.into()),
        Ok(Ok(Err(error))) => Err(error),
        Ok(Err(_)) | Err(_) => Err(Failure::CleanupPending.into()),
    }
}
pub(super) fn closed<T, E: WorkerError>(
    result: Result<T, E>,
    closed: Result<(), Failure>,
) -> Result<T, E> {
    if result.as_ref().err().is_some_and(WorkerError::unsettled) {
        return result;
    }
    closed.map_err(E::from)?;
    result
}

#[cfg(all(test, target_os = "macos"))]
pub(super) async fn pending_capacity_fixture<F, FF>(slots: Arc<Slots>, refused: F)
where
    F: FnOnce() -> FF,
    FF: Future<Output = bool>,
{
    let release = Arc::new(AtomicBool::new(false));
    let mut callers = Vec::new();
    for _ in 0..4 {
        let slots = slots.clone();
        let release = release.clone();
        callers.push(tokio::spawn(async move {
            run(
                &slots,
                &|| async { false },
                Limits {
                    filesystem: Duration::from_millis(20),
                    total: Duration::from_secs(2),
                    grace: Duration::from_millis(20),
                },
                move |control| async move {
                    let snapshot = super::Snapshot::fixture(&[("inert.txt", b"inert")]);
                    let until = Instant::now() + Duration::from_secs(3);
                    while !release.load(Ordering::Acquire) && Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    closed(control.check(), snapshot.close())
                },
            )
            .await
        }));
    }
    for caller in callers {
        assert!(matches!(
            caller.await.unwrap(),
            Err(Failure::CleanupPending)
        ));
    }
    assert_eq!(slots.outstanding(), 4);
    assert!(refused().await);
    release.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(2), async {
        while slots.outstanding() != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
