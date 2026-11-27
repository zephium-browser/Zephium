use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use crate::types::{ReadRefusal, TimeoutPhase};

tokio::task_local! {
    static PROBE: Arc<ReadProbe>;
}

const QUEUE: u8 = 0;
const CONNECT: u8 = 1;
const FIRST_BYTE: u8 = 2;
const BODY: u8 = 3;

/// What one read observed of its own connections and progress. Shared with
/// the connector only through the task-local scope of that read's requests.
#[derive(Default)]
pub(crate) struct ReadProbe {
    phase: AtomicU8,
    connect_nanos: AtomicU64,
    opened: AtomicU64,
    wire: AtomicU64,
}

impl ReadProbe {
    pub(crate) fn queue(&self) {
        self.phase.store(QUEUE, Ordering::Release);
    }

    pub(crate) fn add_wire(&self, bytes: u64) -> u64 {
        self.wire
            .fetch_add(bytes, Ordering::AcqRel)
            .saturating_add(bytes)
    }

    pub(crate) fn wire(&self) -> u64 {
        self.wire.load(Ordering::Acquire)
    }

    pub(crate) fn first_byte(&self) {
        self.phase.store(FIRST_BYTE, Ordering::Release);
    }

    pub(crate) fn body(&self) {
        self.phase.store(BODY, Ordering::Release);
    }

    pub(crate) fn timeout(&self) -> ReadRefusal {
        ReadRefusal::Timeout(match self.phase.load(Ordering::Acquire) {
            QUEUE => TimeoutPhase::Queue,
            CONNECT => TimeoutPhase::Connect,
            FIRST_BYTE => TimeoutPhase::FirstByte,
            _ => TimeoutPhase::Body,
        })
    }

    pub(crate) fn connect(&self) -> Option<Duration> {
        (self.opened.load(Ordering::Acquire) > 0)
            .then(|| Duration::from_nanos(self.connect_nanos.load(Ordering::Acquire)))
    }

    pub(crate) fn opened(&self) -> u64 {
        self.opened.load(Ordering::Acquire)
    }

    pub(crate) async fn scope<F: Future>(self: &Arc<Self>, future: F) -> F::Output {
        PROBE.scope(self.clone(), future).await
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ConnectTiming;

impl<S> tower_layer::Layer<S> for ConnectTiming {
    type Service = Timed<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Timed(inner)
    }
}

#[derive(Clone)]
pub(crate) struct Timed<S>(S);

impl<S, R> tower_service::Service<R> for Timed<S>
where
    S: tower_service::Service<R>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(context)
    }

    fn call(&mut self, request: R) -> Self::Future {
        // Captured when the connection is asked for, inside the read's scope,
        // so a connect finished by a pool task still reports to its read.
        let probe = PROBE.try_with(Arc::clone).ok();
        if let Some(probe) = &probe {
            probe.phase.store(CONNECT, Ordering::Release);
        }
        let started = Instant::now();
        let connecting = self.0.call(request);
        Box::pin(async move {
            let connected = connecting.await;
            if let Some(probe) = probe {
                let nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
                probe.connect_nanos.fetch_add(nanos, Ordering::AcqRel);
                if connected.is_ok() {
                    probe.opened.fetch_add(1, Ordering::AcqRel);
                }
                let _ = probe.phase.compare_exchange(
                    CONNECT,
                    FIRST_BYTE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
            connected
        })
    }
}
