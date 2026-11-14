//! What the bridge hands to the shared runtime, and what it takes back.

use std::collections::HashMap;

use url::Url;
use zephium_core::ids::{ExtensionInstallId, ItemId};
use zephium_webext_runtime::context::{ContextKey, DocumentGeneration, FrameId, Sender, ViewKind};
use zephium_webext_runtime::dispatch::{encode_for, Dispatcher, Inbound};
use zephium_webext_runtime::envelope::{Direction, Envelope};
use zephium_webext_runtime::envelope::{EventSeq, PortId};
use zephium_webext_runtime::errors::ApiError;
use zephium_webext_runtime::limits::{Clock, MonotonicClock, Reservation};
use zephium_webext_runtime::permissions::InstallPermissions;
use zephium_webkit_web_process_protocol::protocol::RequestEvent;

/// A view registered with a profile bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BridgeViewId(pub(crate) u64);

/// Which host object a view is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewRef {
    /// An ordinary tab.
    Tab(ItemId),
    /// A runtime-owned extension view.
    Extension {
        /// The owning install.
        install: ExtensionInstallId,
        /// What the view is for.
        kind: ViewKind,
    },
}

/// Frame and document lifecycle, as the module reported it and the host
/// accepted it. URLs of subframes are renderer-reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameLifecycle {
    /// A frame got a new document.
    DocumentCreated {
        /// The view.
        view: BridgeViewId,
        /// The host object.
        owner: ViewRef,
        /// Chrome's frame id.
        frame: FrameId,
        /// Parent frame, when verified against this view's table.
        parent: Option<FrameId>,
        /// Whether native facts report a sandboxed or opaque document.
        opaque: bool,
        /// Verified related-document URL, when available.
        creator: Option<Url>,
        /// The host generation of the new document.
        document: DocumentGeneration,
        /// The document URL.
        url: Url,
    },
    /// A document finished loading.
    DocumentLoaded {
        /// The view.
        view: BridgeViewId,
        /// The host object.
        owner: ViewRef,
        /// Chrome's frame id.
        frame: FrameId,
        /// The document.
        document: DocumentGeneration,
    },
    /// A document reached `DOMContentLoaded`, or its window `load` when
    /// `completed`. Renderer-reported, for the document the bridge records.
    DocumentProgress {
        /// The view.
        view: BridgeViewId,
        /// The host object.
        owner: ViewRef,
        /// Chrome's frame id.
        frame: FrameId,
        /// The document.
        document: DocumentGeneration,
        /// `load` rather than `DOMContentLoaded`.
        completed: bool,
    },
    /// A document went away, by navigation or frame removal.
    DocumentRemoved {
        /// The view.
        view: BridgeViewId,
        /// The host object.
        owner: ViewRef,
        /// Chrome's frame id.
        frame: FrameId,
        /// The document.
        document: DocumentGeneration,
    },
    /// A tab's main frame has a new document: end `activeTab` grants.
    MainDocumentChanged {
        /// The tab.
        tab: ItemId,
        /// The new main document.
        document: DocumentGeneration,
    },
    /// Every frame of the view is gone at once: re-attachment, a reset, a
    /// lost web process or detachment.
    ViewReset {
        /// The view.
        view: BridgeViewId,
        /// The host object.
        owner: ViewRef,
        /// Why.
        reason: CloseReason,
    },
}

/// Why a context stopped existing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CloseReason {
    /// Its document was replaced or its frame removed.
    DocumentGone,
    /// The view was attached again with a new generation.
    Reattached,
    /// The module reset its frame records.
    Reset,
    /// The view's web process exited or was replaced.
    ProcessGone,
    /// The host no longer grants the world or origin.
    Revoked,
    /// The install was retired or restarted.
    InstallRetired,
    /// The view was detached.
    Detached,
    /// The bridge is shutting down.
    Shutdown,
}

/// Why a frame could not be sent to a context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliverError {
    /// The context is gone or was never bound.
    Stale,
    /// Larger than the context may receive.
    TooLarge,
    /// The context's queue is full; retry after it drains or drop.
    Backpressure,
    /// The transport failed or returned an invalid answer.
    Refused,
    /// The native document has no registered receiver.
    NoReceiver,
    /// The module denied the target.
    Denied,
    /// The receiver threw an exception.
    EvaluationFailed,
    /// The native delivery budget was exhausted.
    RateLimited,
    /// The bridge is shutting down.
    Closed,
}

impl From<zephium_webkit_web_process_protocol::protocol::DeliveryStatus> for DeliverError {
    fn from(status: zephium_webkit_web_process_protocol::protocol::DeliveryStatus) -> Self {
        use zephium_webkit_web_process_protocol::protocol::DeliveryStatus;
        match status {
            DeliveryStatus::Delivered => Self::Refused,
            DeliveryStatus::StaleTarget => Self::Stale,
            DeliveryStatus::Denied => Self::Denied,
            DeliveryStatus::NoReceiver => Self::NoReceiver,
            DeliveryStatus::EvaluationFailed => Self::EvaluationFailed,
            DeliveryStatus::RateLimited => Self::RateLimited,
        }
    }
}

/// Frames the runtime wants delivered, collected while it runs.
#[derive(Debug, Default)]
pub struct Outbox {
    pub(crate) frames: Vec<(ContextKey, Vec<u8>)>,
}

impl Outbox {
    /// Queues `frame` for `context`.
    pub fn deliver(&mut self, context: ContextKey, frame: Vec<u8>) {
        self.frames.push((context, frame));
    }
}

/// The runtime side of a profile bridge. Every method runs on the GTK
/// thread, never re-entrantly, with the bridge's own state released.
pub trait Runtime {
    /// One envelope from a context whose sender the authority bound.
    fn receive(&mut self, sender: &Sender, envelope: &[u8], out: &mut Outbox);

    /// The context can no longer send or receive: cancel its calls and
    /// disconnect its ports.
    fn context_closed(&mut self, context: &ContextKey, reason: CloseReason, out: &mut Outbox) {
        let _ = (context, reason, out);
    }

    /// Frame and document lifecycle, for ports and `webNavigation`.
    fn frame_lifecycle(&mut self, event: &FrameLifecycle, out: &mut Outbox) {
        let _ = (event, out);
    }

    /// The module could not attach; this view cannot run extensions.
    fn transport_unavailable(
        &mut self,
        view: BridgeViewId,
        owner: ViewRef,
        reason: super::UnavailableReason,
    ) {
        let _ = (view, owner, reason);
    }

    /// A delivery the bridge could not complete.
    fn undeliverable(&mut self, context: &ContextKey, error: DeliverError) {
        let _ = (context, error);
    }

    /// A native execution result checked against the host's pending request.
    fn injection_result(
        &mut self,
        view: BridgeViewId,
        result: &zephium_webkit_web_process_protocol::protocol::InjectionResult,
    ) {
        let _ = (view, result);
    }

    /// Request observations from a view attached with `observe_requests`.
    fn requests(&mut self, view: BridgeViewId, owner: ViewRef, observations: Vec<RequestEvent>) {
        let _ = (view, owner, observations);
    }

    /// A form submission from a view attached with `observe_forms`. The
    /// fields can hold credentials: never log them.
    fn form_submitted(
        &mut self,
        view: BridgeViewId,
        owner: ViewRef,
        frame: FrameId,
        document: DocumentGeneration,
        fields: Vec<(String, String)>,
    ) {
        let _ = (view, owner, frame, document, fields);
    }
}

/// Port and event traffic the dispatcher leaves to the ports owner.
#[derive(Debug)]
pub enum PortTraffic<'a> {
    /// Route a port message, then release its reservation through
    /// [`Dispatcher::release`].
    Message {
        /// The sending context.
        sender: &'a Sender,
        /// The port.
        port: PortId,
        /// The opaque payload.
        payload: &'a [u8],
        /// The budget the message holds.
        reservation: Reservation,
    },
    /// The context closed a port.
    Disconnect {
        /// The sending context.
        sender: &'a Sender,
        /// The port.
        port: PortId,
    },
    /// The context handled an event.
    EventAck {
        /// The sending context.
        sender: &'a Sender,
        /// The acknowledged delivery.
        seq: EventSeq,
    },
}

/// The ports and events owner [`DispatchRuntime`] forwards to.
pub trait Ports<S> {
    /// Port or event traffic from a context. A refused port message's
    /// disconnect frame goes back through `out`.
    fn traffic(
        &mut self,
        dispatcher: &mut Dispatcher<S>,
        traffic: PortTraffic<'_>,
        out: &mut Outbox,
    );

    /// The context is gone; disconnect its ports.
    fn context_closed(&mut self, context: &ContextKey, out: &mut Outbox) {
        let _ = (context, out);
    }
}

/// A [`Runtime`] that hands every envelope to the shared [`Dispatcher`],
/// which applies the authoritative rate, byte and outstanding-call limits.
pub struct DispatchRuntime<S, P> {
    /// The call pipeline.
    pub dispatcher: Dispatcher<S>,
    /// The state handlers run against.
    pub state: S,
    /// Permission state of each running install.
    pub permissions: HashMap<ExtensionInstallId, InstallPermissions>,
    /// The ports owner.
    pub ports: P,
    clock: Box<dyn Clock>,
    refused: u64,
}

impl<S, P: Ports<S>> DispatchRuntime<S, P> {
    /// A runtime over `dispatcher`.
    pub fn new(dispatcher: Dispatcher<S>, state: S, ports: P) -> Self {
        Self {
            dispatcher,
            state,
            permissions: HashMap::new(),
            ports,
            clock: Box::new(MonotonicClock::new()),
            refused: 0,
        }
    }

    /// Uses `clock` for rate limits instead of the monotonic clock.
    pub fn with_clock(mut self, clock: impl Clock + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Frames the dispatcher refused without a reply.
    pub fn refused(&self) -> u64 {
        self.refused
    }
}

impl<S, P: Ports<S>> Runtime for DispatchRuntime<S, P> {
    fn receive(&mut self, sender: &Sender, envelope: &[u8], out: &mut Outbox) {
        let Some(permissions) = self.permissions.get(&sender.install()) else {
            self.refused += 1;
            let decoded = Envelope::decode(envelope, Direction::FromContext, u64::MAX);
            if let Ok(Envelope::Call { id, .. }) = decoded {
                let error = ApiError::context_invalidated();
                let reply = Envelope::Reply {
                    id,
                    result: Err(error.message()),
                };
                if let Ok(frame) = encode_for(sender.kind(), &reply) {
                    out.deliver(sender.context_key(), frame);
                }
            }
            return;
        };
        let now = self.clock.now_nanos();
        let traffic =
            match self
                .dispatcher
                .receive(&mut self.state, permissions, sender, envelope, now)
            {
                Inbound::Reply(frame) => {
                    out.deliver(sender.context_key(), frame);
                    return;
                }
                Inbound::Pending => return,
                Inbound::Refused(_) => {
                    self.refused += 1;
                    return;
                }
                Inbound::PortRefused { port, error } => {
                    let frame = encode_for(
                        sender.kind(),
                        &Envelope::PortDisconnect {
                            port,
                            error: Some(error.message()),
                        },
                    );
                    if let Ok(frame) = frame {
                        out.deliver(sender.context_key(), frame);
                    }
                    return;
                }
                Inbound::PortMessage {
                    port,
                    payload,
                    reservation,
                } => PortTraffic::Message {
                    sender,
                    port,
                    payload,
                    reservation,
                },
                Inbound::PortDisconnect { port } => PortTraffic::Disconnect { sender, port },
                Inbound::EventAck { seq } => PortTraffic::EventAck { sender, seq },
            };
        self.ports.traffic(&mut self.dispatcher, traffic, out);
    }

    fn context_closed(&mut self, context: &ContextKey, _: CloseReason, out: &mut Outbox) {
        self.dispatcher.cancel_context(context);
        self.ports.context_closed(context, out);
    }
}
