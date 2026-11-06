//! `results.json`: every raw sample of a run, which the summary reads and a
//! later run can be compared against.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) const SCHEMA: u32 = 1;

/// One site loaded once in one configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Visit {
    pub(super) config: String,
    pub(super) run: usize,
    pub(super) site: String,
    pub(super) kind: String,
    pub(super) url: String,
    /// How long the driver waited for the page, which its browser decides
    /// differently from every other; the page's own timing is the probe's.
    #[serde(alias = "loadMs")]
    pub(super) nav_ms: Option<f64>,
    pub(super) timed_out: bool,
    pub(super) cpu_ms: Option<f64>,
    /// Footprint of the whole browser before the page opened.
    pub(super) footprint_before_mb: Option<f64>,
    pub(super) footprint_mb: Option<f64>,
    pub(super) processes: Option<usize>,
    pub(super) net_kb: Option<f64>,
    /// `document.visibilityState` when the scrolling ended: a page that is
    /// not visible loads and runs differently.
    pub(super) visibility: Option<String>,
    pub(super) blocked_before: Option<u64>,
    pub(super) blocked_after: Option<u64>,
    pub(super) blocked: Option<u64>,
    pub(super) consent_shown: bool,
    /// Gone two seconds after the click and still gone when the page was probed.
    pub(super) consent_cleared: Option<bool>,
    pub(super) consent_cmp: Option<String>,
    pub(super) consent_clicked: Option<String>,
    pub(super) probe: Option<Value>,
    pub(super) errors: Vec<String>,
}

impl Visit {
    /// A consent wall was there and stayed.
    pub(super) fn walled(&self) -> bool {
        self.consent_shown && self.consent_cleared == Some(false)
    }

    pub(super) fn failed(&self) -> bool {
        self.timed_out || !self.errors.is_empty()
    }
}

/// One launch of a browser: the ready event, or why there was none.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Launch {
    pub(super) config: String,
    pub(super) run: usize,
    /// `run` for a measured run, `cold` and `warm` for a Zephium session's
    /// first and later launches, `fresh` for a startup launch elsewhere.
    pub(super) phase: String,
    pub(super) ready: Option<Value>,
    /// Whether the driver took the script that runs at document start.
    pub(super) preload: Option<bool>,
    /// The wait after Firefox was ready, in milliseconds.
    pub(super) settle_ms: Option<u64>,
    pub(super) error: Option<String>,
    /// The components a Chromium profile held when its YouTube pass ended:
    /// each one's name, version and when it was written, in seconds from the
    /// launch, so a list fetched during the pass shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) components: Option<Vec<Value>>,
}

/// One page of a `--youtube` pass, with the script's raw result.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct YoutubeStep {
    pub(super) config: String,
    pub(super) run: usize,
    /// `feed`, `watch` or `shorts`.
    pub(super) step: String,
    pub(super) id: Option<String>,
    pub(super) url: String,
    /// How long the driver waited for the page to load.
    #[serde(alias = "loadMs")]
    pub(super) nav_ms: Option<f64>,
    pub(super) timed_out: bool,
    pub(super) cpu_ms: Option<f64>,
    pub(super) footprint_mb: Option<f64>,
    pub(super) blocked: Option<u64>,
    pub(super) consent_answered: bool,
    pub(super) result: Option<Value>,
    pub(super) errors: Vec<String>,
}

/// One Speedometer run in one configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct SpeedometerRun {
    pub(super) config: String,
    pub(super) run: usize,
    pub(super) url: String,
    /// How long the driver waited for the page to load.
    #[serde(alias = "loadMs")]
    pub(super) nav_ms: Option<f64>,
    /// The page's score and its plus-or-minus range, as numbers and as shown.
    pub(super) score: Option<f64>,
    pub(super) range: Option<f64>,
    pub(super) score_text: Option<String>,
    pub(super) range_text: Option<String>,
    /// The run was still going at the ten-minute cap.
    pub(super) capped: bool,
    pub(super) polls: usize,
    pub(super) duration_s: Option<f64>,
    /// CPU of the browser's processes from before the page opened to the score.
    pub(super) cpu_ms: Option<f64>,
    pub(super) footprint_before_mb: Option<f64>,
    pub(super) footprint_mb: Option<f64>,
    pub(super) blocked: Option<u64>,
    /// Speedometer's own mean time per suite, in milliseconds.
    pub(super) suites: Option<serde_json::Value>,
    pub(super) errors: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Results {
    pub(super) schema: u32,
    pub(super) label: String,
    /// Seconds since the epoch.
    pub(super) generated: u64,
    /// `sites`, `youtube`, `startup` or `speedometer`.
    pub(super) mode: String,
    /// In the order they were asked for.
    pub(super) configs: Vec<String>,
    pub(super) sites: Vec<String>,
    pub(super) machine: Value,
    pub(super) options: Value,
    pub(super) launches: Vec<Launch>,
    pub(super) visits: Vec<Visit>,
    pub(super) youtube: Vec<YoutubeStep>,
    pub(super) speedometer: Vec<SpeedometerRun>,
}

impl Results {
    pub(super) fn load(path: &Path) -> Result<Results, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub(super) fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        // A run stopped mid-write keeps the previous checkpoint, and two runs
        // saving to the same file never share a partial one.
        let partial = path.with_extension(format!("json.{}.partial", std::process::id()));
        std::fs::write(&partial, json).map_err(|e| e.to_string())?;
        std::fs::rename(&partial, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_that_was_never_cleared_is_walled() {
        let mut visit = Visit {
            consent_shown: true,
            consent_cleared: Some(true),
            ..Visit::default()
        };
        assert!(!visit.walled());
        visit.consent_cleared = Some(false);
        assert!(visit.walled());
        visit.consent_shown = false;
        visit.consent_cleared = None;
        assert!(!visit.walled());
    }

    #[test]
    fn results_survive_a_round_trip_and_tolerate_missing_fields() {
        let mut results = Results {
            schema: SCHEMA,
            label: "x".into(),
            ..Results::default()
        };
        results.visits.push(Visit {
            site: "cnn".into(),
            nav_ms: Some(1234.5),
            probe: Some(serde_json::json!({ "requests": 80 })),
            ..Visit::default()
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("results.json");
        results.save(&path).unwrap();
        results.save(&path).unwrap();
        let names: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["results.json"]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"navMs\": 1234.5"));
        let old: Visit = serde_json::from_str(r#"{"loadMs": 99.5}"#).unwrap();
        assert_eq!(old.nav_ms, Some(99.5));
        let loaded = Results::load(&path).unwrap();
        assert_eq!(loaded.visits[0].probe.as_ref().unwrap()["requests"], 80);
        let sparse: Results = serde_json::from_str(r#"{"label":"old"}"#).unwrap();
        assert!(sparse.visits.is_empty());
    }
}
