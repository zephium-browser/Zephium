//! Memory and CPU of the WebKit helpers one lab process owns. They are
//! re-parented to launchd, and `lsappinfo` no longer lists them on macOS 27,
//! but `launchctl print pid/<lab>` names the services launched for it, `ps`
//! reads their resident size and CPU time, and `footprint` their footprint:
//! resident size drops when the system compresses pages, which a busy machine
//! does to whichever extension happens to be idle.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Default)]
pub(super) struct Sample {
    /// Cumulative CPU seconds per helper process.
    cpu: HashMap<u32, f64>,
    pub(super) rss_kb: u64,
    /// Absent where `footprint` reported on none of the helpers.
    pub(super) footprint_bytes: Option<u64>,
}

impl Sample {
    pub(super) fn processes(&self) -> usize {
        self.cpu.len()
    }

    /// CPU seconds the helpers used between two samples.
    pub(super) fn cpu_since(&self, earlier: &Sample) -> f64 {
        self.cpu
            .iter()
            .map(|(pid, cpu)| (cpu - earlier.cpu.get(pid).copied().unwrap_or(0.0)).max(0.0))
            .sum()
    }
}

pub(super) fn sample(lab: u32, scratch: &Path) -> Option<Sample> {
    let services = Command::new("launchctl")
        .args(["print", &format!("pid/{lab}")])
        .output()
        .ok()?;
    let pids = helper_pids(&String::from_utf8_lossy(&services.stdout));
    if pids.is_empty() {
        return None;
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let ps = Command::new("ps")
        .args(["-o", "pid=,rss=,time=", "-p", &list])
        .output()
        .ok()?;
    let mut sample = parse_ps(&String::from_utf8_lossy(&ps.stdout))?;
    let reported: Vec<Option<u64>> = pids.iter().map(|pid| footprint(*pid, scratch)).collect();
    sample.footprint_bytes = total(&reported);
    Some(sample)
}

/// A total missing a helper would read as a change in memory, so it is left
/// out rather than published short.
fn total(reported: &[Option<u64>]) -> Option<u64> {
    reported.iter().copied().sum()
}

/// `scratch` is shared by every pid of a run, so a failed call must not leave
/// the previous pid's report to be read again.
fn footprint(pid: u32, scratch: &Path) -> Option<u64> {
    let _ = std::fs::remove_file(scratch);
    let status = Command::new("footprint")
        .args(["--noCategories", "-j"])
        .arg(scratch)
        .args(["-p", &pid.to_string()])
        .output()
        .ok()?
        .status;
    if !status.success() {
        return None;
    }
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(scratch).ok()?).ok()?;
    report.get("total footprint")?.as_u64()
}

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

fn parse_ps(output: &str) -> Option<Sample> {
    let mut sample = Sample::default();
    for line in output.lines() {
        let mut words = line.split_whitespace();
        let pid: u32 = words.next()?.parse().ok()?;
        sample.rss_kb += words.next()?.parse::<u64>().ok()?;
        sample.cpu.insert(pid, cpu_seconds(words.next()?)?);
    }
    (!sample.cpu.is_empty()).then_some(sample)
}

/// `ps` prints cumulative CPU as `[[H:]M:]S.cc`.
fn cpu_seconds(text: &str) -> Option<f64> {
    text.split(':').try_fold(0.0, |total, part| {
        Some(total * 60.0 + part.parse::<f64>().ok()?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_footprint_total_needs_every_helper() {
        assert_eq!(total(&[Some(3), Some(4)]), Some(7));
        assert_eq!(total(&[Some(3), None]), None);
    }

    #[test]
    fn reads_helper_services_from_launchctl() {
        let output = "pid/25249 = {\n\tservices = {\n\t\t   25249      - \tcom.apple.WebKit.WebContent.10E4B0D2\n\t\t       0      - \tcom.apple.WebKit.GPU\n\t\t   25247      - \tcom.apple.WebKit.Networking.AE6E131E\n\t\t   25250      - \tcom.apple.other.Service\n\t}\n}";
        assert_eq!(helper_pids(output), [25249, 25247]);
    }

    #[test]
    fn sums_memory_and_diffs_cpu_per_process() {
        let before = parse_ps("100 2000 0:01.50\n101 3000 1:00.00\n").unwrap();
        let after = parse_ps("100 2100 0:03.75\n101 3500 1:00.25\n102 900 0:00.50\n").unwrap();
        assert_eq!(after.rss_kb, 6500);
        assert_eq!(after.processes(), 3);
        assert!((after.cpu_since(&before) - 3.0).abs() < 1e-9);
        assert_eq!(cpu_seconds("1:02:03.5"), Some(3723.5));
    }
}
