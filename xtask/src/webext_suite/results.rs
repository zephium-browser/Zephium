//! `results.json`: the last run's verdicts and resource numbers, which the
//! next run compares itself against.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Memory growth below this is noise, whatever its share.
const MIN_GROWTH_MB: f64 = 10.0;
const MAX_GROWTH: f64 = 1.25;
const MIN_BUSY_GROWTH_MS: f64 = 40.0;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Status {
    Ok,
    Limited,
    Fail,
}

impl Status {
    pub(super) fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Limited => "limited",
            Status::Fail => "fail",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct Resources {
    /// Resident size of the helpers; falls when the system compresses them.
    pub(super) rss_mb: Option<f64>,
    /// Memory the helpers hold, compressed or not, as Activity Monitor counts it.
    #[serde(default)]
    pub(super) footprint_mb: Option<f64>,
    pub(super) idle_cpu_s: Option<f64>,
    #[serde(default)]
    pub(super) processes: usize,
    /// Least main-thread time an ordinary page lost in its first 1.5 s,
    /// over a few loads.
    #[serde(default)]
    pub(super) busy_ms: Option<f64>,
    /// Memory gained over many page loads and closes, with `--soak`.
    #[serde(default)]
    pub(super) soak_mb: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Record {
    pub(super) id: String,
    pub(super) status: Status,
    pub(super) reasons: Vec<String>,
    #[serde(flatten)]
    pub(super) resources: Resources,
    pub(super) duration_s: f64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(super) struct Results {
    pub(super) generated: u64,
    #[serde(default)]
    pub(super) baseline: Option<Resources>,
    #[serde(default)]
    pub(super) extensions: BTreeMap<String, Record>,
}

impl Results {
    pub(super) fn load(path: &Path) -> Results {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub(super) fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// What got worse since `self`, for the extensions `now` ran.
    pub(super) fn regressions(&self, now: &BTreeMap<String, Record>) -> Vec<String> {
        let mut found = Vec::new();
        for (name, record) in now {
            let Some(before) = self.extensions.get(name) else {
                continue;
            };
            if record.status > before.status {
                found.push(format!(
                    "{name}: {} -> {}",
                    before.status.label(),
                    record.status.label()
                ));
            }
            if let (Some(was), Some(is)) =
                (before.resources.footprint_mb, record.resources.footprint_mb)
            {
                if is > was * MAX_GROWTH && is - was > MIN_GROWTH_MB {
                    found.push(format!(
                        "{name}: memory {was:.0} MB -> {is:.0} MB (+{:.0}%)",
                        (is / was - 1.0) * 100.0
                    ));
                }
            }
            if let (Some(was), Some(is)) = (before.resources.busy_ms, record.resources.busy_ms) {
                if is > was * MAX_GROWTH && is - was > MIN_BUSY_GROWTH_MS {
                    found.push(format!("{name}: page busy {was:.0} ms -> {is:.0} ms"));
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(status: Status, rss_mb: f64) -> Record {
        Record {
            id: "x".into(),
            status,
            reasons: Vec::new(),
            resources: Resources {
                rss_mb: Some(rss_mb / 2.0),
                footprint_mb: Some(rss_mb),
                idle_cpu_s: Some(0.1),
                processes: 3,
                busy_ms: None,
                soak_mb: None,
            },
            duration_s: 30.0,
        }
    }

    #[test]
    fn flags_worse_status_and_memory_growth_over_a_quarter() {
        let mut before = Results::default();
        before
            .extensions
            .insert("A".into(), record(Status::Ok, 200.0));
        before
            .extensions
            .insert("B".into(), record(Status::Ok, 200.0));
        before
            .extensions
            .insert("C".into(), record(Status::Limited, 20.0));
        before
            .extensions
            .insert("D".into(), record(Status::Fail, 100.0));
        let mut now = BTreeMap::new();
        now.insert("A".into(), record(Status::Limited, 240.0));
        now.insert("B".into(), record(Status::Ok, 260.0));
        now.insert("C".into(), record(Status::Limited, 29.0));
        now.insert("D".into(), record(Status::Ok, 100.0));
        now.insert("E".into(), record(Status::Fail, 900.0));
        assert_eq!(
            before.regressions(&now),
            ["A: ok -> limited", "B: memory 200 MB -> 260 MB (+30%)"]
        );
    }
}
