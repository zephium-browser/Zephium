//! Install-scoped alarm scheduling. Adapters supply wall time, own persistence
//! and enqueue returned alarms before committing the updated snapshot.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::Sender;
use crate::dispatch::{ApiTable, RegistrationError};
use crate::errors::ApiError;
use crate::limits::Clock;

/// Maximum active alarms for one install.
pub const MAX_ALARMS: usize = 500;
/// Maximum UTF-8 bytes in an alarm name.
pub const MAX_ALARM_NAME_BYTES: usize = 1024;
/// Maximum bytes accepted when decoding a durable alarm snapshot.
pub const MAX_ALARM_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
/// The runtime's packed-extension alarm minimum, as required by its policy.
pub const MIN_PACKED_DELAY_NANOS: u64 = 30_000_000_000;
/// Chromium's development minimum; a zero-period alarm cannot spin.
pub const MIN_UNPACKED_DELAY_NANOS: u64 = 1_000_000_000;
const NANOS_PER_MILLI: f64 = 1_000_000.0;
const MILLIS_PER_MINUTE: f64 = 60_000.0;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// A single adapter observation of monotonic and Unix time.
#[derive(Clone, Copy, Debug)]
pub struct AlarmEnvironment {
    /// Process monotonic time from [`Clock`].
    pub monotonic_nanos: u64,
    /// Milliseconds since the Unix epoch.
    pub unix_millis: u64,
    /// Whether installation policy admits development alarm timing.
    pub unpacked: bool,
}

impl AlarmEnvironment {
    /// Joins a monotonic clock reading to adapter-supplied wall time.
    pub fn now(clock: &impl Clock, unix_millis: u64, unpacked: bool) -> Self {
        Self {
            monotonic_nanos: clock.now_nanos(),
            unix_millis,
            unpacked,
        }
    }

    fn minimum(self) -> u64 {
        if self.unpacked {
            MIN_UNPACKED_DELAY_NANOS
        } else {
            MIN_PACKED_DELAY_NANOS
        }
    }
}

/// Arguments describing an alarm, without the separate optional name argument.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlarmCreateInfo {
    /// Optional object-form name, mutually exclusive with a separate name.
    pub name: Option<String>,
    /// Absolute Unix time in milliseconds.
    pub when: Option<f64>,
    /// Delay before the initial firing, in minutes.
    pub delay_in_minutes: Option<f64>,
    /// Repeat interval, in minutes.
    pub period_in_minutes: Option<f64>,
    /// Whether browser restarts retain this alarm; defaults to true.
    pub persist_across_sessions: Option<bool>,
}

/// The Chrome-shaped alarm returned by queries and `alarms.onAlarm`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Alarm {
    /// Alarm name, or the empty string for an unnamed alarm.
    pub name: String,
    /// Requested next firing time, in Unix milliseconds.
    pub scheduled_time: f64,
    /// Requested repeat period, if any. Runtime minimums still apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_in_minutes: Option<f64>,
    /// Whether browser restarts retain this alarm.
    pub persist_across_sessions: bool,
}

/// Versioned persistent state for one profile/install instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlarmSnapshot {
    /// Snapshot format version, currently one.
    pub version: u32,
    /// Only alarms that persist across browser sessions.
    pub alarms: Vec<PersistedAlarm>,
}

/// Persisted alarm plus its effective wake time after minimum-delay policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedAlarm {
    /// Chrome-shaped data, retaining the requested scheduled time.
    pub alarm: Alarm,
    /// Effective Unix wake time; may be later than the requested time.
    pub wake_unix_millis: f64,
}

#[derive(Clone, Debug)]
struct Scheduled {
    alarm: Alarm,
    deadline: u64,
    period: Option<u64>,
    wake_unix_millis: f64,
}

/// Alarms for exactly one install and private/non-private profile instance.
/// Keep this service when its background stops; persist [`Self::snapshot`]
/// separately for browser restarts. Install updates clear all alarms.
#[derive(Clone, Debug, Default)]
pub struct Alarms {
    alarms: BTreeMap<String, Scheduled>,
}

impl Alarms {
    /// An empty install alarm service.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates or replaces an alarm atomically after validating every field.
    pub fn create(
        &mut self,
        name: Option<&str>,
        info: AlarmCreateInfo,
        now: AlarmEnvironment,
    ) -> Result<(), ApiError> {
        self.create_with_warnings(name, info, now).map(|_| ())
    }

    /// Creates an alarm and returns diagnostics for the calling context's console.
    pub fn create_with_warnings(
        &mut self,
        name: Option<&str>,
        info: AlarmCreateInfo,
        now: AlarmEnvironment,
    ) -> Result<Vec<String>, ApiError> {
        let warnings = alarm_warnings(name, &info, now);
        let (name, alarm) = prepare(name, info, now)?;
        if !self.alarms.contains_key(&name) && self.alarms.len() >= MAX_ALARMS {
            return Err(ApiError::other(
                "An extension cannot have more than 500 active alarms.",
            ));
        }
        self.alarms.insert(name, alarm);
        Ok(warnings)
    }

    /// Looks up one alarm without changing its schedule.
    pub fn get(&self, name: &str) -> Option<&Alarm> {
        self.alarms.get(name).map(|scheduled| &scheduled.alarm)
    }

    /// Queries alarms in stable name order.
    pub fn get_all(&self) -> Vec<Alarm> {
        self.alarms
            .values()
            .map(|item| item.alarm.clone())
            .collect()
    }

    /// Removes one alarm and reports whether it existed.
    pub fn clear(&mut self, name: &str) -> bool {
        self.alarms.remove(name).is_some()
    }

    /// Removes all alarms. Chrome's boolean form always returns true.
    pub fn clear_all(&mut self) -> bool {
        self.alarms.clear();
        true
    }

    /// Earliest monotonic wake deadline for the adapter's timer.
    pub fn next_wake(&self) -> Option<u64> {
        self.alarms.values().map(|item| item.deadline).min()
    }

    /// Fires each due alarm once, oldest deadline first, with name tie-breaking.
    /// Missed repeat intervals coalesce and restart from this observation.
    /// Wall-clock changes do not move running monotonic timers.
    pub fn take_due(&mut self, now: AlarmEnvironment) -> Vec<Alarm> {
        let mut due: Vec<_> = self
            .alarms
            .iter()
            .filter(|(_, scheduled)| scheduled.deadline <= now.monotonic_nanos)
            .map(|(name, scheduled)| (scheduled.deadline, name.clone()))
            .collect();
        due.sort();
        let mut fired = Vec::with_capacity(due.len());
        for (_, name) in due {
            let Some(mut scheduled) = self.alarms.remove(&name) else {
                continue;
            };
            fired.push(scheduled.alarm.clone());
            if let Some(period) = scheduled.period {
                if let Some(deadline) = now.monotonic_nanos.checked_add(period) {
                    scheduled.deadline = deadline;
                    scheduled.alarm.scheduled_time =
                        now.unix_millis as f64 + period as f64 / NANOS_PER_MILLI;
                    scheduled.wake_unix_millis = scheduled.alarm.scheduled_time;
                    self.alarms.insert(name, scheduled);
                }
            }
        }
        fired
    }

    /// Snapshot to persist after a mutation and after taking due alarms.
    /// The adapter must bind this record to its profile and install identity.
    pub fn snapshot(&self) -> AlarmSnapshot {
        AlarmSnapshot {
            version: 1,
            alarms: self
                .alarms
                .values()
                .filter(|item| item.alarm.persist_across_sessions)
                .map(|item| PersistedAlarm {
                    alarm: item.alarm.clone(),
                    wake_unix_millis: item.wake_unix_millis,
                })
                .collect(),
        }
    }

    /// Takes a persistent snapshot using current wall time. Prefer this form
    /// after a system clock change: remaining monotonic delay is preserved.
    pub fn snapshot_at(&self, now: AlarmEnvironment) -> AlarmSnapshot {
        let mut snapshot = self.snapshot();
        for persisted in &mut snapshot.alarms {
            if let Some(scheduled) = self.alarms.get(&persisted.alarm.name) {
                persisted.wake_unix_millis = now.unix_millis as f64
                    + scheduled.deadline.saturating_sub(now.monotonic_nanos) as f64
                        / NANOS_PER_MILLI;
            }
        }
        snapshot
    }

    /// Restores browser-session state atomically. Overdue entries are due now;
    /// [`Self::take_due`] emits each once. Corrupt or newer state is refused.
    pub fn restore(
        &mut self,
        snapshot: AlarmSnapshot,
        now: AlarmEnvironment,
    ) -> Result<(), ApiError> {
        if snapshot.version != 1 || snapshot.alarms.len() > MAX_ALARMS {
            return Err(ApiError::other("Invalid alarm snapshot."));
        }
        let mut restored = BTreeMap::new();
        for persisted in snapshot.alarms {
            let mut alarm = persisted.alarm;
            validate_name(&alarm.name)?;
            if !alarm.persist_across_sessions
                || !valid_timestamp(alarm.scheduled_time)
                || !valid_timestamp(persisted.wake_unix_millis)
            {
                return Err(ApiError::other("Invalid alarm snapshot."));
            }
            let period = period_nanos(alarm.period_in_minutes, now.minimum())?;
            if !now.unpacked {
                alarm.period_in_minutes = alarm.period_in_minutes.map(|minutes| {
                    minutes.max(now.minimum() as f64 / NANOS_PER_MILLI / MILLIS_PER_MINUTE)
                });
            }
            let delay =
                millis_to_nanos((persisted.wake_unix_millis - now.unix_millis as f64).max(0.0))?;
            let deadline = now
                .monotonic_nanos
                .checked_add(delay)
                .ok_or_else(time_error)?;
            let name = alarm.name.clone();
            if restored
                .insert(
                    name,
                    Scheduled {
                        alarm,
                        deadline,
                        period,
                        wake_unix_millis: persisted.wake_unix_millis,
                    },
                )
                .is_some()
            {
                return Err(ApiError::other("Duplicate name in alarm snapshot."));
            }
        }
        self.alarms = restored;
        Ok(())
    }

    /// Decodes bounded durable bytes and restores atomically. Serde's default
    /// nesting limit remains enabled; malformed JSON never replaces live state.
    pub fn restore_bytes(&mut self, bytes: &[u8], now: AlarmEnvironment) -> Result<(), ApiError> {
        if bytes.len() > MAX_ALARM_SNAPSHOT_BYTES {
            return Err(ApiError::message_too_large());
        }
        let snapshot = serde_json::from_slice(bytes)
            .map_err(|_| ApiError::other("Invalid alarm snapshot."))?;
        self.restore(snapshot, now)
    }

    /// Runs a schema-validated `alarms.*` call. Null represents an absent
    /// alarm or void result on the JSON bridge; bindings own undefined mapping.
    /// Hosts persist mutations before acknowledging durable completion.
    pub fn handle(
        &mut self,
        method: &str,
        args: &[Value],
        now: AlarmEnvironment,
    ) -> Result<Value, ApiError> {
        self.handle_with_warnings(method, args, now)
            .map(|(value, _)| value)
    }

    /// Handles a call, returning warnings to route to its authenticated console.
    pub fn handle_with_warnings(
        &mut self,
        method: &str,
        args: &[Value],
        now: AlarmEnvironment,
    ) -> Result<(Value, Vec<String>), ApiError> {
        let args = crate::schema::normalize(method, args)?;
        let args = args.as_slice();
        match method {
            "alarms.create" => {
                let [name, value] = args else {
                    return Err(ApiError::malformed_arguments());
                };
                let info = serde_json::from_value(value.clone())
                    .map_err(|_| ApiError::malformed_arguments())?;
                let warnings = self.create_with_warnings(name.as_str(), info, now)?;
                return Ok((Value::Null, warnings));
            }
            "alarms.get" => serialize(self.get(optional_name(args))),
            "alarms.getAll" => serialize(self.get_all()),
            "alarms.clear" => Ok(Value::Bool(self.clear(optional_name(args)))),
            "alarms.clearAll" => Ok(Value::Bool(self.clear_all())),
            _ => Err(ApiError::unknown_method()),
        }
        .map(|value| (value, Vec::new()))
    }
}

fn prepare(
    name: Option<&str>,
    mut info: AlarmCreateInfo,
    now: AlarmEnvironment,
) -> Result<(String, Scheduled), ApiError> {
    if name.is_some() && info.name.is_some() {
        return Err(ApiError::other("Alarm name was specified twice."));
    }
    let name = name.or(info.name.as_deref()).unwrap_or_default();
    validate_name(name)?;
    if info.when.is_some() && info.delay_in_minutes.is_some() {
        return Err(ApiError::other("Cannot set both when and delayInMinutes."));
    }
    if info.when.is_none() && info.delay_in_minutes.is_none() && info.period_in_minutes.is_none() {
        return Err(ApiError::other(
            "An alarm requires a time, delay or period.",
        ));
    }
    if let Some(delay) = info.delay_in_minutes {
        if !delay.is_finite() || delay < 0.0 {
            return Err(time_error());
        }
    }
    let period = period_nanos(info.period_in_minutes, now.minimum())?;
    if !now.unpacked {
        info.period_in_minutes = info
            .period_in_minutes
            .map(|minutes| minutes.max(now.minimum() as f64 / NANOS_PER_MILLI / MILLIS_PER_MINUTE));
    }
    let scheduled_time = match info.when {
        Some(when) if valid_timestamp(when) => when,
        Some(_) => return Err(time_error()),
        None => {
            now.unix_millis as f64
                + info
                    .delay_in_minutes
                    .or(info.period_in_minutes)
                    .unwrap_or(0.0)
                    * MILLIS_PER_MINUTE
        }
    };
    if !valid_timestamp(scheduled_time) {
        return Err(time_error());
    }
    let delay =
        millis_to_nanos((scheduled_time - now.unix_millis as f64).max(0.0))?.max(now.minimum());
    let deadline = now
        .monotonic_nanos
        .checked_add(delay)
        .ok_or_else(time_error)?;
    Ok((
        name.to_owned(),
        Scheduled {
            alarm: Alarm {
                name: name.to_owned(),
                scheduled_time,
                period_in_minutes: info.period_in_minutes,
                persist_across_sessions: info.persist_across_sessions.unwrap_or(true),
            },
            deadline,
            period,
            wake_unix_millis: now.unix_millis as f64 + delay as f64 / NANOS_PER_MILLI,
        },
    ))
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    if name.len() > MAX_ALARM_NAME_BYTES {
        return Err(ApiError::other("Alarm name exceeds the 1024-byte limit."));
    }
    Ok(())
}

fn alarm_warnings(
    name: Option<&str>,
    info: &AlarmCreateInfo,
    now: AlarmEnvironment,
) -> Vec<String> {
    let name = name.or(info.name.as_deref()).unwrap_or_default();
    let seconds = now.minimum() / 1_000_000_000;
    let unit = if seconds == 1 { "second" } else { "seconds" };
    [
        ("delay", info.delay_in_minutes),
        ("period", info.period_in_minutes),
    ]
    .into_iter()
    .filter_map(|(field, minutes)| {
        minutes
            .filter(|minutes| *minutes * 60.0 < seconds as f64)
            .map(|_| format!(
                "Alarm {field} is less than the minimum duration of {seconds} {unit}. Alarm \"{name}\" will fire after the minimum duration.",
            ))
    })
    .collect()
}

fn valid_timestamp(value: f64) -> bool {
    value.is_finite() && value.abs() <= MAX_SAFE_INTEGER
}

fn millis_to_nanos(millis: f64) -> Result<u64, ApiError> {
    let nanos = (millis * NANOS_PER_MILLI).ceil();
    if !nanos.is_finite() || nanos < 0.0 || nanos >= u64::MAX as f64 {
        return Err(time_error());
    }
    Ok(nanos as u64)
}

fn period_nanos(period: Option<f64>, minimum: u64) -> Result<Option<u64>, ApiError> {
    period
        .map(|minutes| {
            if !minutes.is_finite() {
                return Err(time_error());
            }
            millis_to_nanos(minutes.max(0.0) * MILLIS_PER_MINUTE).map(|nanos| nanos.max(minimum))
        })
        .transpose()
}

fn time_error() -> ApiError {
    ApiError::other("Invalid or unrepresentable alarm time.")
}

fn serialize(value: impl Serialize) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|_| ApiError::other("Cannot encode alarm state."))
}

fn optional_name(args: &[Value]) -> &str {
    args.first().and_then(Value::as_str).unwrap_or_default()
}

/// Adapter seam: select the install-scoped service using authenticated sender
/// identity, supply time, and settle persistence before reporting success.
pub trait AlarmHost {
    /// Executes an admitted call against the sender's own alarm service.
    fn alarm_call(
        &mut self,
        sender: &Sender,
        method: &str,
        args: &[Value],
    ) -> Result<Value, ApiError>;
}

/// Registers the five alarm methods for extension pages with `alarms` access.
pub fn register<S: AlarmHost + 'static>(table: &mut ApiTable<S>) -> Result<(), RegistrationError> {
    for name in [
        "alarms.create",
        "alarms.get",
        "alarms.getAll",
        "alarms.clear",
        "alarms.clearAll",
    ] {
        table.register_generated(name, |state: &mut S, call| {
            let result = state.alarm_call(call.sender(), call.name(), call.args());
            call.reply(result)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::limits::ManualClock;

    fn now(clock: &ManualClock) -> AlarmEnvironment {
        AlarmEnvironment::now(clock, 1_000_000 + clock.now_nanos() / 1_000_000, false)
    }

    #[test]
    fn relative_absolute_repeating_and_unnamed_alarms() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        alarms
            .handle(
                "alarms.create",
                &[json!({"delayInMinutes": 1})],
                now(&clock),
            )
            .unwrap();
        alarms
            .handle(
                "alarms.create",
                &[json!("repeat"), json!({"periodInMinutes": 2})],
                now(&clock),
            )
            .unwrap();
        alarms
            .handle(
                "alarms.create",
                &[json!("absolute"), json!({"when": 1_030_000})],
                now(&clock),
            )
            .unwrap();
        assert_eq!(alarms.next_wake(), Some(30_000_000_000));
        clock.advance_nanos(30_000_000_000);
        assert_eq!(alarms.take_due(now(&clock))[0].name, "absolute");
        clock.advance_nanos(30_000_000_000);
        assert_eq!(alarms.take_due(now(&clock))[0].name, "");
        clock.advance_nanos(60_000_000_000);
        let fired = alarms.take_due(now(&clock));
        assert_eq!(fired[0].name, "repeat");
        assert_eq!(alarms.next_wake(), Some(240_000_000_000));
        assert_eq!(
            alarms.handle("alarms.get", &[], now(&clock)).unwrap(),
            Value::Null
        );
        assert_eq!(
            alarms
                .handle("alarms.clear", &[json!("repeat")], now(&clock))
                .unwrap(),
            json!(true)
        );
        assert_eq!(
            alarms
                .handle("alarms.clear", &[json!("repeat")], now(&clock))
                .unwrap(),
            json!(false)
        );
        assert_eq!(
            alarms.handle("alarms.clearAll", &[], now(&clock)).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn development_minimum_and_monotonic_timer_ignore_wall_clock_jumps() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        let mut environment = now(&clock);
        environment.unpacked = true;
        alarms
            .handle(
                "alarms.create",
                &[json!({"delayInMinutes": 0})],
                environment,
            )
            .unwrap();
        assert_eq!(alarms.next_wake(), Some(MIN_UNPACKED_DELAY_NANOS));
        environment.unix_millis += 1_000_000;
        assert!(alarms.take_due(environment).is_empty());
        environment.monotonic_nanos = MIN_UNPACKED_DELAY_NANOS;
        assert_eq!(alarms.take_due(environment).len(), 1);
    }

    #[test]
    fn unpacked_periods_are_preserved_and_only_subsecond_values_warn() {
        let clock = ManualClock::default();
        let environment = AlarmEnvironment {
            unpacked: true,
            ..now(&clock)
        };
        let mut alarms = Alarms::new();
        for (name, period, expected_warning) in [
            ("below-minimum", 0.1, false),
            ("at-minimum", 0.5, false),
            ("below-one-second", 0.001, true),
            ("negative-period", -1.0, true),
        ] {
            let (_, warnings) = alarms
                .handle_with_warnings(
                    "alarms.create",
                    &[
                        json!(name),
                        json!({"delayInMinutes": 2, "periodInMinutes": period}),
                    ],
                    environment,
                )
                .unwrap();
            assert_eq!(alarms.get(name).unwrap().period_in_minutes, Some(period));
            assert_eq!(
                warnings,
                if expected_warning {
                    vec![format!("Alarm period is less than the minimum duration of 1 second. Alarm \"{name}\" will fire after the minimum duration.")]
                } else {
                    vec![]
                },
            );
        }
        let mut restored = Alarms::new();
        restored.restore(alarms.snapshot(), environment).unwrap();
        assert_eq!(restored.get_all(), alarms.get_all());
        let due = AlarmEnvironment {
            monotonic_nanos: 120_000_000_000,
            unix_millis: environment.unix_millis + 120_000,
            ..environment
        };
        assert_eq!(restored.take_due(due).len(), 4);
        assert!(restored.take_due(due).is_empty());
        assert_eq!(
            restored.next_wake(),
            Some(due.monotonic_nanos + MIN_UNPACKED_DELAY_NANOS)
        );
    }

    #[test]
    fn packed_periods_clamp_and_both_timer_fields_report_diagnostics() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        for period in [-1.0, 0.0, 0.001, 0.1] {
            let (_, warnings) = alarms
                .handle_with_warnings(
                    "alarms.create",
                    &[
                        json!("packed"),
                        json!({"delayInMinutes": 0, "periodInMinutes": period}),
                    ],
                    now(&clock),
                )
                .unwrap();
            assert_eq!(alarms.get("packed").unwrap().period_in_minutes, Some(0.5));
            assert_eq!(alarms.next_wake(), Some(MIN_PACKED_DELAY_NANOS));
            assert_eq!(warnings, [
                "Alarm delay is less than the minimum duration of 30 seconds. Alarm \"packed\" will fire after the minimum duration.",
                "Alarm period is less than the minimum duration of 30 seconds. Alarm \"packed\" will fire after the minimum duration.",
            ]);
        }
    }

    #[test]
    fn nonfinite_alarm_fields_are_refused_without_replacing_live_state() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        alarms
            .create(
                None,
                AlarmCreateInfo {
                    delay_in_minutes: Some(1.0),
                    ..Default::default()
                },
                now(&clock),
            )
            .unwrap();
        let before = alarms.snapshot();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for unpacked in [false, true] {
                let environment = AlarmEnvironment {
                    unpacked,
                    ..now(&clock)
                };
                assert!(alarms
                    .create_with_warnings(
                        None,
                        AlarmCreateInfo {
                            period_in_minutes: Some(value),
                            ..Default::default()
                        },
                        environment
                    )
                    .is_err());
                assert!(alarms
                    .create_with_warnings(
                        None,
                        AlarmCreateInfo {
                            delay_in_minutes: Some(value),
                            ..Default::default()
                        },
                        environment
                    )
                    .is_err());
                assert_eq!(alarms.snapshot(), before);
            }
        }
    }

    #[test]
    fn zero_repeat_period_is_clamped_and_cannot_spin() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        alarms
            .handle(
                "alarms.create",
                &[json!({"periodInMinutes": 0})],
                now(&clock),
            )
            .unwrap();
        clock.advance_nanos(MIN_PACKED_DELAY_NANOS);
        assert_eq!(alarms.take_due(now(&clock)).len(), 1);
        assert!(alarms.take_due(now(&clock)).is_empty());
        assert_eq!(alarms.next_wake(), Some(2 * MIN_PACKED_DELAY_NANOS));
    }

    #[test]
    fn restart_coalesces_missed_intervals_and_excludes_session_only_alarms() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        alarms
            .handle(
                "alarms.create",
                &[json!("repeat"), json!({"periodInMinutes": 1})],
                now(&clock),
            )
            .unwrap();
        alarms
            .handle(
                "alarms.create",
                &[json!("once"), json!({"delayInMinutes": 1})],
                now(&clock),
            )
            .unwrap();
        alarms
            .handle(
                "alarms.create",
                &[
                    json!("session"),
                    json!({"delayInMinutes": 1, "persistAcrossSessions": false}),
                ],
                now(&clock),
            )
            .unwrap();
        let snapshot =
            serde_json::from_value(serde_json::to_value(alarms.snapshot()).unwrap()).unwrap();
        let environment = AlarmEnvironment {
            unix_millis: 2_000_000,
            ..now(&clock)
        };
        let mut restored = Alarms::new();
        restored.restore(snapshot, environment).unwrap();
        assert_eq!(restored.next_wake(), Some(0));
        assert_eq!(restored.take_due(environment).len(), 2);
        assert!(restored.take_due(environment).is_empty());
        assert_eq!(restored.get("repeat").unwrap().scheduled_time, 2_060_000.0);
        assert!(restored.get("session").is_none());
    }

    #[test]
    fn restart_preserves_effective_minimum_and_wall_clock_adjustments() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        alarms
            .handle(
                "alarms.create",
                &[json!({"delayInMinutes": 0})],
                now(&clock),
            )
            .unwrap();
        clock.advance_nanos(5_000_000_000);
        let restart = AlarmEnvironment {
            monotonic_nanos: 0,
            ..now(&clock)
        };
        let mut restored = Alarms::new();
        restored.restore(alarms.snapshot(), restart).unwrap();
        assert_eq!(restored.next_wake(), Some(25_000_000_000));
        let adjusted = AlarmEnvironment {
            unix_millis: 10_000,
            ..now(&clock)
        };
        restored
            .restore(
                alarms.snapshot_at(adjusted),
                AlarmEnvironment {
                    monotonic_nanos: 0,
                    ..adjusted
                },
            )
            .unwrap();
        assert_eq!(restored.next_wake(), Some(25_000_000_000));
    }

    #[test]
    fn malformed_requests_and_corrupt_snapshots_preserve_existing_state() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        alarms
            .handle(
                "alarms.create",
                &[json!({"delayInMinutes": 1})],
                now(&clock),
            )
            .unwrap();
        let before = alarms.snapshot();
        for args in [
            json!([]),
            json!([{}]),
            json!([{"when": 3, "delayInMinutes": 1}]),
            json!([{"delayInMinutes": -1}]),
            json!([{"when": "tomorrow"}]),
            json!([{"when": 1e100}]),
            json!([{"delayInMinutes": 1e100}]),
            json!([{"when": 1, "unknown": true}]),
            json!(["x", {"name": "x", "when": 3}]),
        ] {
            assert!(
                alarms
                    .handle("alarms.create", args.as_array().unwrap(), now(&clock))
                    .is_err(),
                "{args}"
            );
            assert_eq!(alarms.snapshot(), before);
        }
        let mut duplicate = before.clone();
        duplicate.alarms.push(duplicate.alarms[0].clone());
        assert!(alarms.restore(duplicate, now(&clock)).is_err());
        assert_eq!(alarms.snapshot(), before);
        let mut newer = before.clone();
        newer.version = 2;
        assert!(alarms.restore(newer, now(&clock)).is_err());
        assert_eq!(alarms.snapshot(), before);
        assert!(alarms.restore_bytes(b"{", now(&clock)).is_err());
        assert_eq!(alarms.snapshot(), before);
        assert!(alarms
            .restore_bytes(&vec![b' '; MAX_ALARM_SNAPSHOT_BYTES + 1], now(&clock))
            .is_err());
        assert_eq!(alarms.snapshot(), before);
    }

    #[test]
    fn limit_is_install_scoped_and_replacements_are_atomic() {
        let clock = ManualClock::default();
        let mut alarms = Alarms::new();
        for i in 0..MAX_ALARMS {
            alarms
                .handle(
                    "alarms.create",
                    &[json!(i.to_string()), json!({"delayInMinutes": 1})],
                    now(&clock),
                )
                .unwrap();
        }
        assert!(alarms
            .handle(
                "alarms.create",
                &[json!("overflow"), json!({"when": 5})],
                now(&clock)
            )
            .is_err());
        alarms
            .handle(
                "alarms.create",
                &[json!("0"), json!({"delayInMinutes": 2})],
                now(&clock),
            )
            .unwrap();
        assert_eq!(alarms.get_all().len(), MAX_ALARMS);
        assert_eq!(alarms.get("0").unwrap().scheduled_time, 1_120_000.0);
        assert!(alarms
            .handle(
                "alarms.create",
                &[
                    json!("x".repeat(MAX_ALARM_NAME_BYTES + 1)),
                    json!({"when": 1})
                ],
                now(&clock)
            )
            .is_err());
    }
}
