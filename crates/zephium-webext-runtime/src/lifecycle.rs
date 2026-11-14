//! Pure profile-scoped background lifetime policy. Effects carry a generation;
//! stale native callbacks cannot stop or revive a successor background.

use std::collections::{BTreeMap, VecDeque};

use zephium_core::ids::{ExtensionInstallId, ProfileId};

use crate::errors::ApiError;
use crate::limits::{Clock, MAX_QUEUED_EVENTS_PER_BACKGROUND};

/// Maximum backgrounds occupying native slots in one profile.
pub const MAX_RUNNING_BACKGROUNDS: usize = 8;
/// Unblocked inactivity before a background is retired.
pub const IDLE_TIMEOUT_NANOS: u64 = 30_000_000_000;
/// Maximum processing time of one event or background startup.
pub const EVENT_TIMEOUT_NANOS: u64 = 300_000_000_000;
/// Window used to count repeated crashes.
pub const CRASH_WINDOW_NANOS: u64 = 300_000_000_000;
/// Crashes in one window that disable an install.
pub const MAX_CRASHES: usize = 5;
const CRASH_BACKOFF_NANOS: [u64; 3] = [1_000_000_000, 5_000_000_000, 30_000_000_000];

/// An admitted reason to start or wake an extension background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartReason {
    /// A newly installed extension receives `runtime.onInstalled`.
    Install,
    /// An extension update receives `runtime.onInstalled`.
    Update,
    /// Browser startup, only when a persisted startup listener exists.
    BrowserStartup,
    /// An event has a persisted listener.
    Event,
    /// A due alarm has a listener.
    Alarm,
    /// A message or connection needs a background receiver.
    Message,
    /// An explicit extension action needs a background.
    UserAction,
}

/// Why a native background must stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The unblocked idle timer expired.
    Idle,
    /// One event exceeded the processing deadline.
    EventTimeout,
    /// Background construction did not complete within its deadline.
    StartupTimeout,
    /// A queued extension needs its profile slot.
    Capacity,
    /// The adapter reported system memory pressure.
    MemoryPressure,
    /// Explicit user or policy disablement.
    Disabled,
    /// Profile retirement or browser shutdown.
    Shutdown,
}

/// Authoritative adapter counts that prevent idle termination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Blockers {
    /// Outstanding API calls from this background.
    pub calls: u32,
    /// Open runtime or tab ports.
    pub ports: u32,
    /// Events awaiting delivery or acknowledgment.
    pub pending_events: u32,
    /// Whether an offscreen document is live.
    pub offscreen_document: bool,
    /// Open native messaging ports.
    pub native_ports: u32,
}

impl Blockers {
    /// Whether at least one resource prevents idle retirement.
    pub fn any(self) -> bool {
        self.calls != 0
            || self.ports != 0
            || self.pending_events != 0
            || self.offscreen_document
            || self.native_ports != 0
    }
}

/// State of one install's background, without native handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundState {
    /// No native background exists.
    Stopped,
    /// Native construction was requested but has not completed.
    Starting,
    /// The native background is ready for events.
    Running,
    /// Cancellation and native destruction are awaiting acknowledgment.
    Terminating,
    /// The previous native process died; a new event may request restart.
    Crashed,
    /// Automatic starts are forbidden until explicitly enabled.
    Disabled,
}

/// Effects the adapter executes in returned order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Construct an isolated background, then observe `Started` or `Crashed`.
    Start {
        /// Install in this machine's profile.
        install: ExtensionInstallId,
        /// Fresh process generation.
        generation: u64,
        /// Admitted trigger.
        reason: StartReason,
    },
    /// Drain the event service's pending delivery queue into this generation.
    Deliver {
        /// Target install.
        install: ExtensionInstallId,
        /// Ready process generation.
        generation: u64,
    },
    /// Cancel bridge pulls, calls and ports before terminating the native view.
    CancelWork {
        /// Retiring install.
        install: ExtensionInstallId,
        /// Retiring process generation.
        generation: u64,
    },
    /// Terminate the native process, drop its view and observe `Terminated`.
    Terminate {
        /// Retiring install.
        install: ExtensionInstallId,
        /// Retiring process generation.
        generation: u64,
        /// Policy decision requiring retirement.
        reason: StopReason,
    },
    /// Surface automatic crash-loop disablement to the user.
    Disabled {
        /// Disabled install.
        install: ExtensionInstallId,
    },
}

/// An adapter observation or admitted wake request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    /// Listener/admission policy has established a reason to wake this install.
    Wake {
        /// Target install.
        install: ExtensionInstallId,
        /// Admitted reason.
        reason: StartReason,
    },
    /// Native background construction completed.
    Started {
        /// Target install.
        install: ExtensionInstallId,
        /// Generation returned by `Start`.
        generation: u64,
    },
    /// A ready background made API progress or received an event/message.
    Activity {
        /// Target install.
        install: ExtensionInstallId,
        /// Live generation.
        generation: u64,
    },
    /// Resource ownership changed. Opening a port is a blocker but does not
    /// itself count as message activity.
    Blockers {
        /// Target install.
        install: ExtensionInstallId,
        /// Live generation.
        generation: u64,
        /// Complete current resource counts.
        blockers: Blockers,
    },
    /// One delivered event began execution.
    EventStarted {
        /// Target install.
        install: ExtensionInstallId,
        /// Live generation.
        generation: u64,
        /// Event service sequence, unique within the live generation.
        event: u64,
    },
    /// One event completed; its individual cap no longer applies.
    EventFinished {
        /// Target install.
        install: ExtensionInstallId,
        /// Live generation.
        generation: u64,
        /// Previously started event.
        event: u64,
    },
    /// Native process death, including construction failure.
    Crashed {
        /// Target install.
        install: ExtensionInstallId,
        /// Dead generation.
        generation: u64,
    },
    /// Native termination and view destruction are complete.
    Terminated {
        /// Target install.
        install: ExtensionInstallId,
        /// Generation that no longer owns a native slot.
        generation: u64,
    },
    /// Inspect deadlines and start admitted requests whose backoff expired.
    Tick,
    /// Reclaim at most this many unblocked backgrounds in idle-LRU order.
    MemoryPressure {
        /// Maximum backgrounds to retire during this observation.
        reclaim: usize,
    },
    /// Explicit disablement cancels pending wake requests.
    Disable {
        /// Target install.
        install: ExtensionInstallId,
    },
    /// Explicit reenablement clears crash history but does not start a process.
    Enable {
        /// Target install.
        install: ExtensionInstallId,
    },
    /// Cancel all profile backgrounds; no future wake request is admitted.
    Shutdown,
}

#[derive(Clone, Debug)]
struct Background {
    state: BackgroundState,
    generation: u64,
    last_activity: u64,
    started_at: u64,
    idle_since: Option<u64>,
    blockers: Blockers,
    events: BTreeMap<u64, u64>,
    crashes: VecDeque<u64>,
    retry_at: u64,
    wake: Option<(u64, StartReason)>,
    disabled: bool,
}

impl Background {
    fn new() -> Self {
        Self {
            state: BackgroundState::Stopped,
            generation: 0,
            last_activity: 0,
            started_at: 0,
            idle_since: None,
            blockers: Blockers::default(),
            events: BTreeMap::new(),
            crashes: VecDeque::new(),
            retry_at: 0,
            wake: None,
            disabled: false,
        }
    }

    fn occupies_slot(&self) -> bool {
        matches!(
            self.state,
            BackgroundState::Starting | BackgroundState::Running | BackgroundState::Terminating
        )
    }

    fn is_idle(&self) -> bool {
        self.state == BackgroundState::Running && !self.blockers.any() && self.events.is_empty()
    }

    fn update_idle(&mut self, now: u64) {
        if self.is_idle() {
            self.idle_since.get_or_insert(now);
        } else {
            self.idle_since = None;
        }
    }

    fn activity(&mut self, now: u64) {
        self.last_activity = now;
        self.idle_since = None;
        self.update_idle(now);
    }

    fn terminate(
        &mut self,
        install: ExtensionInstallId,
        reason: StopReason,
        commands: &mut Vec<Command>,
    ) {
        if !matches!(
            self.state,
            BackgroundState::Starting | BackgroundState::Running
        ) {
            return;
        }
        self.state = BackgroundState::Terminating;
        self.idle_since = None;
        commands.push(Command::CancelWork {
            install,
            generation: self.generation,
        });
        commands.push(Command::Terminate {
            install,
            generation: self.generation,
            reason,
        });
    }

    fn clear_resources(&mut self) {
        self.blockers = Blockers::default();
        self.events.clear();
        self.idle_since = None;
    }

    fn prune_crashes(&mut self, now: u64) {
        while self
            .crashes
            .front()
            .is_some_and(|crash| now.saturating_sub(*crash) >= CRASH_WINDOW_NANOS)
        {
            self.crashes.pop_front();
        }
    }
}

/// One profile's background policy and running-slot ledger. Instantiate a
/// separate machine for a private profile. Events and payloads stay in the
/// event service; this machine only asks the adapter to drain them.
#[derive(Clone, Debug)]
pub struct Lifecycle {
    profile: ProfileId,
    backgrounds: BTreeMap<ExtensionInstallId, Background>,
    next_wake_order: u64,
    next_generation: u64,
    shutting_down: bool,
}

impl Lifecycle {
    /// An empty profile with the policy's eight-background native cap.
    pub fn new(profile: ProfileId) -> Self {
        Self {
            profile,
            backgrounds: BTreeMap::new(),
            next_wake_order: 0,
            next_generation: 0,
            shutting_down: false,
        }
    }

    /// Profile to which all registered installs belong.
    pub fn profile(&self) -> ProfileId {
        self.profile
    }

    /// Registers an admitted install. Duplicate registration preserves state.
    pub fn register(&mut self, install: ExtensionInstallId) -> bool {
        if self.shutting_down || self.backgrounds.contains_key(&install) {
            return false;
        }
        self.backgrounds.insert(install, Background::new());
        true
    }

    /// Forgets an install only after native retirement has completed.
    pub fn forget(&mut self, install: ExtensionInstallId) -> bool {
        if self
            .backgrounds
            .get(&install)
            .is_some_and(|background| !background.occupies_slot())
        {
            self.backgrounds.remove(&install);
            return true;
        }
        false
    }

    /// Current state, if this profile admitted the install.
    pub fn state(&self, install: ExtensionInstallId) -> Option<BackgroundState> {
        self.backgrounds
            .get(&install)
            .map(|background| background.state)
    }

    /// Current process generation, including a retired or constructing one.
    pub fn generation(&self, install: ExtensionInstallId) -> Option<u64> {
        self.backgrounds
            .get(&install)
            .map(|background| background.generation)
    }

    /// Native slots held, including construction and pending destruction.
    pub fn occupied_slots(&self) -> usize {
        self.backgrounds
            .values()
            .filter(|background| background.occupies_slot())
            .count()
    }

    /// Earliest useful timer deadline. Queued requests awaiting a native slot
    /// resume on a termination/blocker observation rather than busy polling.
    pub fn next_wake(&self) -> Option<u64> {
        let can_admit = self.occupied_slots() < MAX_RUNNING_BACKGROUNDS
            || (!self
                .backgrounds
                .values()
                .any(|background| background.state == BackgroundState::Terminating)
                && self.backgrounds.values().any(Background::is_idle));
        self.backgrounds
            .values()
            .flat_map(|background| {
                let idle = background
                    .idle_since
                    .map(|since| since.saturating_add(IDLE_TIMEOUT_NANOS));
                let event = if background.state == BackgroundState::Running {
                    background
                        .events
                        .values()
                        .min()
                        .map(|started| started.saturating_add(EVENT_TIMEOUT_NANOS))
                } else {
                    None
                };
                let startup = (background.state == BackgroundState::Starting)
                    .then_some(background.started_at.saturating_add(EVENT_TIMEOUT_NANOS));
                let retry = (can_admit
                    && background.state == BackgroundState::Crashed
                    && background.wake.is_some())
                .then_some(background.retry_at);
                [idle, event, startup, retry].into_iter().flatten()
            })
            .min()
    }

    /// Applies an observation at monotonic clock time and returns ordered
    /// adapter effects. Unknown installs are refused; stale process callbacks
    /// and duplicate event starts/completions are harmless no-ops.
    pub fn observe(
        &mut self,
        observation: Observation,
        clock: &impl Clock,
    ) -> Result<Vec<Command>, ApiError> {
        let now = clock.now_nanos();
        let mut commands = Vec::new();
        match observation {
            Observation::Tick => self.expire(now, &mut commands),
            Observation::MemoryPressure { reclaim } => {
                let candidates = self.idle_candidates();
                for install in candidates.into_iter().take(reclaim) {
                    if let Some(background) = self.backgrounds.get_mut(&install) {
                        background.terminate(install, StopReason::MemoryPressure, &mut commands);
                    }
                }
            }
            Observation::Shutdown => {
                self.shutting_down = true;
                for (&install, background) in &mut self.backgrounds {
                    background.wake = None;
                    background.disabled = true;
                    background.terminate(install, StopReason::Shutdown, &mut commands);
                    if !background.occupies_slot() {
                        background.state = BackgroundState::Disabled;
                    }
                }
            }
            observation => self.observe_install(observation, now, &mut commands)?,
        }
        self.schedule(now, &mut commands);
        Ok(commands)
    }

    fn observe_install(
        &mut self,
        observation: Observation,
        now: u64,
        commands: &mut Vec<Command>,
    ) -> Result<(), ApiError> {
        let install = match observation {
            Observation::Wake { install, .. }
            | Observation::Started { install, .. }
            | Observation::Activity { install, .. }
            | Observation::Blockers { install, .. }
            | Observation::EventStarted { install, .. }
            | Observation::EventFinished { install, .. }
            | Observation::Crashed { install, .. }
            | Observation::Terminated { install, .. }
            | Observation::Disable { install }
            | Observation::Enable { install } => install,
            _ => return Ok(()),
        };
        let background = self
            .backgrounds
            .get_mut(&install)
            .ok_or_else(ApiError::context_invalidated)?;
        match observation {
            Observation::Wake { reason, .. } => {
                if self.shutting_down || background.disabled {
                    return Err(ApiError::context_invalidated());
                }
                if background.state == BackgroundState::Running {
                    background.activity(now);
                    commands.push(Command::Deliver {
                        install,
                        generation: background.generation,
                    });
                } else if background.wake.is_none() {
                    self.next_wake_order = self
                        .next_wake_order
                        .checked_add(1)
                        .ok_or_else(|| ApiError::other("Background wake sequence exhausted."))?;
                    background.wake = Some((self.next_wake_order, reason));
                }
            }
            Observation::Started { generation, .. }
                if background.generation == generation
                    && background.state == BackgroundState::Starting =>
            {
                background.state = BackgroundState::Running;
                background.wake = None;
                background.activity(now);
                commands.push(Command::Deliver {
                    install,
                    generation,
                });
            }
            Observation::Activity { generation, .. } if live(background, generation) => {
                background.activity(now);
            }
            Observation::Blockers {
                generation,
                blockers,
                ..
            } if live(background, generation) => {
                background.blockers = blockers;
                background.update_idle(now);
            }
            Observation::EventStarted {
                generation, event, ..
            } if live(background, generation) && !background.events.contains_key(&event) => {
                if event == 0 {
                    return Err(ApiError::other("Event sequence must be nonzero."));
                }
                if background.events.len() >= MAX_QUEUED_EVENTS_PER_BACKGROUND as usize {
                    return Err(ApiError::too_many_pending());
                }
                background.events.insert(event, now);
                background.activity(now);
            }
            Observation::EventFinished {
                generation, event, ..
            } if live(background, generation) && background.events.remove(&event).is_some() => {
                background.activity(now);
            }
            Observation::Crashed { generation, .. } => {
                if background.generation != generation || !background.occupies_slot() {
                    return Ok(());
                }
                commands.push(Command::CancelWork {
                    install,
                    generation,
                });
                background.clear_resources();
                if background.disabled {
                    background.state = BackgroundState::Disabled;
                    return Ok(());
                }
                background.prune_crashes(now);
                background.crashes.push_back(now);
                if background.crashes.len() >= MAX_CRASHES {
                    background.state = BackgroundState::Disabled;
                    background.disabled = true;
                    background.wake = None;
                    commands.push(Command::Disabled { install });
                } else {
                    background.state = BackgroundState::Crashed;
                    let backoff = CRASH_BACKOFF_NANOS[(background.crashes.len() - 1).min(2)];
                    background.retry_at = now.saturating_add(backoff);
                }
            }
            Observation::Terminated { generation, .. }
                if background.generation == generation
                    && background.state == BackgroundState::Terminating =>
            {
                background.state = if background.disabled {
                    BackgroundState::Disabled
                } else {
                    BackgroundState::Stopped
                };
                background.clear_resources();
            }
            Observation::Disable { .. } => {
                background.disabled = true;
                background.wake = None;
                background.terminate(install, StopReason::Disabled, commands);
                if !background.occupies_slot() {
                    background.state = BackgroundState::Disabled;
                }
            }
            Observation::Enable { .. } => {
                if self.shutting_down || background.occupies_slot() {
                    return Err(ApiError::context_invalidated());
                }
                background.disabled = false;
                background.crashes.clear();
                background.retry_at = 0;
                background.state = BackgroundState::Stopped;
            }
            _ => {}
        }
        Ok(())
    }

    fn expire(&mut self, now: u64, commands: &mut Vec<Command>) {
        for (&install, background) in &mut self.backgrounds {
            background.prune_crashes(now);
            let reason = if background.state == BackgroundState::Starting
                && now.saturating_sub(background.started_at) >= EVENT_TIMEOUT_NANOS
            {
                Some(StopReason::StartupTimeout)
            } else if background.state == BackgroundState::Running
                && background
                    .events
                    .values()
                    .any(|started| now.saturating_sub(*started) >= EVENT_TIMEOUT_NANOS)
            {
                Some(StopReason::EventTimeout)
            } else if background
                .idle_since
                .is_some_and(|since| now.saturating_sub(since) >= IDLE_TIMEOUT_NANOS)
            {
                Some(StopReason::Idle)
            } else {
                None
            };
            if let Some(reason) = reason {
                background.terminate(install, reason, commands);
            }
        }
    }

    fn idle_candidates(&self) -> Vec<ExtensionInstallId> {
        let mut idle: Vec<_> = self
            .backgrounds
            .iter()
            .filter(|(_, background)| background.is_idle())
            .map(|(&install, background)| (background.last_activity, install))
            .collect();
        idle.sort();
        idle.into_iter().map(|(_, install)| install).collect()
    }

    fn schedule(&mut self, now: u64, commands: &mut Vec<Command>) {
        if self.shutting_down {
            return;
        }
        loop {
            let candidate = self
                .backgrounds
                .iter()
                .filter(|(_, background)| {
                    !background.disabled
                        && matches!(
                            background.state,
                            BackgroundState::Stopped | BackgroundState::Crashed
                        )
                        && (background.state != BackgroundState::Crashed
                            || now >= background.retry_at)
                })
                .filter_map(|(&install, background)| {
                    background
                        .wake
                        .map(|(order, reason)| (order, install, reason))
                })
                .min_by_key(|(order, install, _)| (*order, *install));
            let Some((_, install, reason)) = candidate else {
                break;
            };
            if self.occupied_slots() >= MAX_RUNNING_BACKGROUNDS {
                // Already requested destruction will free a slot; never evict
                // another background for the same waiting request.
                if !self
                    .backgrounds
                    .values()
                    .any(|background| background.state == BackgroundState::Terminating)
                {
                    if let Some(victim) = self.idle_candidates().first().copied() {
                        if let Some(background) = self.backgrounds.get_mut(&victim) {
                            background.terminate(victim, StopReason::Capacity, commands);
                        }
                    }
                }
                break;
            }
            let Some(background) = self.backgrounds.get_mut(&install) else {
                break;
            };
            let Some(generation) = self.next_generation.checked_add(1) else {
                background.disabled = true;
                background.state = BackgroundState::Disabled;
                background.wake = None;
                commands.push(Command::Disabled { install });
                continue;
            };
            self.next_generation = generation;
            background.generation = generation;
            background.state = BackgroundState::Starting;
            background.started_at = now;
            background.wake = None;
            commands.push(Command::Start {
                install,
                generation,
                reason,
            });
        }
    }
}

fn live(background: &Background, generation: u64) -> bool {
    background.state == BackgroundState::Running && background.generation == generation
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::limits::ManualClock;

    fn install(value: u128) -> ExtensionInstallId {
        ExtensionInstallId::from(value)
    }

    fn started(machine: &mut Lifecycle, id: ExtensionInstallId, clock: &ManualClock) -> u64 {
        machine.register(id);
        let commands = machine
            .observe(
                Observation::Wake {
                    install: id,
                    reason: StartReason::Event,
                },
                clock,
            )
            .unwrap();
        let generation = commands
            .iter()
            .find_map(|command| match command {
                Command::Start { generation, .. } => Some(*generation),
                _ => None,
            })
            .expect("start");
        assert_eq!(
            machine
                .observe(
                    Observation::Started {
                        install: id,
                        generation
                    },
                    clock
                )
                .unwrap(),
            [Command::Deliver {
                install: id,
                generation
            }]
        );
        generation
    }

    #[test]
    fn idle_requires_thirty_unblocked_seconds_and_teardown_ack() {
        let clock = ManualClock::default();
        let mut machine = Lifecycle::new(ProfileId::from(1));
        let id = install(1);
        let generation = started(&mut machine, id, &clock);
        clock.advance_nanos(IDLE_TIMEOUT_NANOS - 1);
        assert!(machine
            .observe(Observation::Tick, &clock)
            .unwrap()
            .is_empty());
        for blockers in [
            Blockers {
                calls: 1,
                ..Blockers::default()
            },
            Blockers {
                ports: 1,
                ..Blockers::default()
            },
            Blockers {
                pending_events: 1,
                ..Blockers::default()
            },
            Blockers {
                offscreen_document: true,
                ..Blockers::default()
            },
            Blockers {
                native_ports: 1,
                ..Blockers::default()
            },
        ] {
            machine
                .observe(
                    Observation::Blockers {
                        install: id,
                        generation,
                        blockers,
                    },
                    &clock,
                )
                .unwrap();
            clock.advance_nanos(IDLE_TIMEOUT_NANOS);
            assert!(machine
                .observe(Observation::Tick, &clock)
                .unwrap()
                .is_empty());
        }
        machine
            .observe(
                Observation::Blockers {
                    install: id,
                    generation,
                    blockers: Blockers::default(),
                },
                &clock,
            )
            .unwrap();
        clock.advance_nanos(IDLE_TIMEOUT_NANOS);
        assert_eq!(
            machine.observe(Observation::Tick, &clock).unwrap(),
            [
                Command::CancelWork {
                    install: id,
                    generation
                },
                Command::Terminate {
                    install: id,
                    generation,
                    reason: StopReason::Idle
                }
            ]
        );
        assert_eq!(machine.occupied_slots(), 1);
        assert!(machine
            .observe(Observation::Tick, &clock)
            .unwrap()
            .is_empty());
        machine
            .observe(
                Observation::Terminated {
                    install: id,
                    generation,
                },
                &clock,
            )
            .unwrap();
        assert_eq!(machine.occupied_slots(), 0);
    }

    #[test]
    fn each_event_has_its_own_cap_and_activity_cannot_extend_it() {
        let clock = ManualClock::default();
        let mut machine = Lifecycle::new(ProfileId::from(1));
        let id = install(1);
        let generation = started(&mut machine, id, &clock);
        machine
            .observe(
                Observation::EventStarted {
                    install: id,
                    generation,
                    event: 1,
                },
                &clock,
            )
            .unwrap();
        clock.advance_nanos(EVENT_TIMEOUT_NANOS / 2);
        machine
            .observe(
                Observation::EventStarted {
                    install: id,
                    generation,
                    event: 2,
                },
                &clock,
            )
            .unwrap();
        machine
            .observe(
                Observation::EventStarted {
                    install: id,
                    generation,
                    event: 1,
                },
                &clock,
            )
            .unwrap();
        machine
            .observe(
                Observation::Activity {
                    install: id,
                    generation,
                },
                &clock,
            )
            .unwrap();
        clock.advance_nanos(EVENT_TIMEOUT_NANOS / 2);
        assert!(matches!(
            machine.observe(Observation::Tick, &clock).unwrap().last(),
            Some(Command::Terminate {
                reason: StopReason::EventTimeout,
                ..
            })
        ));
    }

    #[test]
    fn crash_backoff_requires_a_new_wake_and_fifth_crash_disables() {
        let clock = ManualClock::default();
        let mut machine = Lifecycle::new(ProfileId::from(1));
        let id = install(1);
        let mut generation = started(&mut machine, id, &clock);
        for attempt in 0..MAX_CRASHES {
            let commands = machine
                .observe(
                    Observation::Crashed {
                        install: id,
                        generation,
                    },
                    &clock,
                )
                .unwrap();
            if attempt == MAX_CRASHES - 1 {
                assert_eq!(commands.last(), Some(&Command::Disabled { install: id }));
                assert_eq!(machine.state(id), Some(BackgroundState::Disabled));
                assert!(machine
                    .observe(
                        Observation::Wake {
                            install: id,
                            reason: StartReason::Event
                        },
                        &clock
                    )
                    .is_err());
                break;
            }
            clock.advance_nanos(1);
            assert!(machine
                .observe(Observation::Tick, &clock)
                .unwrap()
                .is_empty());
            assert!(machine
                .observe(
                    Observation::Wake {
                        install: id,
                        reason: StartReason::Event
                    },
                    &clock
                )
                .unwrap()
                .is_empty());
            clock.advance_nanos(CRASH_BACKOFF_NANOS[attempt.min(2)] - 1);
            let commands = machine.observe(Observation::Tick, &clock).unwrap();
            assert!(matches!(commands.first(), Some(Command::Start { .. })));
            generation = machine.generation(id).unwrap();
            machine
                .observe(
                    Observation::Started {
                        install: id,
                        generation,
                    },
                    &clock,
                )
                .unwrap();
        }
        machine
            .observe(Observation::Enable { install: id }, &clock)
            .unwrap();
        assert_eq!(machine.state(id), Some(BackgroundState::Stopped));
    }

    #[test]
    fn crash_window_expires_and_stale_callbacks_do_not_touch_successors() {
        let clock = ManualClock::default();
        let mut machine = Lifecycle::new(ProfileId::from(1));
        let id = install(1);
        let generation = started(&mut machine, id, &clock);
        machine
            .observe(
                Observation::Crashed {
                    install: id,
                    generation,
                },
                &clock,
            )
            .unwrap();
        clock.advance_nanos(CRASH_WINDOW_NANOS);
        machine
            .observe(
                Observation::Wake {
                    install: id,
                    reason: StartReason::Alarm,
                },
                &clock,
            )
            .unwrap();
        let successor = machine.generation(id).unwrap();
        machine
            .observe(
                Observation::Started {
                    install: id,
                    generation: successor,
                },
                &clock,
            )
            .unwrap();
        assert!(machine
            .observe(
                Observation::Crashed {
                    install: id,
                    generation
                },
                &clock
            )
            .unwrap()
            .is_empty());
        assert!(machine
            .observe(
                Observation::Terminated {
                    install: id,
                    generation
                },
                &clock
            )
            .unwrap()
            .is_empty());
        assert_eq!(machine.state(id), Some(BackgroundState::Running));
        machine
            .observe(
                Observation::Crashed {
                    install: id,
                    generation: successor,
                },
                &clock,
            )
            .unwrap();
        machine
            .observe(
                Observation::Wake {
                    install: id,
                    reason: StartReason::Event,
                },
                &clock,
            )
            .unwrap();
        assert_eq!(
            machine.next_wake(),
            Some(clock.now_nanos() + CRASH_BACKOFF_NANOS[0])
        );
    }

    #[test]
    fn capacity_waits_for_native_death_and_evicts_idle_lru_only() {
        let clock = ManualClock::default();
        let mut machine = Lifecycle::new(ProfileId::from(1));
        for i in 1..=MAX_RUNNING_BACKGROUNDS {
            let generation = started(&mut machine, install(i as u128), &clock);
            if i == 1 {
                machine
                    .observe(
                        Observation::Blockers {
                            install: install(1),
                            generation,
                            blockers: Blockers {
                                calls: 1,
                                ..Blockers::default()
                            },
                        },
                        &clock,
                    )
                    .unwrap();
            }
            clock.advance_nanos(1);
        }
        let extra = install(99);
        let victim_generation = machine.generation(install(2)).unwrap();
        machine.register(extra);
        let commands = machine
            .observe(
                Observation::Wake {
                    install: extra,
                    reason: StartReason::Message,
                },
                &clock,
            )
            .unwrap();
        assert_eq!(
            commands.last(),
            Some(&Command::Terminate {
                install: install(2),
                generation: victim_generation,
                reason: StopReason::Capacity
            })
        );
        assert_eq!(machine.occupied_slots(), MAX_RUNNING_BACKGROUNDS);
        assert!(machine
            .observe(Observation::Tick, &clock)
            .unwrap()
            .is_empty());
        let commands = machine
            .observe(
                Observation::Terminated {
                    install: install(2),
                    generation: victim_generation,
                },
                &clock,
            )
            .unwrap();
        assert_eq!(
            commands,
            [Command::Start {
                install: extra,
                generation: MAX_RUNNING_BACKGROUNDS as u64 + 1,
                reason: StartReason::Message
            }]
        );
        assert_eq!(machine.occupied_slots(), MAX_RUNNING_BACKGROUNDS);
    }

    #[test]
    fn pressure_never_reclaims_busy_process_and_shutdown_cannot_revive() {
        let clock = ManualClock::default();
        let mut machine = Lifecycle::new(ProfileId::from(1));
        let generation = started(&mut machine, install(1), &clock);
        let second_generation = started(&mut machine, install(2), &clock);
        machine
            .observe(
                Observation::EventStarted {
                    install: install(1),
                    generation,
                    event: 1,
                },
                &clock,
            )
            .unwrap();
        let commands = machine
            .observe(Observation::MemoryPressure { reclaim: 20 }, &clock)
            .unwrap();
        assert_eq!(
            commands.last(),
            Some(&Command::Terminate {
                install: install(2),
                generation: second_generation,
                reason: StopReason::MemoryPressure
            })
        );
        machine.observe(Observation::Shutdown, &clock).unwrap();
        assert!(machine
            .observe(
                Observation::Wake {
                    install: install(1),
                    reason: StartReason::Event
                },
                &clock
            )
            .is_err());
        machine
            .observe(
                Observation::Terminated {
                    install: install(1),
                    generation,
                },
                &clock,
            )
            .unwrap();
        assert!(machine
            .observe(
                Observation::Enable {
                    install: install(1)
                },
                &clock
            )
            .is_err());
        assert!(!machine.register(install(3)));
    }

    #[test]
    fn forgotten_install_cannot_reuse_a_generation_from_delayed_callbacks() {
        let clock = ManualClock::default();
        let mut machine = Lifecycle::new(ProfileId::from(1));
        let id = install(1);
        let old = started(&mut machine, id, &clock);
        machine
            .observe(Observation::Disable { install: id }, &clock)
            .unwrap();
        assert!(!machine.forget(id));
        machine
            .observe(
                Observation::Terminated {
                    install: id,
                    generation: old,
                },
                &clock,
            )
            .unwrap();
        assert!(machine.forget(id));
        let new = started(&mut machine, id, &clock);
        assert!(new > old);
        assert!(machine
            .observe(
                Observation::Crashed {
                    install: id,
                    generation: old
                },
                &clock
            )
            .unwrap()
            .is_empty());
        assert_eq!(machine.state(id), Some(BackgroundState::Running));
    }

    proptest! {
        #[test]
        fn idle_transitions_match_an_independent_timer_model(
            operations in prop::collection::vec((0u8..3, any::<bool>(), 0u64..40_000_000_000), 1..100)
        ) {
            let clock = ManualClock::default();
            let mut machine = Lifecycle::new(ProfileId::from(1));
            let id = install(1);
            let generation = started(&mut machine, id, &clock);
            let mut blocked = false;
            let mut idle_since = 0;
            let mut running = true;
            for (operation, next_blocked, advance) in operations {
                clock.advance_nanos(advance);
                let now = clock.now_nanos();
                let observation = match operation {
                    0 => {
                        if blocked && !next_blocked { idle_since = now; }
                        blocked = next_blocked;
                        Observation::Blockers { install: id, generation, blockers: Blockers { ports: u32::from(blocked), ..Blockers::default() } }
                    }
                    1 => {
                        idle_since = now;
                        Observation::Activity { install: id, generation }
                    }
                    _ => Observation::Tick,
                };
                let expected_termination = running && operation == 2 && !blocked && now - idle_since >= IDLE_TIMEOUT_NANOS;
                let commands = machine.observe(observation, &clock).unwrap();
                prop_assert_eq!(commands.iter().any(|command| matches!(command, Command::Terminate { reason: StopReason::Idle, .. })), expected_termination);
                if expected_termination { running = false; }
                prop_assert_eq!(machine.state(id), Some(if running { BackgroundState::Running } else { BackgroundState::Terminating }));
            }
        }

        #[test]
        fn arbitrary_observations_preserve_slot_cap_and_generation_ownership(
            operations in prop::collection::vec((0u8..12, 1u128..14, 0u64..4, 0u64..40_000_000_000), 0..400)
        ) {
            let clock = ManualClock::default();
            let mut machine = Lifecycle::new(ProfileId::from(1));
            for i in 1..14 { machine.register(install(i)); }
            let mut last_generations = BTreeMap::<ExtensionInstallId, u64>::new();
            for (operation, raw, modifier, advance) in operations {
                let id = install(raw);
                clock.advance_nanos(advance);
                let current = machine.generation(id).unwrap_or(0);
                let generation = if modifier == 0 { current.saturating_sub(1) } else { current };
                let observation = match operation {
                    0 => Observation::Wake { install: id, reason: StartReason::Event },
                    1 => Observation::Started { install: id, generation },
                    2 => Observation::Crashed { install: id, generation },
                    3 => Observation::Terminated { install: id, generation },
                    4 => Observation::Blockers { install: id, generation, blockers: Blockers { calls: modifier as u32, ..Blockers::default() } },
                    5 => Observation::EventStarted { install: id, generation, event: modifier + 1 },
                    6 => Observation::EventFinished { install: id, generation, event: modifier + 1 },
                    7 => Observation::Activity { install: id, generation },
                    8 => Observation::Disable { install: id },
                    9 => Observation::Enable { install: id },
                    10 => Observation::MemoryPressure { reclaim: modifier as usize },
                    _ => Observation::Tick,
                };
                if let Ok(commands) = machine.observe(observation, &clock) {
                    for command in commands {
                        if let Command::Start { install, generation, .. } = command {
                            prop_assert!(generation > last_generations.get(&install).copied().unwrap_or(0));
                            last_generations.insert(install, generation);
                        }
                    }
                }
                prop_assert!(machine.occupied_slots() <= MAX_RUNNING_BACKGROUNDS);
                for (&install, background) in &machine.backgrounds {
                    prop_assert!(background.events.len() <= MAX_QUEUED_EVENTS_PER_BACKGROUND as usize);
                    prop_assert!(background.crashes.len() <= MAX_CRASHES);
                    if background.state == BackgroundState::Running {
                        prop_assert_eq!(last_generations.get(&install).copied(), Some(background.generation));
                    }
                    if background.idle_since.is_some() { prop_assert!(background.is_idle()); }
                }
            }
        }
    }
}
