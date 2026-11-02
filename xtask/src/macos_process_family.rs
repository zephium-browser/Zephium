//! Release-evidence sampler for one macOS application process coalition.
//!
//! WebKit XPC helpers are re-parented to launchd, so parent-PID traversal does
//! not describe a browser's resource ownership. LaunchServices publishes the
//! exact application coalition used by Activity Monitor. This command binds to
//! one unambiguous bundle identifier, refreshes that coalition while sampling,
//! and reads monotonic per-process kernel counters. It is an xtask-only
//! measurement tool and grants no authority to product code.

#[cfg(any(target_os = "macos", test))]
use serde::Serialize;

const MAX_BUNDLE_ID_BYTES: usize = 255;
const MAX_LABEL_BYTES: usize = 64;
#[cfg(target_os = "macos")]
const MAX_APPLICATION_NAME_BYTES: usize = 512;
#[cfg(any(target_os = "macos", test))]
const MAX_COALITION_PROCESSES: usize = 128;
const MIN_DURATION_SECONDS: u64 = 1;
const MAX_DURATION_SECONDS: u64 = 24 * 60 * 60;
const DEFAULT_INTERVAL_MILLIS: u64 = 250;
const MIN_INTERVAL_MILLIS: u64 = 50;
const MAX_INTERVAL_MILLIS: u64 = 5_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct MeasurementArguments {
    bundle_id: String,
    duration: std::time::Duration,
    interval: std::time::Duration,
    label: String,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProcessRole {
    Application,
    WebContent,
    Networking,
    GraphicsAndMedia,
    Auxiliary,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct KernelCounters {
    user_cpu_abstime: u64,
    system_cpu_abstime: u64,
    package_idle_wakeups: u64,
    interrupt_wakeups: u64,
    pageins: u64,
    disk_bytes_read: u64,
    disk_bytes_written: u64,
    logical_writes: u64,
    instructions: u64,
    cycles: u64,
    billed_energy_raw: u64,
    serviced_energy_raw: u64,
}

#[cfg(any(target_os = "macos", test))]
impl KernelCounters {
    fn checked_delta(self, earlier: Self) -> Result<Self, String> {
        macro_rules! delta {
            ($field:ident) => {
                self.$field.checked_sub(earlier.$field).ok_or_else(|| {
                    concat!(stringify!($field), " counter moved backwards").to_owned()
                })?
            };
        }
        Ok(Self {
            user_cpu_abstime: delta!(user_cpu_abstime),
            system_cpu_abstime: delta!(system_cpu_abstime),
            package_idle_wakeups: delta!(package_idle_wakeups),
            interrupt_wakeups: delta!(interrupt_wakeups),
            pageins: delta!(pageins),
            disk_bytes_read: delta!(disk_bytes_read),
            disk_bytes_written: delta!(disk_bytes_written),
            logical_writes: delta!(logical_writes),
            instructions: delta!(instructions),
            cycles: delta!(cycles),
            billed_energy_raw: delta!(billed_energy_raw),
            serviced_energy_raw: delta!(serviced_energy_raw),
        })
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
struct UsageCounters {
    user_cpu_ns: u64,
    system_cpu_ns: u64,
    package_idle_wakeups: u64,
    interrupt_wakeups: u64,
    pageins: u64,
    disk_bytes_read: u64,
    disk_bytes_written: u64,
    logical_writes: u64,
    instructions: u64,
    cycles: u64,
    billed_energy_raw: u64,
    serviced_energy_raw: u64,
}

#[cfg(any(target_os = "macos", test))]
impl UsageCounters {
    fn checked_add_assign(&mut self, other: Self) -> Result<(), String> {
        macro_rules! add {
            ($field:ident) => {
                self.$field = self
                    .$field
                    .checked_add(other.$field)
                    .ok_or_else(|| concat!(stringify!($field), " counter overflowed").to_owned())?;
            };
        }
        add!(user_cpu_ns);
        add!(system_cpu_ns);
        add!(package_idle_wakeups);
        add!(interrupt_wakeups);
        add!(pageins);
        add!(disk_bytes_read);
        add!(disk_bytes_written);
        add!(logical_writes);
        add!(instructions);
        add!(cycles);
        add!(billed_energy_raw);
        add!(serviced_energy_raw);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[cfg(target_os = "macos")]
struct ProcessIdentity {
    pid: i32,
    uuid: [u8; 16],
    start_abstime: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_os = "macos")]
struct ProcessSample {
    identity: ProcessIdentity,
    resident_bytes: u64,
    physical_footprint_bytes: u64,
    lifetime_peak_physical_footprint_bytes: u64,
    counters: KernelCounters,
}

#[derive(Clone, Debug)]
#[cfg(target_os = "macos")]
struct TrackedProcess {
    role: ProcessRole,
    first: ProcessSample,
    last: ProcessSample,
}

#[derive(Clone, Copy, Debug, Default)]
#[cfg(target_os = "macos")]
struct MemorySample {
    process_count: usize,
    resident_bytes: u64,
    physical_footprint_bytes: u64,
}

#[cfg(target_os = "macos")]
impl MemorySample {
    fn checked_add(&mut self, process: ProcessSample) -> Result<(), String> {
        self.process_count = self
            .process_count
            .checked_add(1)
            .ok_or_else(|| "process count overflowed".to_owned())?;
        self.resident_bytes = self
            .resident_bytes
            .checked_add(process.resident_bytes)
            .ok_or_else(|| "family resident-byte total overflowed".to_owned())?;
        self.physical_footprint_bytes = self
            .physical_footprint_bytes
            .checked_add(process.physical_footprint_bytes)
            .ok_or_else(|| "family physical-footprint total overflowed".to_owned())?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
#[cfg(target_os = "macos")]
struct RoleAccumulator {
    observed_processes: usize,
    peak_process_count: usize,
    peak_resident_bytes: u64,
    peak_physical_footprint_bytes: u64,
    maximum_lifetime_process_physical_footprint_bytes: u64,
    counters: UsageCounters,
}

#[derive(Clone, Debug, Serialize)]
#[cfg(target_os = "macos")]
struct RoleReport {
    role: ProcessRole,
    observed_processes: usize,
    peak_process_count: usize,
    peak_resident_bytes: u64,
    peak_physical_footprint_bytes: u64,
    maximum_lifetime_process_physical_footprint_bytes: u64,
    counters: UsageCounters,
}

#[derive(Clone, Debug, Serialize)]
#[cfg(target_os = "macos")]
struct MeasurementReport {
    schema_version: u8,
    label: String,
    bundle_id: String,
    application_name: String,
    root_pid: i32,
    requested_duration_ms: u64,
    observed_duration_ms: u64,
    sample_interval_ms: u64,
    sample_count: usize,
    observed_processes: usize,
    peak_process_count: usize,
    terminal_process_count: usize,
    peak_resident_bytes: u64,
    terminal_resident_bytes: u64,
    peak_physical_footprint_bytes: u64,
    terminal_physical_footprint_bytes: u64,
    maximum_lifetime_process_physical_footprint_bytes: u64,
    counters: UsageCounters,
    roles: Vec<RoleReport>,
}

pub(crate) fn run(arguments: &[String]) -> Result<(), String> {
    let arguments = parse_arguments(arguments)?;
    platform::run(arguments)
}

fn parse_arguments(arguments: &[String]) -> Result<MeasurementArguments, String> {
    let mut bundle_id = None;
    let mut duration = None;
    let mut interval = std::time::Duration::from_millis(DEFAULT_INTERVAL_MILLIS);
    let mut interval_supplied = false;
    let mut label = "unlabelled".to_owned();
    let mut label_supplied = false;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        index += 1;
        let value = arguments
            .get(index)
            .ok_or_else(|| format!("{option} requires a value"))?;
        index += 1;
        match option {
            "--bundle-id" => {
                if bundle_id.replace(value.clone()).is_some() {
                    return Err("--bundle-id may be supplied only once".into());
                }
            }
            "--duration-seconds" => {
                let seconds = parse_bounded_number(
                    "duration seconds",
                    value,
                    MIN_DURATION_SECONDS,
                    MAX_DURATION_SECONDS,
                )?;
                if duration
                    .replace(std::time::Duration::from_secs(seconds))
                    .is_some()
                {
                    return Err("--duration-seconds may be supplied only once".into());
                }
            }
            "--interval-millis" => {
                if std::mem::replace(&mut interval_supplied, true) {
                    return Err("--interval-millis may be supplied only once".into());
                }
                interval = std::time::Duration::from_millis(parse_bounded_number(
                    "interval milliseconds",
                    value,
                    MIN_INTERVAL_MILLIS,
                    MAX_INTERVAL_MILLIS,
                )?);
            }
            "--label" => {
                if std::mem::replace(&mut label_supplied, true) {
                    return Err("--label may be supplied only once".into());
                }
                validate_label(value)?;
                label.clone_from(value);
            }
            _ => {
                return Err(format!(
                    "unknown process-family measurement option {option:?}"
                ))
            }
        }
    }

    let bundle_id = bundle_id.ok_or_else(|| "--bundle-id is required".to_owned())?;
    validate_bundle_id(&bundle_id)?;
    let duration = duration.ok_or_else(|| "--duration-seconds is required".to_owned())?;
    if interval > duration {
        return Err("the sample interval may not exceed the campaign duration".into());
    }
    Ok(MeasurementArguments {
        bundle_id,
        duration,
        interval,
        label,
    })
}

fn parse_bounded_number(
    label: &str,
    value: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{label} must be an unsigned decimal integer"));
    }
    let value = value
        .parse::<u64>()
        .map_err(|_| format!("{label} is out of range"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{label} must be within {minimum}..={maximum}"));
    }
    Ok(value)
}

fn validate_bundle_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_BUNDLE_ID_BYTES
        || !value.is_ascii()
        || value.starts_with('.')
        || value.ends_with('.')
        || !value.contains('.')
        || value
            .split('.')
            .any(|part| part.is_empty() || !part.bytes().all(is_bundle_id_byte))
    {
        return Err("bundle identifier is not a bounded canonical ASCII identifier".into());
    }
    Ok(())
}

const fn is_bundle_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

fn validate_label(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_LABEL_BYTES
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("measurement label must be 1..=64 ASCII letters, digits, '-' or '_'".into());
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn parse_single_asn(output: &str) -> Result<&str, String> {
    let mut asns = output.split_ascii_whitespace().filter(|word| {
        word.starts_with("ASN:")
            && word.ends_with(':')
            && word.bytes().all(|byte| {
                byte.is_ascii_hexdigit() || matches!(byte, b'A' | b'S' | b'N' | b':' | b'x' | b'-')
            })
    });
    let asn = asns
        .next()
        .ok_or_else(|| "no running application matched the bundle identifier".to_owned())?;
    if asns.next().is_some() {
        return Err("multiple running applications matched the bundle identifier".into());
    }
    Ok(asn)
}

/// macOS 27 prints `pid = 123` and `bundleID="..."` where earlier systems
/// printed `"pid"=123` and `"CFBundleIdentifier"="..."`.
#[cfg(any(target_os = "macos", test))]
const CURRENT_KEYS: [(&str, &str); 2] = [("pid", "pid = "), ("CFBundleIdentifier", "bundleID=")];

#[cfg(any(target_os = "macos", test))]
fn find_value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    let legacy = format!("\"{key}\"=");
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix(&legacy))
        .or_else(|| {
            let (_, current) = CURRENT_KEYS.iter().find(|(name, _)| *name == key)?;
            output.lines().find_map(|line| {
                let value = line.trim().strip_prefix(current)?;
                Some(value.split_ascii_whitespace().next().unwrap_or(value))
            })
        })
}

#[cfg(any(target_os = "macos", test))]
fn parse_integer_value(output: &str, key: &str) -> Result<i32, String> {
    let value = find_value(output, key).ok_or_else(|| format!("LaunchServices omitted {key}"))?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("LaunchServices returned an invalid {key}"));
    }
    value
        .parse::<i32>()
        .map_err(|_| format!("LaunchServices {key} is out of range"))
}

#[cfg(any(target_os = "macos", test))]
fn parse_string_value(output: &str, key: &str, maximum: usize) -> Result<String, String> {
    let value = find_value(output, key)
        .and_then(|value| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            (key == "LSDisplayName")
                .then(|| display_name(output))
                .flatten()
        })
        .ok_or_else(|| format!("LaunchServices omitted {key}"))?;
    if value.is_empty() || value.len() > maximum || value.contains(['\r', '\n', '\0']) {
        return Err(format!("LaunchServices returned an invalid {key}"));
    }
    Ok(value.to_owned())
}

/// macOS 27 leads the record with the quoted display name.
#[cfg(any(target_os = "macos", test))]
fn display_name(output: &str) -> Option<&str> {
    let line = output.lines().next()?.trim_start();
    let rest = line.strip_prefix('"')?;
    let end = rest.find("\" ASN:")?;
    Some(&rest[..end])
}

#[cfg(any(target_os = "macos", test))]
fn parse_coalition_pids(output: &str) -> Result<Vec<i32>, String> {
    let prefix = "\"LSApplicationCoalitionPIDsKey\"=(";
    let cohort = output
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| "LaunchServices omitted the application coalition".to_owned())?;
    let mut pids = Vec::new();
    for value in cohort.split(',').map(str::trim) {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("LaunchServices returned an invalid coalition PID".into());
        }
        let pid = value
            .parse::<i32>()
            .map_err(|_| "LaunchServices coalition PID is out of range".to_owned())?;
        if pid <= 1 || pids.contains(&pid) {
            return Err("LaunchServices returned an invalid or duplicate coalition PID".into());
        }
        pids.push(pid);
        if pids.len() > MAX_COALITION_PROCESSES {
            return Err("application coalition exceeds the measurement process bound".into());
        }
    }
    if pids.is_empty() {
        return Err("LaunchServices returned an empty application coalition".into());
    }
    pids.sort_unstable();
    Ok(pids)
}

/// A process's resource coalition. Only a process that has exited has none;
/// any other failure could hide one of the app's own helpers, so it fails the
/// measurement.
#[cfg(any(target_os = "macos", test))]
fn coalition_lookup(
    pid: i32,
    coalition: u64,
    error: Option<std::io::Error>,
) -> Result<Option<u64>, String> {
    match error {
        None => Ok(Some(coalition)),
        Some(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(None),
        Some(error) => Err(format!("cannot read the coalition of PID {pid}: {error}")),
    }
}

/// The processes of `listed[..count]` in the `root` resource coalition. A
/// full buffer may have cut the list short, so it is refused rather than
/// measured as a smaller family.
#[cfg(any(target_os = "macos", test))]
fn coalition_members(
    listed: &[i32],
    count: usize,
    root: u64,
    mut coalition_of: impl FnMut(i32) -> Result<Option<u64>, String>,
) -> Result<Vec<i32>, String> {
    if count >= listed.len() {
        return Err("the process list filled its buffer and may be incomplete".into());
    }
    let mut pids = Vec::new();
    for &pid in &listed[..count] {
        if pid > 1 && coalition_of(pid)? == Some(root) {
            pids.push(pid);
            if pids.len() > MAX_COALITION_PROCESSES {
                return Err("application coalition exceeds the measurement process bound".into());
            }
        }
    }
    pids.sort_unstable();
    Ok(pids)
}

#[cfg(any(target_os = "macos", test))]
fn role_for_process(root: i32, pid: i32, name: &str) -> ProcessRole {
    if pid == root {
        return ProcessRole::Application;
    }
    match name {
        "com.apple.WebKit.WebContent" => ProcessRole::WebContent,
        "com.apple.WebKit.Networking" => ProcessRole::Networking,
        "com.apple.WebKit.GPU" => ProcessRole::GraphicsAndMedia,
        _ => ProcessRole::Auxiliary,
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{
        coalition_lookup, coalition_members, parse_coalition_pids, parse_integer_value,
        parse_single_asn, parse_string_value, role_for_process, KernelCounters,
        MeasurementArguments, MeasurementReport, MemorySample, ProcessIdentity, ProcessRole,
        ProcessSample, RoleAccumulator, RoleReport, TrackedProcess, UsageCounters,
        MAX_APPLICATION_NAME_BYTES,
    };
    use std::collections::{BTreeMap, HashMap};
    use std::ffi::c_void;
    use std::process::Command;
    use std::time::{Duration, Instant};

    pub(super) fn run(arguments: MeasurementArguments) -> Result<(), String> {
        let asn = find_application(&arguments.bundle_id)?;
        let identity = read_application_identity(&asn, &arguments.bundle_id)?;
        let timebase = mach_timebase()?;
        let started = Instant::now();
        let deadline = started
            .checked_add(arguments.duration)
            .ok_or_else(|| "measurement deadline overflowed".to_owned())?;
        let mut tracked = HashMap::<ProcessIdentity, TrackedProcess>::new();
        let mut peak = MemorySample::default();
        let mut maximum_lifetime_process_physical_footprint_bytes = 0;
        let mut role_peaks = BTreeMap::<ProcessRole, MemorySample>::new();
        let mut sample_count = 0usize;

        let terminal = loop {
            let pids = read_coalition(&asn, identity.root_pid, &arguments.bundle_id)?;
            let mut current = MemorySample::default();
            let mut current_roles = BTreeMap::<ProcessRole, MemorySample>::new();
            let mut root_observed = false;
            for pid in pids {
                let Some(sample) = sample_process(pid)? else {
                    continue;
                };
                root_observed |= pid == identity.root_pid;
                let name = process_name(pid)?;
                let role = role_for_process(identity.root_pid, pid, &name);
                current.checked_add(sample)?;
                current_roles.entry(role).or_default().checked_add(sample)?;
                maximum_lifetime_process_physical_footprint_bytes =
                    maximum_lifetime_process_physical_footprint_bytes
                        .max(sample.lifetime_peak_physical_footprint_bytes);
                tracked
                    .entry(sample.identity)
                    .and_modify(|process| {
                        process.last = sample;
                    })
                    .or_insert(TrackedProcess {
                        role,
                        first: sample,
                        last: sample,
                    });
            }
            if current.process_count == 0 {
                return Err("no readable process remained in the application coalition".into());
            }
            if !root_observed {
                return Err("the application process was not readable".into());
            }
            peak.process_count = peak.process_count.max(current.process_count);
            peak.resident_bytes = peak.resident_bytes.max(current.resident_bytes);
            peak.physical_footprint_bytes = peak
                .physical_footprint_bytes
                .max(current.physical_footprint_bytes);
            for (role, current) in current_roles {
                let peak = role_peaks.entry(role).or_default();
                peak.process_count = peak.process_count.max(current.process_count);
                peak.resident_bytes = peak.resident_bytes.max(current.resident_bytes);
                peak.physical_footprint_bytes = peak
                    .physical_footprint_bytes
                    .max(current.physical_footprint_bytes);
            }
            sample_count = sample_count
                .checked_add(1)
                .ok_or_else(|| "sample count overflowed".to_owned())?;

            let now = Instant::now();
            if now >= deadline {
                break current;
            }
            std::thread::sleep(
                arguments
                    .interval
                    .min(deadline.saturating_duration_since(now)),
            );
        };

        let mut counters = UsageCounters::default();
        let mut roles = BTreeMap::<ProcessRole, RoleAccumulator>::new();
        for process in tracked.values() {
            let kernel_delta = process
                .last
                .counters
                .checked_delta(process.first.counters)?;
            let delta = report_counters(kernel_delta, timebase)?;
            counters.checked_add_assign(delta)?;
            let role = roles.entry(process.role).or_default();
            role.observed_processes = role
                .observed_processes
                .checked_add(1)
                .ok_or_else(|| "role process count overflowed".to_owned())?;
            role.maximum_lifetime_process_physical_footprint_bytes = role
                .maximum_lifetime_process_physical_footprint_bytes
                .max(process.last.lifetime_peak_physical_footprint_bytes);
            role.counters.checked_add_assign(delta)?;
        }
        for (role, peak) in role_peaks {
            let accumulator = roles.entry(role).or_default();
            accumulator.peak_process_count = peak.process_count;
            accumulator.peak_resident_bytes = peak.resident_bytes;
            accumulator.peak_physical_footprint_bytes = peak.physical_footprint_bytes;
        }
        let roles = roles
            .into_iter()
            .map(|(role, values)| RoleReport {
                role,
                observed_processes: values.observed_processes,
                peak_process_count: values.peak_process_count,
                peak_resident_bytes: values.peak_resident_bytes,
                peak_physical_footprint_bytes: values.peak_physical_footprint_bytes,
                maximum_lifetime_process_physical_footprint_bytes: values
                    .maximum_lifetime_process_physical_footprint_bytes,
                counters: values.counters,
            })
            .collect();
        let observed_duration_ms = duration_millis(started.elapsed())?;
        let report = MeasurementReport {
            schema_version: 1,
            label: arguments.label,
            bundle_id: arguments.bundle_id,
            application_name: identity.application_name,
            root_pid: identity.root_pid,
            requested_duration_ms: duration_millis(arguments.duration)?,
            observed_duration_ms,
            sample_interval_ms: duration_millis(arguments.interval)?,
            sample_count,
            observed_processes: tracked.len(),
            peak_process_count: peak.process_count,
            terminal_process_count: terminal.process_count,
            peak_resident_bytes: peak.resident_bytes,
            terminal_resident_bytes: terminal.resident_bytes,
            peak_physical_footprint_bytes: peak.physical_footprint_bytes,
            terminal_physical_footprint_bytes: terminal.physical_footprint_bytes,
            maximum_lifetime_process_physical_footprint_bytes,
            counters,
            roles,
        };
        eprintln!(
            "macOS process-family measurement passed: label={} app={} samples={} processes={} peak_footprint_bytes={} terminal_footprint_bytes={} user_cpu_ms={} system_cpu_ms={} idle_wakeups={} interrupt_wakeups={}",
            report.label,
            report.application_name,
            report.sample_count,
            report.observed_processes,
            report.peak_physical_footprint_bytes,
            report.terminal_physical_footprint_bytes,
            report.counters.user_cpu_ns / 1_000_000,
            report.counters.system_cpu_ns / 1_000_000,
            report.counters.package_idle_wakeups,
            report.counters.interrupt_wakeups,
        );
        println!(
            "{}",
            serde_json::to_string(&report)
                .map_err(|error| format!("cannot serialize measurement report: {error}"))?
        );
        Ok(())
    }

    struct ApplicationIdentity {
        root_pid: i32,
        application_name: String,
    }

    fn find_application(bundle_id: &str) -> Result<String, String> {
        let output = command_output(
            "/usr/bin/lsappinfo",
            &["-nonames", "find", &format!("bundleid={bundle_id}")],
        )?;
        Ok(parse_single_asn(&output)?.to_owned())
    }

    fn read_application_identity(
        asn: &str,
        bundle_id: &str,
    ) -> Result<ApplicationIdentity, String> {
        let output = application_info(asn)?;
        let observed_bundle_id = parse_string_value(&output, "CFBundleIdentifier", 255)?;
        if observed_bundle_id != bundle_id {
            return Err("LaunchServices application identity changed during admission".into());
        }
        Ok(ApplicationIdentity {
            root_pid: parse_integer_value(&output, "pid")?,
            application_name: parse_string_value(
                &output,
                "LSDisplayName",
                MAX_APPLICATION_NAME_BYTES,
            )?,
        })
    }

    fn read_coalition(asn: &str, root_pid: i32, bundle_id: &str) -> Result<Vec<i32>, String> {
        let output = application_info(asn)?;
        if parse_integer_value(&output, "pid")? != root_pid
            || parse_string_value(&output, "CFBundleIdentifier", 255)? != bundle_id
        {
            return Err("LaunchServices application identity changed during measurement".into());
        }
        let pids = if output.contains("LSApplicationCoalitionPIDsKey") {
            parse_coalition_pids(&output)?
        } else {
            kernel_coalition(root_pid)?
        };
        if !pids.contains(&root_pid) {
            return Err("the application coalition omitted its admitted root process".into());
        }
        Ok(pids)
    }

    /// macOS 27's LaunchServices no longer lists the coalition; the kernel
    /// still reports the resource coalition each process belongs to.
    fn kernel_coalition(root_pid: i32) -> Result<Vec<i32>, String> {
        let root = resource_coalition(root_pid)?
            .ok_or_else(|| "the application process was not readable".to_owned())?;
        let mut all = vec![0i32; 16_384];
        let capacity = i32::try_from(all.len() * std::mem::size_of::<i32>())
            .map_err(|_| "PID buffer exceeds its bound".to_owned())?;
        // SAFETY: proc_listallpids fills at most `capacity` bytes of the
        // writable buffer and returns how many PIDs it wrote.
        let count = unsafe { libc::proc_listallpids(all.as_mut_ptr().cast::<c_void>(), capacity) };
        let count = usize::try_from(count).map_err(|_| "cannot list processes".to_owned())?;
        coalition_members(&all, count, root, resource_coalition)
    }

    #[repr(C)]
    #[derive(Default)]
    struct CoalitionInfo {
        coalition_id: [u64; 2],
        reserved: [u64; 3],
    }

    const PROC_PIDCOALITIONINFO: i32 = 20;

    fn resource_coalition(pid: i32) -> Result<Option<u64>, String> {
        let mut info = CoalitionInfo::default();
        let size = i32::try_from(std::mem::size_of::<CoalitionInfo>())
            .map_err(|_| "coalition record size overflowed".to_owned())?;
        // SAFETY: proc_pidinfo writes at most `size` bytes of the exact
        // proc_pidcoalitioninfo layout into the owned record.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDCOALITIONINFO,
                0,
                std::ptr::addr_of_mut!(info).cast::<c_void>(),
                size,
            )
        };
        let error = (written != size).then(std::io::Error::last_os_error);
        coalition_lookup(pid, info.coalition_id[0], error)
    }

    fn application_info(asn: &str) -> Result<String, String> {
        command_output(
            "/usr/bin/lsappinfo",
            &[
                "-nonames",
                "info",
                asn,
                "pid",
                "coalitionPIDs",
                "bundleid",
                "displayname",
            ],
        )
    }

    fn command_output(program: &str, arguments: &[&str]) -> Result<String, String> {
        let output = Command::new(program)
            .args(arguments)
            .output()
            .map_err(|error| format!("cannot execute {program}: {error}"))?;
        if !output.status.success() {
            return Err(format!("{program} exited unsuccessfully"));
        }
        if output.stdout.len() > 64 * 1024 {
            return Err(format!("{program} output exceeded its bound"));
        }
        String::from_utf8(output.stdout).map_err(|_| format!("{program} returned non-UTF-8 output"))
    }

    fn sample_process(pid: i32) -> Result<Option<ProcessSample>, String> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
        // SAFETY: proc_pid_rusage receives a writable buffer of the exact V4
        // layout selected by RUSAGE_INFO_V4. The buffer is read only after a
        // successful return. PIDs come from the bounded LaunchServices cohort.
        let result = unsafe {
            libc::proc_pid_rusage(
                pid,
                libc::RUSAGE_INFO_V4,
                usage.as_mut_ptr().cast::<libc::rusage_info_t>(),
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ESRCH)) {
                return Ok(None);
            }
            return Err(format!(
                "cannot read resource counters for coalition PID {pid}: {error}"
            ));
        }
        // SAFETY: the successful proc_pid_rusage call initialized the complete
        // rusage_info_v4 value above.
        let usage = unsafe { usage.assume_init() };
        Ok(Some(ProcessSample {
            identity: ProcessIdentity {
                pid,
                uuid: usage.ri_uuid,
                start_abstime: usage.ri_proc_start_abstime,
            },
            resident_bytes: usage.ri_resident_size,
            physical_footprint_bytes: usage.ri_phys_footprint,
            lifetime_peak_physical_footprint_bytes: usage.ri_lifetime_max_phys_footprint,
            counters: KernelCounters {
                user_cpu_abstime: usage.ri_user_time,
                system_cpu_abstime: usage.ri_system_time,
                package_idle_wakeups: usage.ri_pkg_idle_wkups,
                interrupt_wakeups: usage.ri_interrupt_wkups,
                pageins: usage.ri_pageins,
                disk_bytes_read: usage.ri_diskio_bytesread,
                disk_bytes_written: usage.ri_diskio_byteswritten,
                logical_writes: usage.ri_logical_writes,
                instructions: usage.ri_instructions,
                cycles: usage.ri_cycles,
                billed_energy_raw: usage.ri_billed_energy,
                serviced_energy_raw: usage.ri_serviced_energy,
            },
        }))
    }

    #[derive(Clone, Copy)]
    struct MachTimebase {
        numer: u32,
        denom: u32,
    }

    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    fn mach_timebase() -> Result<MachTimebase, String> {
        let mut info = std::mem::MaybeUninit::<MachTimebaseInfo>::zeroed();
        // SAFETY: mach_timebase_info receives a writable buffer of its exact
        // ABI type, which is read only after KERN_SUCCESS.
        if unsafe { mach_timebase_info(info.as_mut_ptr()) } != 0 {
            return Err("cannot read the macOS Mach timebase".into());
        }
        // SAFETY: the successful call above initialized the complete value.
        let info = unsafe { info.assume_init() };
        if info.numer == 0 || info.denom == 0 {
            return Err("macOS returned an invalid Mach timebase".into());
        }
        Ok(MachTimebase {
            numer: info.numer,
            denom: info.denom,
        })
    }

    fn report_counters(
        raw: KernelCounters,
        timebase: MachTimebase,
    ) -> Result<UsageCounters, String> {
        Ok(UsageCounters {
            user_cpu_ns: abstime_to_nanoseconds(raw.user_cpu_abstime, timebase)?,
            system_cpu_ns: abstime_to_nanoseconds(raw.system_cpu_abstime, timebase)?,
            package_idle_wakeups: raw.package_idle_wakeups,
            interrupt_wakeups: raw.interrupt_wakeups,
            pageins: raw.pageins,
            disk_bytes_read: raw.disk_bytes_read,
            disk_bytes_written: raw.disk_bytes_written,
            logical_writes: raw.logical_writes,
            instructions: raw.instructions,
            cycles: raw.cycles,
            billed_energy_raw: raw.billed_energy_raw,
            serviced_energy_raw: raw.serviced_energy_raw,
        })
    }

    fn abstime_to_nanoseconds(value: u64, timebase: MachTimebase) -> Result<u64, String> {
        let nanoseconds = u128::from(value)
            .checked_mul(u128::from(timebase.numer))
            .ok_or_else(|| "Mach absolute-time conversion overflowed".to_owned())?
            / u128::from(timebase.denom);
        u64::try_from(nanoseconds)
            .map_err(|_| "Mach absolute time exceeded u64 nanoseconds".to_owned())
    }

    fn process_name(pid: i32) -> Result<String, String> {
        let mut name = [0u8; 256];
        // SAFETY: proc_name receives a writable buffer with its exact byte
        // capacity. The returned count is checked before slicing.
        let length = unsafe {
            libc::proc_name(
                pid,
                name.as_mut_ptr().cast::<c_void>(),
                u32::try_from(name.len()).expect("process-name buffer length fits u32"),
            )
        };
        if length <= 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ESRCH)) {
                return Ok("exited".to_owned());
            }
            return Err(format!("cannot read coalition PID {pid} name: {error}"));
        }
        let length = usize::try_from(length)
            .map_err(|_| "process-name length was negative".to_owned())?
            .min(name.len());
        let end = name[..length]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(length);
        String::from_utf8(name[..end].to_vec()).map_err(|_| "process name was not UTF-8".to_owned())
    }

    fn duration_millis(duration: Duration) -> Result<u64, String> {
        u64::try_from(duration.as_millis()).map_err(|_| "duration exceeded u64 milliseconds".into())
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::MeasurementArguments;

    pub(super) fn run(_arguments: MeasurementArguments) -> Result<(), String> {
        Err("macOS process-family measurement is available only on macOS".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        coalition_lookup, coalition_members, parse_arguments, parse_coalition_pids,
        parse_integer_value, parse_single_asn, parse_string_value, role_for_process,
        KernelCounters, ProcessRole, UsageCounters, MAX_COALITION_PROCESSES,
    };

    #[test]
    fn arguments_are_exact_and_bounded() {
        let arguments = parse_arguments(&[
            "--bundle-id".into(),
            "app.zephium.extensions-staging".into(),
            "--duration-seconds".into(),
            "10".into(),
            "--interval-millis".into(),
            "125".into(),
            "--label".into(),
            "dark_reader_idle".into(),
        ])
        .unwrap();
        assert_eq!(arguments.duration.as_secs(), 10);
        assert_eq!(arguments.interval.as_millis(), 125);
        assert_eq!(arguments.label, "dark_reader_idle");

        for invalid in [
            vec!["--bundle-id".into(), "not-a-bundle".into()],
            vec![
                "--bundle-id".into(),
                "app.zephium.test".into(),
                "--duration-seconds".into(),
                "0".into(),
            ],
            vec![
                "--bundle-id".into(),
                "app.zephium.test".into(),
                "--duration-seconds".into(),
                "1".into(),
                "--label".into(),
                "bad label".into(),
            ],
        ] {
            assert!(parse_arguments(&invalid).is_err());
        }
    }

    #[test]
    fn launchservices_parsers_reject_ambiguity_and_malformed_cohorts() {
        assert_eq!(
            parse_single_asn("ASN:0x0-0x19c19c:\n").unwrap(),
            "ASN:0x0-0x19c19c:"
        );
        assert!(parse_single_asn("ASN:0x0-0x1:\nASN:0x0-0x2:\n").is_err());
        assert!(parse_single_asn("not-an-asn").is_err());

        let info = "\"pid\"=45010\n\
            \"LSApplicationCoalitionPIDsKey\"=( 45012, 45010, 45011)\n\
            \"CFBundleIdentifier\"=\"app.zephium.test\"\n\
            \"LSDisplayName\"=\"Zephium Test\"\n";
        assert_eq!(parse_integer_value(info, "pid").unwrap(), 45010);
        assert_eq!(
            parse_string_value(info, "CFBundleIdentifier", 255).unwrap(),
            "app.zephium.test"
        );
        assert_eq!(
            parse_coalition_pids(info).unwrap(),
            vec![45010, 45011, 45012]
        );
        assert!(parse_coalition_pids("\"LSApplicationCoalitionPIDsKey\"=( 2, 2)\n").is_err());
    }

    #[test]
    fn only_an_exited_process_has_no_coalition() {
        assert_eq!(coalition_lookup(7, 42, None), Ok(Some(42)));
        let exited = std::io::Error::from_raw_os_error(libc::ESRCH);
        assert_eq!(coalition_lookup(7, 0, Some(exited)), Ok(None));
        let denied = std::io::Error::from_raw_os_error(libc::EPERM);
        assert!(coalition_lookup(7, 0, Some(denied)).is_err());
    }

    #[test]
    fn coalition_members_refuse_a_full_list_and_an_oversized_family() {
        let coalition = |pid: i32| Ok(Some(if pid % 2 == 0 { 7 } else { 9 }));
        assert_eq!(
            coalition_members(&[1, 4, 3, 2, 0, 0], 4, 7, coalition).unwrap(),
            vec![2, 4]
        );
        assert!(coalition_members(&[2, 4, 6], 3, 7, coalition).is_err());
        assert!(coalition_members(&[2, 4, 6], 4, 7, coalition).is_err());
        let family: Vec<i32> = (2..=2 * (MAX_COALITION_PROCESSES as i32 + 2))
            .step_by(2)
            .collect();
        let mut buffer = family.clone();
        buffer.push(0);
        assert!(coalition_members(&buffer, family.len(), 7, coalition).is_err());
    }

    #[test]
    fn launchservices_parsers_read_the_macos_27_record() {
        let info = "\"Zephium Extensions QA\" ASN:0x0-0x1183182: (in front) \n\
            \x20   bundleID=\"app.zephium.webext-qa\"\n\
            \x20   bundle path=[ NULL ] \n\
            \x20   pid = 79754 !cgsConnection !signalled type=[ NULL ]  Arch=!!none \n";
        assert_eq!(
            parse_string_value(info, "CFBundleIdentifier", 255).unwrap(),
            "app.zephium.webext-qa"
        );
        assert_eq!(parse_integer_value(info, "pid").unwrap(), 79754);
        assert_eq!(
            parse_string_value(info, "LSDisplayName", 512).unwrap(),
            "Zephium Extensions QA"
        );
        assert!(parse_integer_value("    pid = 79x ", "pid").is_err());
    }

    #[test]
    fn roles_are_browser_specific_without_trusting_display_names() {
        assert_eq!(
            role_for_process(10, 10, "renamed"),
            ProcessRole::Application
        );
        assert_eq!(
            role_for_process(10, 11, "com.apple.WebKit.WebContent"),
            ProcessRole::WebContent
        );
        assert_eq!(
            role_for_process(10, 12, "com.apple.WebKit.Networking"),
            ProcessRole::Networking
        );
        assert_eq!(
            role_for_process(10, 13, "com.apple.WebKit.GPU"),
            ProcessRole::GraphicsAndMedia
        );
        assert_eq!(role_for_process(10, 14, "AutoFill"), ProcessRole::Auxiliary);
    }

    #[test]
    fn usage_deltas_are_monotonic_and_checked() {
        let earlier = KernelCounters {
            user_cpu_abstime: 10,
            package_idle_wakeups: 2,
            ..KernelCounters::default()
        };
        let later = KernelCounters {
            user_cpu_abstime: 15,
            package_idle_wakeups: 9,
            ..KernelCounters::default()
        };
        let delta = later.checked_delta(earlier).unwrap();
        assert_eq!(delta.user_cpu_abstime, 5);
        assert_eq!(delta.package_idle_wakeups, 7);
        assert!(earlier.checked_delta(later).is_err());

        let mut total = UsageCounters::default();
        let report_delta = UsageCounters {
            user_cpu_ns: 5,
            package_idle_wakeups: 7,
            ..UsageCounters::default()
        };
        total.checked_add_assign(report_delta).unwrap();
        assert_eq!(total, report_delta);
        total.user_cpu_ns = u64::MAX;
        assert!(total
            .checked_add_assign(UsageCounters {
                user_cpu_ns: 1,
                ..UsageCounters::default()
            })
            .is_err());
    }
}
