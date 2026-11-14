//! Listener routing and restart-safe, acknowledged background event delivery.
//!
//! The adapter persists snapshots inside the owning install's storage and calls
//! `finish_registration` after the background's first evaluation task. Static
//! source scans never create authoritative listener registrations.

mod filters;

use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zephium_core::ids::ExtensionInstallId;

use crate::context::{ContextKey, ContextKind, ContextMask, Sender};
use crate::dispatch::{ApiTable, Handled, Invocation, RegistrationError};
use crate::envelope::{ApiName, EventSeq};
use crate::errors::ApiError;
use crate::limits::{MAX_QUEUED_EVENTS_PER_BACKGROUND, MAX_QUEUED_EVENT_BYTES_PER_BACKGROUND};
use crate::storage::{Storage, StorageChangeEvent};

use filters::FilterCost;
pub use filters::{EventFacts, ListenerFilter};

/// Maximum listener registrations in one context or persistent snapshot.
pub const MAX_LISTENERS: usize = 4_096;
/// Maximum encoded persistent listener snapshot, including filters.
pub const MAX_LISTENER_SNAPSHOT_BYTES: usize = 1 << 20;
/// Maximum live and saved registrations combined for one install.
pub const MAX_LISTENERS_PER_INSTALL: usize = 4_096;
/// Maximum live and saved registrations combined for one profile.
pub const MAX_LISTENERS_PER_PROFILE: usize = 16_384;
/// Maximum live and saved filter JSON bytes combined for one install.
pub const MAX_FILTER_BYTES_PER_INSTALL: usize = 1 << 20;
/// Maximum live and saved filter JSON bytes combined for one profile.
pub const MAX_FILTER_BYTES_PER_PROFILE: usize = 8 << 20;
/// Maximum live and saved compiled regexes combined for one install.
pub const MAX_REGEXES_PER_INSTALL: usize = 64;
/// Maximum live and saved compiled regexes combined for one profile.
pub const MAX_REGEXES_PER_PROFILE: usize = 256;

/// A binding-assigned callback identity, scoped to one authenticated context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ListenerId(NonZeroU64);

impl ListenerId {
    /// A nonzero callback identity.
    pub fn new(value: u64) -> Option<Self> {
        NonZeroU64::new(value).map(Self)
    }

    /// The callback identity returned to the binding.
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListenerSpec {
    event: String,
    filter: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    extra: Vec<String>,
}

#[derive(Clone, Debug)]
struct Listener {
    spec: ListenerSpec,
    filter: ListenerFilter,
    persistent: bool,
    cost: FilterCost,
}

/// Serializable listener interests for one install; contains no live authority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerSnapshot {
    version: u8,
    listeners: Vec<ListenerSpec>,
}

impl ListenerSnapshot {
    /// Decodes bounded persisted bytes. Restore validates the catalog,
    /// filters and aggregate budgets before creating saved interests.
    pub fn decode(bytes: &[u8]) -> Result<Self, ApiError> {
        if bytes.len() > MAX_LISTENER_SNAPSHOT_BYTES {
            return Err(ApiError::message_too_large());
        }
        serde_json::from_slice(bytes)
            .map_err(|_| ApiError::other("Invalid extension listener snapshot."))
    }
}

/// Per-profile live listeners plus per-install durable background interests.
#[derive(Debug, Default)]
pub struct EventRegistry {
    live: HashMap<ContextKey, HashMap<(String, ListenerId), Listener>>,
    evaluating: HashSet<ContextKey>,
    durable: HashMap<ExtensionInstallId, Vec<Listener>>,
}

impl EventRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts collecting the new background's first-task registrations.
    pub fn begin_registration(&mut self, sender: &Sender) -> Result<(), ApiError> {
        if sender.kind() != ContextKind::Background {
            return Err(ApiError::not_available_in_context(
                "events.beginRegistration",
            ));
        }
        self.live.remove(&sender.context_key());
        self.evaluating.insert(sender.context_key());
        Ok(())
    }

    /// Atomically replaces saved interests after successful first-task evaluation.
    /// A crash before this observation retains the previous snapshot.
    pub fn finish_registration(&mut self, sender: &Sender) -> Result<(), ApiError> {
        let key = sender.context_key();
        if sender.kind() != ContextKind::Background || !self.evaluating.contains(&key) {
            return Err(ApiError::context_invalidated());
        }
        let mut listeners: Vec<Listener> = self
            .live
            .get(&key)
            .into_iter()
            .flat_map(|entries| entries.values())
            .filter(|l| l.persistent)
            .cloned()
            .collect();
        listeners.sort_by(|a, b| {
            a.spec
                .event
                .cmp(&b.spec.event)
                .then_with(|| {
                    a.spec
                        .filter
                        .as_ref()
                        .map(Value::to_string)
                        .cmp(&b.spec.filter.as_ref().map(Value::to_string))
                })
                .then_with(|| a.spec.extra.cmp(&b.spec.extra))
        });
        listeners.dedup_by(|a, b| a.spec == b.spec);
        check_snapshot(&listeners)?;
        self.check_budget(
            sender.install(),
            listeners.iter().map(|listener| listener.cost),
            true,
        )?;
        self.evaluating.remove(&key);
        self.durable.insert(sender.install(), listeners);
        Ok(())
    }

    /// Registers a callback. Re-adding the same callback is a no-op.
    pub fn add(
        &mut self,
        sender: &Sender,
        id: ListenerId,
        event: &str,
        filter: Option<&Value>,
    ) -> Result<bool, ApiError> {
        self.add_with_options(sender, id, event, filter, &[])
    }

    /// Registers a callback and the generated event's extra information modes.
    pub fn add_with_options(
        &mut self,
        sender: &Sender,
        id: ListenerId,
        event: &str,
        filter: Option<&Value>,
        extra: &[String],
    ) -> Result<bool, ApiError> {
        validate_extra(event, id, filter, extra)?;
        if !EVENT_APIS.contains(&event) || !event_contexts(event).contains(sender.kind()) {
            return Err(ApiError::not_available_in_context(event));
        }
        let key = sender.context_key();
        if self
            .live
            .get(&key)
            .is_some_and(|listeners| listeners.contains_key(&(event.to_owned(), id)))
        {
            return Ok(false);
        }
        if self
            .live
            .get(&key)
            .is_some_and(|listeners| listeners.len() >= MAX_LISTENERS)
        {
            return Err(ApiError::too_many_pending());
        }
        let cost = listener_cost(filter, extra)?;
        self.check_budget(sender.install(), [cost], false)?;
        let parsed = ListenerFilter::parse(event, filter)?;
        let listeners = self.live.entry(key.clone()).or_default();
        listeners.insert(
            (event.to_owned(), id),
            Listener {
                spec: ListenerSpec {
                    event: event.to_owned(),
                    filter: filter.cloned(),
                    extra: extra.to_vec(),
                },
                filter: parsed,
                persistent: sender.kind() == ContextKind::Background
                    && self.evaluating.contains(&key),
                cost,
            },
        );
        Ok(true)
    }

    /// Removes one callback and its saved interest when no equivalent remains.
    pub fn remove(&mut self, sender: &Sender, event: &str, id: ListenerId) -> bool {
        let Some(listeners) = self.live.get_mut(&sender.context_key()) else {
            return false;
        };
        let Some(removed) = listeners.remove(&(event.to_owned(), id)) else {
            return false;
        };
        if sender.kind() == ContextKind::Background
            && removed.persistent
            && !listeners
                .values()
                .any(|l| l.persistent && l.spec == removed.spec)
            && !self.evaluating.contains(&sender.context_key())
        {
            if let Some(durable) = self.durable.get_mut(&sender.install()) {
                durable.retain(|l| l.spec != removed.spec);
            }
        }
        true
    }

    /// Whether this exact context registered the callback for `event`.
    pub fn has_listener(&self, sender: &Sender, id: ListenerId, event: &str) -> bool {
        self.live
            .get(&sender.context_key())
            .is_some_and(|listeners| listeners.contains_key(&(event.to_owned(), id)))
    }

    /// Whether this callback registered a blocking result mode for the event.
    pub fn is_blocking(&self, sender: &Sender, id: ListenerId, event: &str) -> bool {
        self.live
            .get(&sender.context_key())
            .and_then(|listeners| listeners.get(&(event.to_owned(), id)))
            .is_some_and(|listener| {
                listener
                    .spec
                    .extra
                    .iter()
                    .any(|mode| mode == "blocking" || mode == "asyncBlocking")
            })
    }

    /// Whether this exact context has any registered callback for the event.
    pub fn has_listeners(&self, sender: &Sender, event: &str) -> bool {
        self.live
            .get(&sender.context_key())
            .is_some_and(|listeners| {
                listeners
                    .values()
                    .any(|listener| listener.spec.event == event)
            })
    }

    /// Matching callback identities in this exact live context.
    pub fn listeners(
        &self,
        sender: &Sender,
        event: &str,
        facts: &EventFacts<'_>,
    ) -> Vec<ListenerId> {
        if is_storage_event(event) {
            return Vec::new();
        }
        self.matching_listeners(sender, event, facts)
    }

    fn matching_listeners(
        &self,
        sender: &Sender,
        event: &str,
        facts: &EventFacts<'_>,
    ) -> Vec<ListenerId> {
        let mut ids: Vec<_> = self
            .live
            .get(&sender.context_key())
            .into_iter()
            .flat_map(|l| l.iter())
            .filter(|(_, listener)| listener.spec.event == event && listener.filter.matches(facts))
            .map(|((_, id), _)| *id)
            .collect();
        ids.sort_by_key(|id| id.get());
        ids
    }

    /// Fan-out targets, once per context even when several callbacks match.
    pub fn targets(
        &self,
        install: ExtensionInstallId,
        event: &str,
        facts: &EventFacts<'_>,
    ) -> Vec<ContextKey> {
        if is_storage_event(event) {
            return Vec::new();
        }
        self.live
            .iter()
            .filter(|(key, listeners)| {
                key.install() == install
                    && listeners
                        .values()
                        .any(|l| l.spec.event == event && l.filter.matches(facts))
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Storage callback identities after checking current area access and
    /// profile/install/incognito identity. Generic routing refuses these events.
    pub fn storage_listeners(
        &self,
        sender: &Sender,
        storage: &Storage,
        event: &str,
        change: &StorageChangeEvent,
    ) -> Vec<ListenerId> {
        if storage.check_access(sender, change.area).is_err()
            || !(event == "storage.onChanged"
                || event == format!("storage.{}.onChanged", change.area.as_str()))
        {
            return Vec::new();
        }
        self.matching_listeners(sender, event, &EventFacts::default())
    }

    /// Storage fan-out to authenticated live senders, once per context.
    /// Call immediately before delivery so access revocation cannot leak data.
    pub fn storage_targets<'a>(
        &self,
        storage: &Storage,
        senders: impl IntoIterator<Item = &'a Sender>,
        event: &str,
        change: &StorageChangeEvent,
    ) -> Vec<ContextKey> {
        let mut seen = HashSet::new();
        senders
            .into_iter()
            .filter(|sender| {
                !self
                    .storage_listeners(sender, storage, event, change)
                    .is_empty()
            })
            .map(Sender::context_key)
            .filter(|key| seen.insert(key.clone()))
            .collect()
    }

    /// Whether a sleeping background registered an interest matching this event.
    pub fn should_wake(
        &self,
        install: ExtensionInstallId,
        event: &str,
        facts: &EventFacts<'_>,
    ) -> bool {
        self.durable.get(&install).is_some_and(|listeners| {
            listeners
                .iter()
                .any(|l| l.spec.event == event && l.filter.matches(facts))
        })
    }

    /// Removes live authority after navigation, suspension or context death.
    pub fn cancel_context(&mut self, context: &ContextKey) {
        self.live.remove(context);
        self.evaluating.remove(context);
    }

    /// Erases all listener state when an install is removed.
    pub fn erase(&mut self, install: ExtensionInstallId) {
        self.live.retain(|key, _| key.install() != install);
        self.evaluating.retain(|key| key.install() != install);
        self.durable.remove(&install);
    }

    /// Snapshot to persist under the adapter-supplied install identity.
    pub fn snapshot(&self, install: ExtensionInstallId) -> ListenerSnapshot {
        ListenerSnapshot {
            version: 1,
            listeners: self
                .durable
                .get(&install)
                .into_iter()
                .flatten()
                .map(|l| l.spec.clone())
                .collect(),
        }
    }

    /// Validates and restores a saved snapshot without restoring live contexts.
    pub fn restore(
        &mut self,
        install: ExtensionInstallId,
        snapshot: ListenerSnapshot,
    ) -> Result<(), ApiError> {
        if snapshot.version != 1 || snapshot.listeners.len() > MAX_LISTENERS {
            return Err(ApiError::other("Invalid extension listener snapshot."));
        }
        let costs = snapshot
            .listeners
            .iter()
            .map(|spec| listener_cost(spec.filter.as_ref(), &spec.extra))
            .collect::<Result<Vec<_>, _>>()?;
        self.check_budget(install, costs.iter().copied(), true)?;
        check_snapshot_specs(&snapshot.listeners)?;
        let mut listeners = Vec::with_capacity(snapshot.listeners.len());
        for (spec, cost) in snapshot.listeners.into_iter().zip(costs) {
            if !EVENT_APIS.contains(&spec.event.as_str()) {
                return Err(ApiError::other("Invalid extension listener snapshot."));
            }
            let filter = ListenerFilter::parse(&spec.event, spec.filter.as_ref())?;
            validate_extra(
                &spec.event,
                ListenerId::new(1).ok_or_else(ApiError::malformed_arguments)?,
                spec.filter.as_ref(),
                &spec.extra,
            )?;
            listeners.push(Listener {
                spec,
                filter,
                persistent: true,
                cost,
            });
        }
        check_snapshot(&listeners)?;
        self.durable.insert(install, listeners);
        Ok(())
    }

    fn check_budget(
        &self,
        install: ExtensionInstallId,
        added: impl IntoIterator<Item = FilterCost>,
        replace_durable: bool,
    ) -> Result<(), ApiError> {
        let mut install_cost = (0_usize, 0_usize, 0_usize);
        let mut profile_cost = (0_usize, 0_usize, 0_usize);
        let existing = self
            .live
            .iter()
            .flat_map(|(key, listeners)| {
                listeners
                    .values()
                    .map(move |listener| (key.install(), listener.cost))
            })
            .chain(
                self.durable
                    .iter()
                    .filter(|(key, _)| !replace_durable || **key != install)
                    .flat_map(|(key, listeners)| {
                        listeners.iter().map(move |listener| (*key, listener.cost))
                    }),
            );
        for (owner, cost) in existing.chain(added.into_iter().map(|cost| (install, cost))) {
            profile_cost.0 += 1;
            profile_cost.1 += cost.bytes;
            profile_cost.2 += cost.regexes;
            if owner == install {
                install_cost.0 += 1;
                install_cost.1 += cost.bytes;
                install_cost.2 += cost.regexes;
            }
        }
        if install_cost.0 > MAX_LISTENERS_PER_INSTALL
            || install_cost.1 > MAX_FILTER_BYTES_PER_INSTALL
            || install_cost.2 > MAX_REGEXES_PER_INSTALL
            || profile_cost.0 > MAX_LISTENERS_PER_PROFILE
            || profile_cost.1 > MAX_FILTER_BYTES_PER_PROFILE
            || profile_cost.2 > MAX_REGEXES_PER_PROFILE
        {
            return Err(ApiError::too_many_pending());
        }
        Ok(())
    }
}

fn check_snapshot(listeners: &[Listener]) -> Result<(), ApiError> {
    check_snapshot_specs(
        &listeners
            .iter()
            .map(|listener| listener.spec.clone())
            .collect::<Vec<_>>(),
    )
}

fn check_snapshot_specs(listeners: &[ListenerSpec]) -> Result<(), ApiError> {
    let bytes = serde_json::to_vec(&ListenerSnapshot {
        version: 1,
        listeners: listeners.to_vec(),
    })
    .map_err(|_| ApiError::other("Invalid extension listener snapshot."))?;
    if bytes.len() > MAX_LISTENER_SNAPSHOT_BYTES {
        return Err(ApiError::too_many_pending());
    }
    Ok(())
}

/// Whether queue pressure may discard a not-yet-delivered event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventImportance {
    /// Never discard; installation, startup and user-triggered work use this.
    Critical,
    /// The oldest undelivered event may be dropped when the queue is full.
    Droppable,
}

/// A queued delivery, retaining the same sequence across background restarts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedEvent {
    /// Acknowledgement identity.
    pub sequence: EventSeq,
    /// Validated Chrome event name.
    pub name: String,
    /// Already serialized arguments; relayed without inspecting the payload.
    pub arguments: Vec<u8>,
    importance: EventImportance,
    delivered_to: Option<ContextKey>,
}

/// FIFO wake queue for one install. Only its head may be in flight.
#[derive(Debug)]
pub struct EventQueue {
    install: ExtensionInstallId,
    next: u64,
    bytes: u64,
    queued: VecDeque<QueuedEvent>,
}

impl EventQueue {
    /// An empty queue. Keep this owner alive while backgrounds restart.
    pub fn new(install: ExtensionInstallId) -> Self {
        Self {
            install,
            next: 1,
            bytes: 0,
            queued: VecDeque::new(),
        }
    }

    /// Admits an event atomically, dropping only oldest undelivered droppable
    /// entries if required. A refusal never removes existing work.
    pub fn enqueue(
        &mut self,
        name: &str,
        arguments: Vec<u8>,
        importance: EventImportance,
    ) -> Result<EventSeq, ApiError> {
        if ApiName::parse(name).is_none() {
            return Err(ApiError::malformed_arguments());
        }
        let added = (name.len() as u64).saturating_add(arguments.len() as u64);
        if added > MAX_QUEUED_EVENT_BYTES_PER_BACKGROUND {
            return Err(ApiError::message_too_large());
        }
        let sequence = EventSeq::new(self.next).ok_or_else(ApiError::too_many_pending)?;
        let next = self
            .next
            .checked_add(1)
            .ok_or_else(ApiError::too_many_pending)?;
        let mut count = self.queued.len() + 1;
        let mut bytes = self.bytes + added;
        let mut discard = HashSet::new();
        for entry in &self.queued {
            if count <= MAX_QUEUED_EVENTS_PER_BACKGROUND as usize
                && bytes <= MAX_QUEUED_EVENT_BYTES_PER_BACKGROUND
            {
                break;
            }
            if entry.importance == EventImportance::Droppable && entry.delivered_to.is_none() {
                discard.insert(entry.sequence);
                count -= 1;
                bytes -= event_bytes(entry);
            }
        }
        if count > MAX_QUEUED_EVENTS_PER_BACKGROUND as usize
            || bytes > MAX_QUEUED_EVENT_BYTES_PER_BACKGROUND
        {
            return Err(ApiError::too_many_pending());
        }
        self.queued
            .retain(|entry| !discard.contains(&entry.sequence));
        self.queued.push_back(QueuedEvent {
            sequence,
            name: name.to_owned(),
            arguments,
            importance,
            delivered_to: None,
        });
        self.next = next;
        self.bytes = bytes;
        Ok(sequence)
    }

    /// Returns the head for delivery or retry. A fresh background may receive
    /// it only after `cancel_context` invalidates the previous delivery.
    pub fn deliver(&mut self, background: &Sender) -> Result<Option<QueuedEvent>, ApiError> {
        if background.install() != self.install || background.kind() != ContextKind::Background {
            return Err(ApiError::context_invalidated());
        }
        let key = background.context_key();
        let Some(head) = self.queued.front_mut() else {
            return Ok(None);
        };
        if head
            .delivered_to
            .as_ref()
            .is_some_and(|previous| previous != &key)
        {
            return Err(ApiError::context_invalidated());
        }
        head.delivered_to = Some(key);
        Ok(Some(head.clone()))
    }

    /// Removes the head only on an ack from the exact context it was sent to.
    pub fn acknowledge(&mut self, sender: &Sender, sequence: EventSeq) -> bool {
        if !self.queued.front().is_some_and(|event| {
            event.sequence == sequence && event.delivered_to.as_ref() == Some(&sender.context_key())
        }) {
            return false;
        }
        if let Some(event) = self.queued.pop_front() {
            self.bytes -= event_bytes(&event);
        }
        true
    }

    /// Makes an unacknowledged head replayable after background death.
    pub fn cancel_context(&mut self, key: &ContextKey) {
        if let Some(head) = self.queued.front_mut() {
            if head.delivered_to.as_ref() == Some(key) {
                head.delivered_to = None;
            }
        }
    }

    /// Pending entries, including the unacknowledged head.
    pub fn len(&self) -> usize {
        self.queued.len()
    }

    /// Whether all queued work was acknowledged.
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// Bytes of event names and serialized arguments retained by the queue.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

fn event_bytes(event: &QueuedEvent) -> u64 {
    event.name.len() as u64 + event.arguments.len() as u64
}

/// Browser observations that can create `runtime.onInstalled`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstalledReason {
    /// First install.
    Install,
    /// Version update or unpacked reload, even at the same version.
    Update {
        /// Version before the update.
        previous_version: String,
    },
    /// Browser updated while this extension remained installed.
    BrowserUpdate,
    /// A shared module used by the extension updated.
    SharedModuleUpdate {
        /// Updated module extension id.
        id: crate::context::ExtensionId,
    },
}

impl InstalledReason {
    /// Chrome event arguments. `previousVersion` exists only for `update`.
    pub fn arguments(&self) -> Value {
        match self {
            Self::Install => json!([{"reason":"install"}]),
            Self::Update { previous_version } => {
                json!([{"reason":"update","previousVersion":previous_version}])
            }
            Self::BrowserUpdate => json!([{"reason":"chrome_update"}]),
            Self::SharedModuleUpdate { id } => {
                json!([{"reason":"shared_module_update","id":id.as_str()}])
            }
        }
    }
}

/// Startup is profile-scoped, never emitted for split incognito startup.
pub fn should_emit_startup(
    registry: &EventRegistry,
    install: ExtensionInstallId,
    incognito: bool,
) -> bool {
    !incognito && registry.should_wake(install, "runtime.onStartup", &EventFacts::default())
}

/// Binding operation after argument and filter validation.
#[derive(Clone, Debug)]
pub enum ListenerRequest {
    /// Register one callback identity with optional supported filters.
    Add {
        /// Chrome event name.
        event: &'static str,
        /// Context-scoped callback identity.
        id: ListenerId,
        /// Validated filter input.
        filter: Option<Value>,
        /// Generated event-specific extra information modes.
        extra: Vec<String>,
    },
    /// Remove one callback identity.
    Remove {
        /// Chrome event name.
        event: &'static str,
        /// Callback identity.
        id: ListenerId,
    },
    /// Test one callback identity.
    Has {
        /// Chrome event name.
        event: &'static str,
        /// Callback identity.
        id: ListenerId,
    },
    /// Test whether any callbacks exist for this event in the calling context.
    HasAny {
        /// Chrome event name.
        event: &'static str,
    },
    /// The background completed its first evaluation task.
    CompleteRegistration,
}

/// Adapter implementation of authenticated listener operations and persistence.
pub trait EventsHost {
    /// Applies a validated request. Durable changes settle after persistence.
    fn listener_request(&mut self, request: ListenerRequest, invocation: Invocation<'_>)
        -> Handled;
}

/// Installs listener binding operations with their event's permission gate.
pub fn register<S: EventsHost + 'static>(table: &mut ApiTable<S>) -> Result<(), RegistrationError> {
    for &event in EVENT_APIS {
        for operation in [
            "addListener",
            "removeListener",
            "hasListener",
            "hasListeners",
        ] {
            // Registration keys must live for the table's lifetime. The catalog
            // uses compile-time strings below instead of leaking allocations.
            let name = listener_method(event, operation);
            if let Some(name) = name {
                table.register_generated(name, move |host: &mut S, invocation| {
                    match parse_listener_request(event, operation, invocation.args()) {
                        Ok(request) => match check_listener_permissions(&request, &invocation) {
                            Ok(()) => host.listener_request(request, invocation),
                            Err(error) => invocation.reply(Err(error)),
                        },
                        Err(error) => invocation.reply(Err(error)),
                    }
                })?;
            }
        }
    }
    if table.contexts("events.completeRegistration").is_none() {
        table.register_generated("events.completeRegistration", |host: &mut S, invocation| {
            host.listener_request(ListenerRequest::CompleteRegistration, invocation)
        })?;
    }
    Ok(())
}

pub(crate) fn check_listener_permissions(
    request: &ListenerRequest,
    invocation: &Invocation<'_>,
) -> Result<(), ApiError> {
    let ListenerRequest::Add { extra, .. } = request else {
        return Ok(());
    };
    if extra.iter().any(|mode| mode == "blocking") {
        crate::permissions::Gate::Requires("webRequestBlocking").check(
            invocation.permissions(),
            invocation.sender(),
            None,
        )?;
    }
    if extra.iter().any(|mode| mode == "asyncBlocking") {
        crate::permissions::Gate::RequiresAny(&["webRequestBlocking", "webRequestAuthProvider"])
            .check(invocation.permissions(), invocation.sender(), None)?;
    }
    Ok(())
}

pub(crate) fn parse_listener_request(
    event: &'static str,
    operation: &str,
    args: &[Value],
) -> Result<ListenerRequest, ApiError> {
    if operation == "hasListeners" && args.is_empty() {
        return Ok(ListenerRequest::HasAny { event });
    }
    let id = args
        .first()
        .and_then(Value::as_u64)
        .filter(|n| *n <= 9_007_199_254_740_991)
        .and_then(ListenerId::new)
        .ok_or_else(ApiError::malformed_arguments)?;
    match operation {
        "addListener" if args.len() <= 3 => {
            let filter = args.get(1).filter(|value| !value.is_null());
            let extra = match args.get(2).filter(|value| !value.is_null()) {
                Some(value) => value
                    .as_array()
                    .ok_or_else(ApiError::malformed_arguments)?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(ApiError::malformed_arguments)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                None => Vec::new(),
            };
            ListenerFilter::parse(event, filter)?;
            validate_extra(event, id, filter, &extra)?;
            Ok(ListenerRequest::Add {
                event,
                id,
                filter: filter.cloned(),
                extra,
            })
        }
        "removeListener" if args.len() == 1 => Ok(ListenerRequest::Remove { event, id }),
        "hasListener" if args.len() == 1 => Ok(ListenerRequest::Has { event, id }),
        _ => Err(ApiError::malformed_arguments()),
    }
}

pub(crate) fn event_contexts(event: &str) -> ContextMask {
    crate::schema::access(&format!("{event}.addListener"))
        .map(|access| {
            access
                .contexts
                .intersection(crate::dispatch::permitted_contexts(event))
        })
        .unwrap_or(ContextMask::NONE)
}

fn listener_cost(filter: Option<&Value>, extra: &[String]) -> Result<FilterCost, ApiError> {
    let mut cost = ListenerFilter::cost(filter)?;
    cost.bytes = cost
        .bytes
        .saturating_add(extra.iter().map(String::len).sum::<usize>());
    Ok(cost)
}

fn validate_extra(
    event: &str,
    id: ListenerId,
    filter: Option<&Value>,
    extra: &[String],
) -> Result<(), ApiError> {
    if extra.is_empty() {
        return Ok(());
    }
    if extra.len() > 16 || extra.iter().any(|value| value.len() > 64) {
        return Err(ApiError::malformed_arguments());
    }
    crate::schema::normalize(
        &format!("{event}.addListener"),
        &[
            json!(id.get()),
            filter.cloned().unwrap_or(Value::Null),
            json!(extra),
        ],
    )?;
    Ok(())
}

fn is_storage_event(event: &str) -> bool {
    matches!(
        event,
        "storage.onChanged"
            | "storage.local.onChanged"
            | "storage.sync.onChanged"
            | "storage.session.onChanged"
            | "storage.managed.onChanged"
    )
}

macro_rules! event_catalog {
    ($( $name:literal ),* $(,)?) => {
        pub(crate) const EVENT_APIS: &[&str] = &[$($name),*];
        fn listener_method(event: &str, operation: &str) -> Option<&'static str> {
            match (event, operation) {
                $(( $name, "addListener") => Some(concat!($name, ".addListener")),
                  ( $name, "removeListener") => Some(concat!($name, ".removeListener")),
                  ( $name, "hasListener") => Some(concat!($name, ".hasListener")),
                  ( $name, "hasListeners") => Some(concat!($name, ".hasListeners")),)*
                _ => None,
            }
        }
    };
}

event_catalog!(
    "runtime.onMessage",
    "runtime.onConnect",
    "runtime.onMessageExternal",
    "runtime.onConnectExternal",
    "runtime.onInstalled",
    "runtime.onStartup",
    "runtime.onSuspend",
    "runtime.onSuspendCanceled",
    "storage.onChanged",
    "alarms.onAlarm",
    "storage.local.onChanged",
    "storage.sync.onChanged",
    "storage.session.onChanged",
    "storage.managed.onChanged",
    "tabs.onCreated",
    "tabs.onUpdated",
    "tabs.onRemoved",
    "tabs.onActivated",
    "windows.onCreated",
    "windows.onRemoved",
    "windows.onFocusChanged",
    "webNavigation.onBeforeNavigate",
    "webNavigation.onCommitted",
    "webNavigation.onDOMContentLoaded",
    "webNavigation.onCompleted",
    "webNavigation.onErrorOccurred",
    "webNavigation.onHistoryStateUpdated",
    "webNavigation.onReferenceFragmentUpdated",
    "webRequest.onBeforeRequest",
    "webRequest.onBeforeSendHeaders",
    "webRequest.onSendHeaders",
    "webRequest.onHeadersReceived",
    "webRequest.onResponseStarted",
    "webRequest.onCompleted",
    "webRequest.onErrorOccurred",
    "webRequest.onAuthRequired",
);

#[cfg(test)]
mod tests;
