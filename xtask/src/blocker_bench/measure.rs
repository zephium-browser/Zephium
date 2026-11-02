//! What a browser costs while it loads a page: the memory and CPU of its
//! whole process family, and the network bytes of the processes in it.
//!
//! On macOS WebKit's XPC services are re-parented to launchd, so the family
//! is the parent-pid tree plus the services `launchctl print pid/<pid>` names
//! for any member. Each process is read with `proc_pid_rusage`, which has the
//! physical footprint Activity Monitor shows and is far cheaper than
//! `footprint`. On Windows one PowerShell call lists the tree.

#![cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::process::Child;

/// A process and when it started, so a reused pid is not read as the old one.
type Key = (u32, u64);

#[derive(Clone, Debug, Default)]
pub(super) struct Sample {
    /// Cumulative CPU nanoseconds per process.
    cpu_ns: HashMap<Key, u64>,
    pub(super) footprint_bytes: u64,
    pids: Vec<u32>,
}

impl Sample {
    pub(super) fn processes(&self) -> usize {
        self.cpu_ns.len()
    }

    pub(super) fn pids(&self) -> &[u32] {
        &self.pids
    }

    /// CPU milliseconds used between two samples. A process the earlier
    /// sample lacks started in between, so all of its time counts.
    pub(super) fn cpu_ms_since(&self, earlier: &Sample) -> f64 {
        let nanoseconds: u64 = self
            .cpu_ns
            .iter()
            .map(|(key, cpu)| cpu.saturating_sub(earlier.cpu_ns.get(key).copied().unwrap_or(0)))
            .sum();
        nanoseconds as f64 / 1e6
    }
}

struct Process {
    pid: u32,
    start: u64,
    footprint_bytes: u64,
    cpu_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Row {
    pub(super) pid: u32,
    pub(super) ppid: u32,
    pub(super) name: String,
}

pub(super) fn sample(root: u32) -> Result<Sample, String> {
    let processes = platform::read_family(root)?;
    if !processes.iter().any(|process| process.pid == root) {
        return Err("the browser process is gone".into());
    }
    let mut sample = Sample::default();
    for process in processes {
        sample.footprint_bytes += process.footprint_bytes;
        sample
            .cpu_ns
            .insert((process.pid, process.start), process.cpu_ns);
        sample.pids.push(process.pid);
    }
    Ok(sample)
}

pub(super) fn process_table() -> Result<Vec<Row>, String> {
    platform::process_table()
}

/// Kills a browser that did not quit, and everything it started.
pub(super) fn kill_tree(root: u32) {
    platform::kill_tree(root);
}

/// A `nettop` run that counts bytes received per process. It takes about five
/// seconds whatever it is asked for, so a caller starts it early and collects
/// the answer when it needs it. `nettop` only sees sockets that are still
/// open, so what a closed connection carried is missed.
pub(super) struct Traffic(Option<Child>);

impl Traffic {
    pub(super) fn start() -> Traffic {
        Traffic(platform::start_traffic())
    }

    /// Bytes received per pid; `None` where `nettop` is unavailable or failed.
    pub(super) fn finish(self) -> Option<HashMap<u32, u64>> {
        let output = self.0?.wait_with_output().ok()?;
        output
            .status
            .success()
            .then(|| parse_nettop(&String::from_utf8_lossy(&output.stdout)))
    }
}

/// Kilobytes the `pids` received between two `nettop` readings.
pub(super) fn net_kb(before: &HashMap<u32, u64>, after: &HashMap<u32, u64>, pids: &[u32]) -> f64 {
    let bytes: u64 = pids
        .iter()
        .filter_map(|pid| {
            let now = after.get(pid)?;
            Some(now.saturating_sub(before.get(pid).copied().unwrap_or(0)))
        })
        .sum();
    bytes as f64 / 1024.0
}

/// `nettop -P -L 1 -x -J bytes_in` prints a `,bytes_in,` header and then one
/// `name.pid,bytes,` row per process; a name may itself hold dots and spaces.
fn parse_nettop(output: &str) -> HashMap<u32, u64> {
    output
        .lines()
        .filter_map(|line| {
            let (name, bytes) = line.trim_end().trim_end_matches(',').rsplit_once(',')?;
            let bytes = bytes.trim().parse().ok()?;
            let (_, pid) = name.rsplit_once('.')?;
            Some((pid.parse().ok()?, bytes))
        })
        .collect()
}

/// `ps -A -o pid=,ppid=,comm=`: the command may hold spaces.
fn parse_ps(output: &str) -> Vec<Row> {
    fn token(text: &str) -> Option<(&str, &str)> {
        let text = text.trim_start();
        let end = text.find(char::is_whitespace).unwrap_or(text.len());
        (end > 0).then(|| (&text[..end], &text[end..]))
    }
    output
        .lines()
        .filter_map(|line| {
            let (pid, rest) = token(line)?;
            let (ppid, rest) = token(rest)?;
            Some(Row {
                pid: pid.parse().ok()?,
                ppid: ppid.parse().ok()?,
                name: rest.trim().to_string(),
            })
        })
        .collect()
}

/// `root` and everything below it.
fn family(root: u32, rows: &[Row]) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row.pid);
    }
    let mut seen = HashSet::from([root]);
    let mut order = vec![root];
    let mut next = 0;
    while next < order.len() {
        for child in children.get(&order[next]).into_iter().flatten() {
            if seen.insert(*child) {
                order.push(*child);
            }
        }
        next += 1;
    }
    order
}

/// The services `launchctl print pid/<pid>` lists as running for that pid.
#[cfg(any(target_os = "macos", test))]
fn helper_pids(launchctl: &str) -> Vec<u32> {
    launchctl
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid: u32 = words.next()?.parse().ok()?;
            let service = words.nth(1)?;
            (pid != 0 && service.starts_with("com.apple.WebKit.")).then_some(pid)
        })
        .collect()
}

#[cfg(any(windows, test))]
struct WindowsProcess {
    row: Row,
    private_bytes: u64,
    cpu_ns: u64,
    start: u64,
}

/// The JSON `windows_script` prints: an object per process, or a lone object.
#[cfg(any(windows, test))]
fn parse_windows(json: &str) -> Result<Vec<WindowsProcess>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        object @ serde_json::Value::Object(_) => vec![object],
        _ => return Err("unexpected process list".into()),
    };
    let number = |item: &serde_json::Value, key: &str| item.get(key).and_then(|v| v.as_u64());
    Ok(items
        .iter()
        .filter_map(|item| {
            Some(WindowsProcess {
                row: Row {
                    pid: u32::try_from(number(item, "pid")?).ok()?,
                    ppid: u32::try_from(number(item, "ppid")?).ok()?,
                    name: item.get("name")?.as_str()?.to_string(),
                },
                private_bytes: number(item, "private").unwrap_or(0),
                // Kernel and user time come in units of 100 ns.
                cpu_ns: number(item, "cpu").unwrap_or(0).saturating_mul(100),
                start: number(item, "start").unwrap_or(0),
            })
        })
        .collect())
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{family, helper_pids, parse_ps, Process, Row};
    use std::process::{Child, Command, Stdio};
    use std::sync::OnceLock;

    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    fn timebase() -> Result<(u32, u32), String> {
        static TIMEBASE: OnceLock<Option<(u32, u32)>> = OnceLock::new();
        TIMEBASE
            .get_or_init(|| {
                let mut info = std::mem::MaybeUninit::<MachTimebaseInfo>::zeroed();
                // SAFETY: the call receives a writable buffer of its exact ABI
                // type, which is read only after KERN_SUCCESS.
                if unsafe { mach_timebase_info(info.as_mut_ptr()) } != 0 {
                    return None;
                }
                // SAFETY: the successful call initialized the value.
                let info = unsafe { info.assume_init() };
                (info.numer != 0 && info.denom != 0).then_some((info.numer, info.denom))
            })
            .ok_or_else(|| "cannot read the macOS Mach timebase".to_string())
    }

    fn capture(program: &str, arguments: &[&str]) -> Result<String, String> {
        let output = Command::new(program)
            .args(arguments)
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run {program}: {e}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub(super) fn process_table() -> Result<Vec<Row>, String> {
        Ok(parse_ps(&capture("ps", &["-A", "-o", "pid=,ppid=,comm="])?))
    }

    pub(super) fn read_family(root: u32) -> Result<Vec<Process>, String> {
        let mut pids = family(root, &process_table()?);
        for owner in pids.clone() {
            let services = capture("launchctl", &["print", &format!("pid/{owner}")])?;
            for pid in helper_pids(&services) {
                if !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
        let (numer, denom) = timebase()?;
        Ok(pids
            .into_iter()
            .filter_map(|pid| usage(pid, numer, denom))
            .collect())
    }

    /// A process that exited, or belongs to someone else, is left out.
    fn usage(pid: u32, numer: u32, denom: u32) -> Option<Process> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
        // SAFETY: proc_pid_rusage receives a writable buffer of the exact V4
        // layout selected by RUSAGE_INFO_V4, read only after a zero return.
        let result = unsafe {
            libc::proc_pid_rusage(
                i32::try_from(pid).ok()?,
                libc::RUSAGE_INFO_V4,
                usage.as_mut_ptr().cast::<libc::rusage_info_t>(),
            )
        };
        if result != 0 {
            return None;
        }
        // SAFETY: the successful call initialized the whole value.
        let usage = unsafe { usage.assume_init() };
        let abstime = u128::from(usage.ri_user_time) + u128::from(usage.ri_system_time);
        Some(Process {
            pid,
            start: usage.ri_proc_start_abstime,
            footprint_bytes: usage.ri_phys_footprint,
            cpu_ns: u64::try_from(abstime * u128::from(numer) / u128::from(denom)).ok()?,
        })
    }

    pub(super) fn kill_tree(root: u32) {
        let Ok(table) = process_table() else { return };
        for pid in family(root, &table).into_iter().rev() {
            if let Ok(pid) = i32::try_from(pid) {
                // SAFETY: signalling a pid has no memory-safety requirement.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }

    pub(super) fn start_traffic() -> Option<Child> {
        Command::new("nettop")
            .args(["-P", "-L", "1", "-x", "-J", "bytes_in"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }
}

#[cfg(windows)]
mod platform {
    use super::{family, parse_windows, Process, Row};
    use std::process::{Child, Command};

    /// Private bytes, CPU time and start time of every process in one call.
    const SCRIPT: &str = "Get-CimInstance Win32_Process | ForEach-Object { [pscustomobject]@{ \
        pid = [int64]$_.ProcessId; ppid = [int64]$_.ParentProcessId; name = $_.Name; \
        private = [int64]$_.PrivatePageCount; \
        cpu = [int64]($_.KernelModeTime + $_.UserModeTime); \
        start = $(if ($_.CreationDate) { [int64]$_.CreationDate.ToFileTimeUtc() } else { 0 }) } } \
        | ConvertTo-Json -Compress";

    fn list() -> Result<Vec<super::WindowsProcess>, String> {
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .output()
            .map_err(|e| format!("cannot run powershell: {e}"))?;
        parse_windows(&String::from_utf8_lossy(&output.stdout))
    }

    pub(super) fn process_table() -> Result<Vec<Row>, String> {
        Ok(list()?.into_iter().map(|process| process.row).collect())
    }

    pub(super) fn read_family(root: u32) -> Result<Vec<Process>, String> {
        let all = list()?;
        let rows: Vec<Row> = all.iter().map(|process| process.row.clone()).collect();
        let wanted = family(root, &rows);
        Ok(all
            .into_iter()
            .filter(|process| wanted.contains(&process.row.pid))
            .map(|process| Process {
                pid: process.row.pid,
                start: process.start,
                footprint_bytes: process.private_bytes,
                cpu_ns: process.cpu_ns,
            })
            .collect())
    }

    pub(super) fn kill_tree(root: u32) {
        let _ = Command::new("taskkill")
            .args(["/pid", &root.to_string(), "/t", "/f"])
            .output();
    }

    pub(super) fn start_traffic() -> Option<Child> {
        None
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::{Process, Row};
    use std::process::Child;

    pub(super) fn process_table() -> Result<Vec<Row>, String> {
        Err("the blocker benchmark runs on macOS and Windows".into())
    }

    pub(super) fn read_family(_root: u32) -> Result<Vec<Process>, String> {
        process_table().map(|_| Vec::new())
    }

    pub(super) fn kill_tree(_root: u32) {}

    pub(super) fn start_traffic() -> Option<Child> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rows_whose_command_has_spaces() {
        let rows = parse_ps(
            "    1     0 /sbin/launchd\n 7020     1 /Applications/Brave Browser.app/Contents/MacOS/Brave Browser\n 7031  7020 Brave Browser Helper (GPU)\nbroken line\n",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].pid, 7020);
        assert_eq!(rows[1].ppid, 1);
        assert!(rows[1].name.ends_with("Brave Browser"));
        assert_eq!(rows[2].name, "Brave Browser Helper (GPU)");
    }

    #[test]
    fn the_family_is_the_root_and_all_its_descendants() {
        let row = |pid, ppid| Row {
            pid,
            ppid,
            name: String::new(),
        };
        let rows = [
            row(1, 0),
            row(10, 1),
            row(11, 10),
            row(12, 11),
            row(20, 1),
            row(13, 10),
        ];
        assert_eq!(family(10, &rows), [10, 11, 13, 12]);
        assert_eq!(family(99, &rows), [99]);
    }

    #[test]
    fn reads_webkit_services_from_launchctl() {
        let output = "pid/25249 = {\n\tservices = {\n\t\t   25249      - \tcom.apple.WebKit.WebContent.10E4B0D2\n\t\t       0      - \tcom.apple.WebKit.GPU\n\t\t   25247      - \tcom.apple.WebKit.Networking.AE6E131E\n\t\t   25250      - \tcom.apple.other.Service\n\t}\n}";
        assert_eq!(helper_pids(output), [25249, 25247]);
        assert!(helper_pids("Could not find domain for pid: 0").is_empty());
    }

    #[test]
    fn cpu_counts_processes_that_started_in_between() {
        let sample = |entries: &[(Key, u64)]| Sample {
            cpu_ns: entries.iter().copied().collect(),
            ..Sample::default()
        };
        let before = sample(&[((10, 1), 2_000_000), ((11, 1), 5_000_000)]);
        let after = sample(&[
            ((10, 1), 3_500_000),
            ((11, 1), 5_000_000),
            ((12, 1), 4_000_000),
        ]);
        assert!((after.cpu_ms_since(&before) - 5.5).abs() < 1e-9);
        // A reused pid with a new start time is a new process.
        let reused = sample(&[((10, 2), 1_000_000)]);
        assert!((reused.cpu_ms_since(&before) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn reads_nettop_rows_and_sums_what_arrived() {
        let output = ",bytes_in,\nBrave Browser.7020,1024,\nBrave Browser Helper.7031,4096,\ncom.apple.WebKit.Networking.88,300,\nbroken,\n";
        let traffic = parse_nettop(output);
        assert_eq!(traffic.len(), 3);
        assert_eq!(traffic[&7031], 4096);
        assert_eq!(traffic[&88], 300);
        let before = HashMap::from([(7020, 1024), (7031, 1000)]);
        // 7031 gained 3096, 7020 gained nothing, 88 is new, 9 is not ours.
        let kb = net_kb(&before, &traffic, &[7020, 7031, 88]);
        assert!((kb - (3096.0 + 300.0) / 1024.0).abs() < 1e-9);
        assert_eq!(net_kb(&traffic, &before, &[7020, 7031]), 0.0);
    }

    #[test]
    fn reads_the_windows_process_list() {
        let json = r#"[{"pid":4,"ppid":0,"name":"System","private":0,"cpu":0,"start":0},
            {"pid":100,"ppid":4,"name":"brave.exe","private":52428800,"cpu":15000000,"start":133}]"#;
        let processes = parse_windows(json).unwrap();
        assert_eq!(processes.len(), 2);
        assert_eq!(processes[1].row.name, "brave.exe");
        assert_eq!(processes[1].private_bytes, 52_428_800);
        assert_eq!(processes[1].cpu_ns, 1_500_000_000);
        assert_eq!(processes[1].start, 133);
        assert_eq!(
            parse_windows(r#"{"pid":1,"ppid":0,"name":"a"}"#)
                .unwrap()
                .len(),
            1
        );
        assert!(parse_windows("not json").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn samples_this_process_family() {
        let sample = sample(std::process::id()).unwrap();
        assert!(sample.processes() >= 1);
        assert!(sample.footprint_bytes > 0);
        assert!(sample.pids().contains(&std::process::id()));
        assert!(sample.cpu_ms_since(&Sample::default()) > 0.0);
    }
}
