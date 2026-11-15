//! Every byte, rate and concurrency bound of the bridge, and the accounting
//! that enforces them. Exceeding one rejects the message with an error; it
//! never kills the context.

use std::collections::HashMap;
use std::time::Instant;

use zephium_core::ids::ExtensionInstallId;

use crate::context::{ContextKey, ContextKind};
use crate::errors::ApiError;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;

/// Largest message from or to a runtime-owned context.
pub const MAX_MESSAGE_BYTES_RUNTIME_OWNED: u64 = 32 * MIB;
/// Largest message from or to a content script, user script, framed
/// extension page or external page. Chrome allows 64 MiB.
pub const MAX_MESSAGE_BYTES_IN_WEB_RENDERER: u64 = 8 * MIB;
/// Bytes one context may have in flight.
pub const MAX_IN_FLIGHT_BYTES_PER_CONTEXT: u64 = 64 * MIB;
/// Bytes one install may have in flight across its contexts.
pub const MAX_IN_FLIGHT_BYTES_PER_INSTALL: u64 = 128 * MIB;
/// Calls one context may have awaiting settlement.
pub const MAX_OUTSTANDING_CALLS_PER_CONTEXT: u32 = 256;
/// Sustained messages per second per context.
pub const RATE_MESSAGES_PER_SECOND: u32 = 2_000;
/// Messages a context may send at once after being idle.
pub const RATE_BURST: u32 = 500;
/// Open ports per install.
pub const MAX_PORTS_PER_INSTALL: u32 = 1_024;
/// Events queued for one sleeping background.
pub const MAX_QUEUED_EVENTS_PER_BACKGROUND: u32 = 512;
/// Event bytes queued for one sleeping background.
pub const MAX_QUEUED_EVENT_BYTES_PER_BACKGROUND: u64 = 8 * MIB;
/// `storage.session` quota.
pub const STORAGE_SESSION_QUOTA_BYTES: u64 = 10 * MIB;
/// `storage.local` quota.
pub const STORAGE_LOCAL_QUOTA_BYTES: u64 = 10 * MIB;
/// `storage.local` quota with `unlimitedStorage`.
pub const STORAGE_LOCAL_UNLIMITED_QUOTA_BYTES: u64 = GIB;
/// Deepest nesting of API arguments.
pub const MAX_ARGUMENT_DEPTH: usize = 64;
/// Longest `namespace.method` name.
pub const MAX_API_NAME_BYTES: usize = 128;

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_TOKEN: u64 = NANOS_PER_SECOND / RATE_MESSAGES_PER_SECOND as u64;
const _: () = assert!(NANOS_PER_TOKEN * RATE_MESSAGES_PER_SECOND as u64 == NANOS_PER_SECOND);

/// The largest message a context of `kind` may send or receive.
pub const fn message_limit(kind: ContextKind) -> u64 {
    if kind.is_runtime_owned() {
        MAX_MESSAGE_BYTES_RUNTIME_OWNED
    } else {
        MAX_MESSAGE_BYTES_IN_WEB_RENDERER
    }
}

/// Monotonic time in nanoseconds since an arbitrary fixed point.
pub trait Clock {
    /// Now. Must never decrease.
    fn now_nanos(&self) -> u64;
}

/// The process monotonic clock.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    /// A clock whose zero is now.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now_nanos(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

/// A clock the caller advances, for tests and simulations.
#[derive(Debug, Default)]
pub struct ManualClock {
    now: std::cell::Cell<u64>,
}

impl ManualClock {
    /// Moves time forward.
    pub fn advance_nanos(&self, nanos: u64) {
        self.now.set(self.now.get().saturating_add(nanos));
    }
}

impl Clock for ManualClock {
    fn now_nanos(&self) -> u64 {
        self.now.get()
    }
}

/// A token bucket refilled at [`RATE_MESSAGES_PER_SECOND`] up to
/// [`RATE_BURST`], in exact integer time.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    tokens: u32,
    refilled_at: u64,
}

impl TokenBucket {
    /// A full bucket at `now`.
    pub fn new(now: u64) -> Self {
        Self {
            tokens: RATE_BURST,
            refilled_at: now,
        }
    }

    /// Takes one token if one is available at `now`.
    pub fn try_take(&mut self, now: u64) -> bool {
        self.refill(now);
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }

    fn refill(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.refilled_at);
        let earned = elapsed / NANOS_PER_TOKEN;
        let room = u64::from(RATE_BURST - self.tokens);
        if earned >= room {
            self.tokens = RATE_BURST;
            self.refilled_at = now;
        } else {
            // `earned < room <= RATE_BURST`, so it fits.
            self.tokens += earned as u32;
            self.refilled_at += earned * NANOS_PER_TOKEN;
        }
    }
}

/// One admitted call or message whose bytes and slot are held until the
/// caller returns it to [`Limiter::release`].
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a reservation holds budget until released"]
pub struct Reservation {
    context: ContextKey,
    bytes: u64,
    call: bool,
}

impl Reservation {
    /// Authenticated context charged by this reservation.
    pub fn context(&self) -> &ContextKey {
        &self.context
    }

    /// The bytes held.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// What a message counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    /// An API call: a rate token, an outstanding slot and its bytes.
    Call,
    /// A port message or event ack: a rate token and its bytes.
    Message,
}

#[derive(Debug)]
struct ContextUsage {
    bucket: TokenBucket,
    bytes: u64,
    calls: u32,
}

/// Per-context and per-install accounting.
#[derive(Debug, Default)]
pub struct Limiter {
    contexts: HashMap<ContextKey, ContextUsage>,
    installs: HashMap<ExtensionInstallId, u64>,
}

impl Limiter {
    /// An empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admits a message of `bytes` from `context`, or says which limit
    /// refused it. Nothing is charged on refusal except the rate token.
    pub fn admit(
        &mut self,
        context: &ContextKey,
        kind: ContextKind,
        bytes: u64,
        charge: Charge,
        now: u64,
    ) -> Result<Reservation, ApiError> {
        if bytes > message_limit(kind) {
            return Err(ApiError::message_too_large());
        }
        let usage = self
            .contexts
            .entry(context.clone())
            .or_insert_with(|| ContextUsage {
                bucket: TokenBucket::new(now),
                bytes: 0,
                calls: 0,
            });
        if !usage.bucket.try_take(now) {
            return Err(ApiError::rate_limited());
        }
        let install_bytes = self.installs.get(&context.install()).copied().unwrap_or(0);
        let call = charge == Charge::Call;
        if (call && usage.calls >= MAX_OUTSTANDING_CALLS_PER_CONTEXT)
            || usage.bytes + bytes > MAX_IN_FLIGHT_BYTES_PER_CONTEXT
            || install_bytes + bytes > MAX_IN_FLIGHT_BYTES_PER_INSTALL
        {
            return Err(ApiError::too_many_pending());
        }
        usage.bytes += bytes;
        usage.calls += u32::from(call);
        *self.installs.entry(context.install()).or_insert(0) += bytes;
        Ok(Reservation {
            context: context.clone(),
            bytes,
            call,
        })
    }

    /// Returns what `reservation` held.
    pub fn release(&mut self, reservation: Reservation) {
        if let Some(usage) = self.contexts.get_mut(&reservation.context) {
            usage.bytes = usage.bytes.saturating_sub(reservation.bytes);
            usage.calls = usage.calls.saturating_sub(u32::from(reservation.call));
        }
        let install = reservation.context.install();
        if let Some(bytes) = self.installs.get_mut(&install) {
            *bytes = bytes.saturating_sub(reservation.bytes);
            if *bytes == 0 {
                self.installs.remove(&install);
            }
        }
    }

    /// Bytes `context` has in flight.
    pub fn context_bytes(&self, context: &ContextKey) -> u64 {
        self.contexts.get(context).map_or(0, |usage| usage.bytes)
    }

    /// Calls `context` has outstanding.
    pub fn context_calls(&self, context: &ContextKey) -> u32 {
        self.contexts.get(context).map_or(0, |usage| usage.calls)
    }

    /// Bytes `install` has in flight.
    pub fn install_bytes(&self, install: ExtensionInstallId) -> u64 {
        self.installs.get(&install).copied().unwrap_or(0)
    }

    /// Forgets an idle context's rate state. Its outstanding reservations
    /// must have been released first, or the context is kept.
    pub fn forget_idle(&mut self, context: &ContextKey) {
        if self
            .contexts
            .get(context)
            .is_some_and(|usage| usage.bytes == 0 && usage.calls == 0)
        {
            self.contexts.remove(context);
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::context::tests::{background_sender, content_sender};

    #[test]
    fn table_values_are_the_design_values() {
        assert_eq!(MAX_MESSAGE_BYTES_RUNTIME_OWNED, 32 << 20);
        assert_eq!(MAX_MESSAGE_BYTES_IN_WEB_RENDERER, 8 << 20);
        assert_eq!(MAX_IN_FLIGHT_BYTES_PER_CONTEXT, 64 << 20);
        assert_eq!(MAX_IN_FLIGHT_BYTES_PER_INSTALL, 128 << 20);
        assert_eq!(STORAGE_LOCAL_UNLIMITED_QUOTA_BYTES, 1 << 30);
        assert_eq!(message_limit(ContextKind::Background), 32 << 20);
        assert_eq!(message_limit(ContextKind::ExtensionTab), 32 << 20);
        assert_eq!(message_limit(ContextKind::ExtensionFrame), 8 << 20);
        assert_eq!(message_limit(ContextKind::ContentScript), 8 << 20);
        assert_eq!(message_limit(ContextKind::ExternalPage), 8 << 20);
    }

    #[test]
    fn bucket_bursts_then_refills_at_the_rate() {
        let mut bucket = TokenBucket::new(0);
        for _ in 0..RATE_BURST {
            assert!(bucket.try_take(0));
        }
        assert!(!bucket.try_take(0));
        assert!(!bucket.try_take(NANOS_PER_TOKEN - 1));
        assert!(bucket.try_take(NANOS_PER_TOKEN));
        assert!(!bucket.try_take(NANOS_PER_TOKEN));
        let later = NANOS_PER_SECOND * 60;
        for _ in 0..RATE_BURST {
            assert!(bucket.try_take(later));
        }
        assert!(!bucket.try_take(later));
    }

    #[test]
    fn oversize_messages_are_refused_by_context_class() {
        let mut limiter = Limiter::new();
        let content = content_sender("https://example.com/");
        let key = content.context_key();
        assert_eq!(
            limiter.admit(
                &key,
                content.kind(),
                MAX_MESSAGE_BYTES_IN_WEB_RENDERER + 1,
                Charge::Message,
                0
            ),
            Err(ApiError::message_too_large())
        );
        let background = background_sender();
        let reservation = limiter
            .admit(
                &background.context_key(),
                background.kind(),
                MAX_MESSAGE_BYTES_IN_WEB_RENDERER + 1,
                Charge::Message,
                0,
            )
            .expect("runtime-owned contexts get more");
        limiter.release(reservation);
    }

    #[test]
    fn outstanding_calls_and_in_flight_bytes_are_capped_and_released() {
        let mut limiter = Limiter::new();
        let sender = background_sender();
        let key = sender.context_key();
        let mut held = Vec::new();
        for _ in 0..MAX_OUTSTANDING_CALLS_PER_CONTEXT {
            held.push(
                limiter
                    .admit(&key, sender.kind(), 1, Charge::Call, 0)
                    .expect("call"),
            );
        }
        assert_eq!(
            limiter.admit(&key, sender.kind(), 1, Charge::Call, 0),
            Err(ApiError::too_many_pending())
        );
        let message = limiter
            .admit(&key, sender.kind(), 1, Charge::Message, 0)
            .expect("messages do not hold call slots");
        held.push(message);
        for reservation in held {
            limiter.release(reservation);
        }
        assert_eq!(limiter.context_calls(&key), 0);
        assert_eq!(limiter.context_bytes(&key), 0);
        assert_eq!(limiter.install_bytes(key.install()), 0);
        limiter.forget_idle(&key);
        assert!(limiter.contexts.is_empty());
    }

    #[test]
    fn install_bytes_span_contexts() {
        let mut limiter = Limiter::new();
        let background = background_sender();
        let content = content_sender("https://example.com/");
        let mut held = Vec::new();
        let step = MAX_MESSAGE_BYTES_IN_WEB_RENDERER;
        let mut second = 0;
        while limiter.install_bytes(background.install()) + step <= MAX_IN_FLIGHT_BYTES_PER_INSTALL
        {
            let (sender, now) = if limiter.context_bytes(&background.context_key()) + step
                <= MAX_IN_FLIGHT_BYTES_PER_CONTEXT
            {
                (&background, 0)
            } else {
                second += 1;
                (&content, second * NANOS_PER_SECOND)
            };
            held.push(
                limiter
                    .admit(
                        &sender.context_key(),
                        sender.kind(),
                        step,
                        Charge::Message,
                        now,
                    )
                    .expect("within budget"),
            );
        }
        assert_eq!(
            limiter.admit(
                &content.context_key(),
                content.kind(),
                1,
                Charge::Message,
                u64::MAX / 2
            ),
            Err(ApiError::too_many_pending())
        );
        for reservation in held {
            limiter.release(reservation);
        }
        assert_eq!(limiter.install_bytes(background.install()), 0);
    }

    #[test]
    fn manual_clock_only_moves_forward() {
        let clock = ManualClock::default();
        clock.advance_nanos(5);
        clock.advance_nanos(u64::MAX);
        assert_eq!(clock.now_nanos(), u64::MAX);
        let monotonic = MonotonicClock::new();
        assert!(monotonic.now_nanos() <= monotonic.now_nanos());
    }

    proptest! {
        #[test]
        fn bucket_never_admits_more_than_burst_plus_rate(
            gaps in prop::collection::vec(0_u64..2_000_000, 1..2_000),
        ) {
            let mut bucket = TokenBucket::new(0);
            let mut now = 0_u64;
            let mut admitted = 0_u64;
            for gap in gaps {
                now += gap;
                if bucket.try_take(now) {
                    admitted += 1;
                }
                prop_assert!(bucket.tokens <= RATE_BURST);
                prop_assert!(
                    admitted <= u64::from(RATE_BURST) + now / NANOS_PER_TOKEN,
                    "admitted {} by {}ns", admitted, now
                );
            }
        }

        #[test]
        fn ledger_matches_a_model_and_returns_to_zero(
            operations in prop::collection::vec((any::<bool>(), 0_u64..(4 << 20), any::<bool>()), 1..400),
        ) {
            let mut limiter = Limiter::new();
            let senders = [background_sender(), content_sender("https://example.com/")];
            let mut held: Vec<Reservation> = Vec::new();
            let mut now = 0_u64;
            for (index, (admit, bytes, call)) in operations.into_iter().enumerate() {
                now += NANOS_PER_TOKEN;
                if admit || held.is_empty() {
                    let sender = &senders[index % 2];
                    let charge = if call { Charge::Call } else { Charge::Message };
                    if let Ok(reservation) =
                        limiter.admit(&sender.context_key(), sender.kind(), bytes, charge, now)
                    {
                        held.push(reservation);
                    }
                } else {
                    let reservation = held.swap_remove(index % held.len());
                    limiter.release(reservation);
                }
                for sender in &senders {
                    let key = sender.context_key();
                    let model_bytes: u64 = held
                        .iter()
                        .filter(|r| r.context == key)
                        .map(|r| r.bytes)
                        .sum();
                    let model_calls = held.iter().filter(|r| r.context == key && r.call).count();
                    prop_assert_eq!(limiter.context_bytes(&key), model_bytes);
                    prop_assert_eq!(limiter.context_calls(&key) as usize, model_calls);
                    prop_assert!(model_bytes <= MAX_IN_FLIGHT_BYTES_PER_CONTEXT);
                    prop_assert!(model_calls <= MAX_OUTSTANDING_CALLS_PER_CONTEXT as usize);
                }
                let install = senders[0].install();
                prop_assert!(limiter.install_bytes(install) <= MAX_IN_FLIGHT_BYTES_PER_INSTALL);
            }
            for reservation in held {
                limiter.release(reservation);
            }
            prop_assert_eq!(limiter.install_bytes(senders[0].install()), 0);
        }
    }
}
