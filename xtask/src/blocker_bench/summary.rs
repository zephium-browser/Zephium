//! The tables printed after a run, written to `summary.md`, and the
//! differences against an earlier `results.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde_json::Value;

use super::results::{Launch, Results, SpeedometerRun, Visit, YoutubeStep};
use super::stats;

pub(super) const ZEPHIUM_ON: &str = "zephium-on";
pub(super) const ZEPHIUM_OFF: &str = "zephium-off";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Metric {
    Load,
    Fcp,
    Lcp,
    CpuMs,
    FootprintMb,
    NetKb,
    Requests,
    Ads,
    AdAreaKpx,
    Blocked,
}

const METRICS: [Metric; 10] = [
    Metric::Load,
    Metric::Fcp,
    Metric::Lcp,
    Metric::CpuMs,
    Metric::FootprintMb,
    Metric::NetKb,
    Metric::Requests,
    Metric::Ads,
    Metric::AdAreaKpx,
    Metric::Blocked,
];

impl Metric {
    fn label(self) -> &'static str {
        match self {
            Metric::Load => "load",
            Metric::Fcp => "fcp",
            Metric::Lcp => "lcp",
            Metric::CpuMs => "cpuMs",
            Metric::FootprintMb => "footprintMb",
            Metric::NetKb => "netKb",
            Metric::Requests => "requests",
            Metric::Ads => "ads",
            Metric::AdAreaKpx => "adAreaKpx",
            Metric::Blocked => "blocked",
        }
    }

    /// What a consent wall that stayed up would make look better than it was.
    fn is_ad(self) -> bool {
        matches!(
            self,
            Metric::Requests | Metric::Ads | Metric::AdAreaKpx | Metric::Blocked
        )
    }

    fn places(self) -> usize {
        match self {
            Metric::Requests | Metric::Ads | Metric::Blocked => 1,
            _ => 0,
        }
    }

    fn of(self, visit: &Visit) -> Option<f64> {
        let probe = |key: &str| visit.probe.as_ref()?.get(key)?.as_f64();
        match self {
            // The load event had not fired by probe time where this is 0.
            Metric::Load => probe("load").filter(|load| *load > 0.0),
            Metric::Fcp => probe("fcp"),
            Metric::Lcp => probe("lcp"),
            Metric::CpuMs => visit.cpu_ms,
            Metric::FootprintMb => visit.footprint_mb,
            Metric::NetKb => visit.net_kb,
            Metric::Requests => probe("requests"),
            Metric::Ads => Some(probe("adFrames")? + probe("adSlots")? + probe("siteAds")?),
            Metric::AdAreaKpx => probe("adAreaKpx"),
            Metric::Blocked => visit.blocked.map(|blocked| blocked as f64),
        }
    }
}

#[derive(Default)]
struct Cell {
    visits: usize,
    failed: usize,
    walled: usize,
    /// Visits that saw a consent wall, cleared or not.
    shown: usize,
    /// Probed visits whose load event had not fired by probe time.
    no_load: usize,
    values: BTreeMap<Metric, Vec<f64>>,
}

struct Tally {
    configs: Vec<String>,
    sites: Vec<String>,
    cells: BTreeMap<(String, String), Cell>,
    /// Sites whose consent wall stayed up in some configuration.
    flagged: BTreeSet<String>,
    /// Per configuration, the sites where other configurations met a wall and
    /// it never did: a blocker's cookie-notice list hides the wall without
    /// giving consent, so the page may never load its consent-gated ads.
    hidden: BTreeMap<String, Vec<String>>,
    /// Per configuration, the visits whose page was not visible, and all.
    background: BTreeMap<String, (usize, usize)>,
    /// Per configuration, the probed visits with no load event, and all probed.
    no_load: BTreeMap<String, (usize, usize)>,
}

impl Tally {
    fn new(results: &Results) -> Tally {
        let mut configs = results.configs.clone();
        let mut sites = results.sites.clone();
        let mut cells: BTreeMap<(String, String), Cell> = BTreeMap::new();
        let mut flagged = BTreeSet::new();
        for visit in &results.visits {
            if !configs.contains(&visit.config) {
                configs.push(visit.config.clone());
            }
            if !sites.contains(&visit.site) {
                sites.push(visit.site.clone());
            }
            let cell = cells
                .entry((visit.site.clone(), visit.config.clone()))
                .or_default();
            cell.visits += 1;
            cell.failed += usize::from(visit.failed());
            let walled = visit.walled();
            cell.walled += usize::from(walled);
            cell.shown += usize::from(visit.consent_shown);
            if visit.probe.is_some() && Metric::Load.of(visit).is_none() {
                cell.no_load += 1;
            }
            if walled {
                flagged.insert(visit.site.clone());
            }
            for metric in METRICS {
                if walled && metric.is_ad() {
                    continue;
                }
                if let Some(value) = metric.of(visit) {
                    cell.values.entry(metric).or_default().push(value);
                }
            }
        }
        let mut background: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut no_load: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for visit in &results.visits {
            if visit.probe.is_some() {
                let count = no_load.entry(visit.config.clone()).or_default();
                count.0 += usize::from(Metric::Load.of(visit).is_none());
                count.1 += 1;
            }
            let count = background.entry(visit.config.clone()).or_default();
            count.0 += usize::from(visit.visibility.as_deref().is_some_and(|v| v != "visible"));
            count.1 += 1;
        }
        let mut hidden: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for site in &sites {
            let saw = |config: &String| {
                cells
                    .get(&(site.clone(), config.clone()))
                    .is_some_and(|cell| cell.shown > 0)
            };
            for config in &configs {
                let measured = cells.contains_key(&(site.clone(), config.clone()));
                if measured && !saw(config) && configs.iter().any(&saw) {
                    hidden.entry(config.clone()).or_default().push(site.clone());
                }
            }
        }
        Tally {
            configs,
            sites,
            cells,
            flagged,
            hidden,
            background,
            no_load,
        }
    }

    fn cell(&self, site: &str, config: &str) -> Option<&Cell> {
        self.cells.get(&(site.to_string(), config.to_string()))
    }

    fn site_mean(&self, site: &str, config: &str, metric: Metric) -> Option<f64> {
        stats::mean(self.cell(site, config)?.values.get(&metric)?)
    }

    /// Per configuration and metric, the median over sites of each site's mean
    /// and the number of sites it came from. Only sites every configuration
    /// has a number for count, so the configurations are compared on the same
    /// pages, and a site with a wall that stayed up gives no ad number.
    fn medians(&self) -> BTreeMap<(String, Metric), (f64, usize)> {
        self.medians_over(|metric| {
            self.usable(metric, &self.measured(metric))
                .into_iter()
                .cloned()
                .collect()
        })
    }

    fn measured(&self, metric: Metric) -> Vec<&String> {
        self.configs
            .iter()
            .filter(|config| {
                self.sites
                    .iter()
                    .any(|site| self.site_mean(site, config, metric).is_some())
            })
            .collect()
    }

    /// The sites where every one of `configs` has a number for `metric`.
    fn usable(&self, metric: Metric, configs: &[&String]) -> Vec<&String> {
        self.sites
            .iter()
            .filter(|_| !configs.is_empty())
            .filter(|site| !(metric.is_ad() && self.flagged.contains(*site)))
            .filter(|site| {
                configs
                    .iter()
                    .all(|config| self.site_mean(site, config, metric).is_some())
            })
            .collect()
    }

    /// `medians` over the sites `sites` names for each metric.
    fn medians_over(&self, sites: impl Fn(Metric) -> Vec<String>) -> Medians {
        let mut medians = BTreeMap::new();
        for metric in METRICS {
            let usable = sites(metric);
            for config in self.measured(metric) {
                let means: Vec<f64> = usable
                    .iter()
                    .filter_map(|site| self.site_mean(site, config, metric))
                    .collect();
                if let Some(median) = stats::median(&means) {
                    medians.insert((config.clone(), metric), (median, usable.len()));
                }
            }
        }
        medians
    }
}

type Medians = BTreeMap<(String, Metric), (f64, usize)>;

fn on_off(medians: &Medians, metric: Metric) -> Option<f64> {
    let on = medians.get(&(ZEPHIUM_ON.to_string(), metric))?.0;
    let off = medians.get(&(ZEPHIUM_OFF.to_string(), metric))?.0;
    (off > 0.0).then(|| on / off)
}

fn number(value: f64, places: usize) -> String {
    format!("{value:.places$}")
}

/// `mean ± sd`, or the lone value when there is only one.
fn spread(values: &[f64], places: usize) -> String {
    match (stats::mean(values), stats::sd(values)) {
        (Some(mean), Some(sd)) if values.len() > 1 => {
            format!("{} ± {}", number(mean, places), number(sd, places))
        }
        (Some(mean), _) => number(mean, places),
        _ => "-".to_string(),
    }
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..headers.len())
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .chain([headers[column].chars().count(), 3])
                .max()
                .unwrap_or(3)
        })
        .collect();
    let line = |cells: Vec<&str>| {
        let padded: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        format!("| {} |\n", padded.join(" | "))
    };
    let mut out = line(headers.to_vec());
    out.push_str(&format!(
        "|{}|\n",
        widths
            .iter()
            .map(|width| "-".repeat(width + 2))
            .collect::<Vec<_>>()
            .join("|")
    ));
    for row in rows {
        out.push_str(&line(row.iter().map(String::as_str).collect()));
    }
    out
}

pub(super) fn render(results: &Results) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Blocker benchmark {}\n", results.label);
    let _ = writeln!(
        out,
        "{} mode, {}. {}\n",
        results.mode,
        super::utc_stamp(results.generated),
        machine_line(&results.machine)
    );
    if !results.visits.is_empty() {
        let tally = Tally::new(results);
        out.push_str(&medians_section(&tally));
        out.push_str(&sites_section(&tally));
        out.push_str(&walled_section(&tally, results));
    }
    out.push_str(&startup_section(results));
    out.push_str(&youtube_section(results));
    out.push_str(&speedometer_section(results));
    out
}

fn machine_line(machine: &Value) -> String {
    let text = |key: &str| machine.get(key).and_then(Value::as_str);
    let mut parts: Vec<String> = ["model", "cpu", "os", "gitRevision"]
        .iter()
        .filter_map(|key| text(key).map(str::to_string))
        .collect();
    if let Some(browsers) = machine.get("browsers").and_then(Value::as_object) {
        parts.extend(
            browsers
                .iter()
                .filter_map(|(name, version)| Some(format!("{name} {}", version.as_str()?))),
        );
    }
    parts.join("; ")
}

fn medians_section(tally: &Tally) -> String {
    let medians = tally.medians();
    let mut out = String::from("## Medians across sites\n\n");
    let mut headers = vec!["config"];
    headers.extend(METRICS.iter().map(|metric| metric.label()));
    let mut rows: Vec<Vec<String>> = tally
        .configs
        .iter()
        .map(|config| {
            let mut row = vec![config.clone()];
            row.extend(METRICS.iter().map(|metric| {
                medians
                    .get(&(config.clone(), *metric))
                    .map_or("-".to_string(), |(median, _)| {
                        number(*median, metric.places())
                    })
            }));
            row
        })
        .collect();
    if tally.configs.iter().any(|c| c == ZEPHIUM_ON)
        && tally.configs.iter().any(|c| c == ZEPHIUM_OFF)
    {
        let mut row = vec!["zephium on/off".to_string()];
        row.extend(METRICS.iter().map(|metric| {
            on_off(&medians, *metric).map_or("-".to_string(), |ratio| format!("{ratio:.2}x"))
        }));
        rows.push(row);
    }
    out.push_str(&table(&headers, &rows));
    let used = |ad: bool| {
        METRICS
            .iter()
            .filter(|metric| metric.is_ad() == ad)
            .flat_map(|metric| medians.iter().filter(move |((_, m), _)| m == metric))
            .map(|(_, (_, sites))| *sites)
            .max()
    };
    let _ = write!(
        out,
        "\nMedian of each site's mean over its runs, on the sites where every configuration with a number for that metric has one: {} sites for speed, CPU, memory and network, {} for the ad numbers",
        used(false).unwrap_or(0),
        used(true).unwrap_or(0)
    );
    let _ = writeln!(
        out,
        ". `load`, `fcp` and `lcp` are the page's own timing from the probe, not the driver's wait, which each browser decides differently and which stays in results.json as navMs; a load event that had not fired by probe time counts as no load. Memory is the physical footprint of the browser and its helpers (private bytes on Windows). `ads` is adFrames + adSlots + siteAds.\n"
    );
    for (config, (missing, probed)) in &tally.no_load {
        if *missing > 0 {
            let _ = writeln!(
                out,
                "No load event: {missing} of {probed} {config} visits had not fired it by probe time.\n"
            );
        }
    }
    out
}

fn sites_section(tally: &Tally) -> String {
    let mut headers = vec!["site", "config", "ok"];
    headers.extend(METRICS.iter().map(|metric| metric.label()));
    let mut rows = Vec::new();
    for site in &tally.sites {
        for config in &tally.configs {
            let Some(cell) = tally.cell(site, config) else {
                continue;
            };
            let mark = if tally.flagged.contains(site) {
                "*"
            } else {
                ""
            };
            let hidden = tally
                .hidden
                .get(config)
                .is_some_and(|sites| sites.contains(site));
            let mut row = vec![
                format!("{site}{mark}"),
                format!("{config}{}", if hidden { "†" } else { "" }),
                format!("{}/{}", cell.visits - cell.failed, cell.visits),
            ];
            row.extend(METRICS.iter().map(|metric| {
                let text = match cell.values.get(metric) {
                    Some(values) => spread(values, metric.places()),
                    None if metric.is_ad() && cell.walled > 0 => "wall".to_string(),
                    None => "-".to_string(),
                };
                if *metric == Metric::Load && cell.no_load > 0 {
                    format!("{text} [{} no load]", cell.no_load)
                } else {
                    text
                }
            }));
            rows.push(row);
        }
    }
    let mut out = format!(
        "## Per site\n\nMean ± sd over runs. `ok` counts loads that finished and probed cleanly; `*` marks a site whose consent wall stayed up somewhere (see below), and `wall` an ad number left out for that reason.\n\n{}\n",
        table(&headers, &rows)
    );
    for (config, (not_visible, visits)) in &tally.background {
        if *not_visible > 0 {
            let _ = writeln!(
                out,
                "Page not visible: {not_visible} of {visits} {config} visits ended with `document.visibilityState` other than visible, which changes what loads and runs.\n"
            );
        }
    }
    for (config, sites) in &tally.hidden {
        let _ = writeln!(
            out,
            "† {config}: the consent wall was hidden rather than cleared on {} (other configurations met one, {config} never did), so consent was not given and consent-gated ads may not have loaded.\n",
            sites.join(", ")
        );
    }
    out
}

fn walled_section(tally: &Tally, results: &Results) -> String {
    if tally.flagged.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Consent walls that stayed up\n\nThese sites give no ad numbers to the medians, because a configuration that never got past the wall would look as if it blocked more.\n\n");
    for site in &tally.flagged {
        let by_config: Vec<String> = tally
            .configs
            .iter()
            .filter_map(|config| {
                let cell = tally.cell(site, config)?;
                (cell.walled > 0).then(|| format!("{config} {}/{}", cell.walled, cell.visits))
            })
            .collect();
        let cmps: BTreeSet<&str> = results
            .visits
            .iter()
            .filter(|visit| visit.site == *site && visit.walled())
            .filter_map(|visit| visit.consent_cmp.as_deref())
            .collect();
        let _ = writeln!(
            out,
            "- {site}: {} ({})",
            by_config.join(", "),
            cmps.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    out.push('\n');
    out
}

fn startup_section(results: &Results) -> String {
    let mut groups: BTreeMap<(usize, String), Vec<&Launch>> = BTreeMap::new();
    for launch in &results.launches {
        let order = results
            .configs
            .iter()
            .position(|config| *config == launch.config)
            .unwrap_or(usize::MAX);
        groups
            .entry((order, format!("{} {}", launch.config, launch.phase)))
            .or_default()
            .push(launch);
    }
    if groups.is_empty() {
        return String::new();
    }
    let rows: Vec<Vec<String>> = groups
        .values()
        .map(|launches| {
            let ready: Vec<&Value> = launches.iter().filter_map(|l| l.ready.as_ref()).collect();
            let field = |key: &str| {
                let values: Vec<f64> = ready.iter().filter_map(|r| r.get(key)?.as_f64()).collect();
                spread(&values, 0)
            };
            vec![
                launches[0].config.clone(),
                launches[0].phase.clone(),
                ready.len().to_string(),
                (launches.len() - ready.len()).to_string(),
                field("startupMs"),
                field("windowMs"),
                field("bootstrapMs"),
                field("policyReadyMs"),
                field("sourcesSettledMs"),
            ]
        })
        .collect();
    format!(
        "## Launches\n\nMean ± sd in milliseconds. `startupMs` is the browser's own report of how long it took until it was ready; the Zephium protection app also reports when its window was shown, when its bootstrap finished and when its blocker policy was ready, all from Tauri setup. `sourcesSettledMs` is when its background list download and compile had finished, and is not part of `startupMs`; the measurement starts after it, and a launch whose lists were still updating after 120 s fails.\n\n{}\n",
        table(
            &["config", "phase", "launched", "failed", "startupMs", "windowMs", "bootstrapMs", "policyReadyMs", "sourcesSettledMs"],
            &rows
        )
    )
}

/// Runs saved before the option existed always rejected.
fn consent_line(results: &Results) -> String {
    let answer = results
        .options
        .get("youtubeConsent")
        .and_then(Value::as_str)
        .unwrap_or("reject");
    let answered: Vec<String> = results
        .configs
        .iter()
        .filter_map(|config| {
            let steps: Vec<&YoutubeStep> = results
                .youtube
                .iter()
                .filter(|step| step.config == *config)
                .collect();
            (!steps.is_empty()).then(|| {
                let count = steps.iter().filter(|step| step.consent_answered).count();
                format!("{config} {count} of {}", steps.len())
            })
        })
        .collect();
    format!(
        "YouTube's consent prompt was answered \"{answer} all\" on these steps: {}.",
        answered.join(", ")
    )
}

/// Which launches took the document-start script the watch timings come from.
fn preload_line(results: &Results) -> String {
    let mut taken: BTreeMap<(usize, &str), (usize, usize)> = BTreeMap::new();
    for launch in results
        .launches
        .iter()
        .filter(|launch| launch.phase == "run")
    {
        let order = results
            .configs
            .iter()
            .position(|config| *config == launch.config)
            .unwrap_or(usize::MAX);
        let count = taken.entry((order, &launch.config)).or_default();
        count.0 += usize::from(launch.preload == Some(true));
        count.1 += 1;
    }
    let parts: Vec<String> = taken
        .iter()
        .map(|((_, config), (yes, all))| format!("{config} {yes}/{all}"))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!(
            "Launches that took the document-start script: {}.",
            parts.join(", ")
        )
    }
}

fn youtube_section(results: &Results) -> String {
    let mut groups: BTreeMap<(usize, String), Vec<&YoutubeStep>> = BTreeMap::new();
    for step in &results.youtube {
        let order = results
            .configs
            .iter()
            .position(|config| *config == step.config)
            .unwrap_or(usize::MAX);
        groups
            .entry((order, format!("{} {}", step.config, step.step)))
            .or_default()
            .push(step);
    }
    if groups.is_empty() {
        return String::new();
    }
    let rows: Vec<Vec<String>> = groups
        .values()
        .map(|steps| {
            let result = |key: &str| -> Vec<f64> {
                steps
                    .iter()
                    .filter_map(|s| s.result.as_ref()?.get(key)?.as_f64())
                    .collect()
            };
            let flag = |key: &str| {
                let seen: Vec<bool> = steps
                    .iter()
                    .filter_map(|s| s.result.as_ref()?.get(key)?.as_bool())
                    .collect();
                if seen.is_empty() {
                    "-".to_string()
                } else {
                    format!("{}/{}", seen.iter().filter(|b| **b).count(), seen.len())
                }
            };
            let player_ads: Vec<f64> = steps
                .iter()
                .filter_map(|s| {
                    let result = s.result.as_ref()?;
                    let field = |key: &str| result.get(key)?.as_f64();
                    Some(field("adPlacements")? + field("playerAds")? + field("adSlots")?)
                })
                .collect();
            let seeks: Vec<Option<f64>> = steps
                .iter()
                .filter(|s| s.errors.is_empty() && !s.timed_out)
                .filter_map(|s| s.result.as_ref()?.get("seekMs")?.as_array().cloned())
                .flatten()
                .map(|seek| seek.as_f64())
                .collect();
            let column = |values: Vec<f64>, places: usize| spread(&values, places);
            let numbers = |pick: fn(&YoutubeStep) -> Option<f64>| -> Vec<f64> {
                steps.iter().filter_map(|s| pick(s)).collect()
            };
            vec![
                steps[0].config.clone(),
                steps[0].step.clone(),
                steps.len().to_string(),
                steps
                    .iter()
                    .filter(|s| !s.errors.is_empty() || s.timed_out)
                    .count()
                    .to_string(),
                column(result("feedAds"), 1),
                column(result("shorts"), 1),
                flag("adSeen"),
                column(result("adMs"), 0),
                flag("adUi"),
                flag("companion"),
                flag("enforcement"),
                column(result("played"), 1),
                column(result("stalledMs"), 0),
                column(result("contentFrameMs"), 0),
                column(seeks.iter().flatten().copied().collect(), 0),
                if seeks.is_empty() {
                    "-".to_string()
                } else {
                    format!(
                        "{}/{}",
                        seeks.iter().filter(|s| s.is_none()).count(),
                        seeks.len()
                    )
                },
                column(player_ads, 1),
                column(numbers(|s| s.blocked.map(|b| b as f64)), 1),
                column(numbers(|s| s.cpu_ms), 0),
                column(numbers(|s| s.footprint_mb), 0),
            ]
        })
        .collect();
    format!(
        "## YouTube\n\n`feedAds` counts the ad renderers seen on the home feed, or over eight steps down the shorts reel, which reach the number of shorts in `shorts` (a reel that does not move fails the step); on a watch page `ad seen`, `ad UI`, `companion` and `enforcement` count the videos that showed a pre-roll, the player's ad controls, a companion slot and a blocker warning, `adMs` is the time the player spent on ads out of 30 s, `start ms` is from navigation to the first `playing` or `timeupdate` event past 0 s with no ad showing, `seek ms` is from a seek to 40 % or 70 % of the video until it plays again, ads the seek brought included, `seek missed` counts the seeks that did not play again within 15 s out of the two each completed watch visit made (a failed visit's are not counted; fewer is better), and `player ad data` is what the player response carried for ads. {} {}\n\n{}\n",
        consent_line(results),
        preload_line(results),
        table(
            &[
                "config",
                "step",
                "n",
                "failed",
                "feedAds",
                "shorts",
                "ad seen",
                "adMs",
                "ad UI",
                "companion",
                "enforcement",
                "played s",
                "stalled ms",
                "start ms",
                "seek ms",
                "seek missed",
                "player ad data",
                "blocked",
                "cpuMs",
                "footprintMb"
            ],
            &rows
        )
    )
}

fn speedometer_section(results: &Results) -> String {
    let mut by_config: BTreeMap<(usize, &str), Vec<&SpeedometerRun>> = BTreeMap::new();
    for run in &results.speedometer {
        let order = results
            .configs
            .iter()
            .position(|config| *config == run.config)
            .unwrap_or(usize::MAX);
        by_config.entry((order, &run.config)).or_default().push(run);
    }
    if by_config.is_empty() {
        return String::new();
    }
    let values = |runs: &[&SpeedometerRun], pick: fn(&SpeedometerRun) -> Option<f64>| -> Vec<f64> {
        runs.iter().filter_map(|run| pick(run)).collect()
    };
    let means: BTreeMap<&str, [Option<f64>; 3]> = by_config
        .iter()
        .map(|((_, config), runs)| {
            (
                *config,
                [
                    stats::mean(&values(runs, |r| r.score)),
                    stats::mean(&values(runs, |r| r.cpu_ms)),
                    stats::mean(&values(runs, |r| r.footprint_mb)),
                ],
            )
        })
        .collect();
    let rows: Vec<Vec<String>> = by_config
        .values()
        .map(|runs| {
            vec![
                runs[0].config.clone(),
                runs.len().to_string(),
                runs.iter()
                    .filter(|run| run.score.is_none() || run.capped)
                    .count()
                    .to_string(),
                spread(&values(runs, |r| r.score), 1),
                spread(&values(runs, |r| r.range), 2),
                spread(&values(runs, |r| r.duration_s), 0),
                spread(&values(runs, |r| r.cpu_ms), 0),
                spread(&values(runs, |r| r.footprint_mb), 0),
            ]
        })
        .collect();
    let mut out = format!(
        "## Speedometer\n\nScore is higher the better; `range` is the page's own plus or minus for a run, `cpuMs` the CPU of the browser's processes from before the page opened to the score, `footprintMb` their footprint at the end. Mean ± sd over runs.\n\n{}\n",
        table(
            &["config", "runs", "failed", "score", "range", "seconds", "cpuMs", "footprintMb"],
            &rows
        )
    );
    if let (Some(on), Some(off)) = (means.get(ZEPHIUM_ON), means.get(ZEPHIUM_OFF)) {
        let percent = |pick: usize| match (on[pick], off[pick]) {
            (Some(on), Some(off)) if off > 0.0 => format!("{:+.1}%", (on / off - 1.0) * 100.0),
            _ => "-".to_string(),
        };
        let _ = writeln!(
            out,
            "Zephium on against off: score {}, CPU {}, memory {}.\n",
            percent(0),
            percent(1),
            percent(2)
        );
    }
    out
}

/// Each metric is compared over the sites where both runs measured every
/// configuration they share, so a different `--only`, a failed site or a
/// configuration only one run had does not move a median by itself.
pub(super) fn compare(old: &Results, new: &Results) -> String {
    let (before, after) = (Tally::new(old), Tally::new(new));
    let mut shared: BTreeMap<Metric, Vec<String>> = BTreeMap::new();
    let mut left_out: BTreeSet<String> = BTreeSet::new();
    for metric in METRICS {
        let names = |tally: &Tally| -> BTreeSet<String> {
            tally.measured(metric).into_iter().cloned().collect()
        };
        let configs: Vec<String> = names(&before)
            .intersection(&names(&after))
            .cloned()
            .collect();
        let configs: Vec<&String> = configs.iter().collect();
        let (a, b) = (
            before.usable(metric, &configs),
            after.usable(metric, &configs),
        );
        for site in a.iter().chain(&b) {
            if a.contains(site) && b.contains(site) {
                let sites = shared.entry(metric).or_default();
                if !sites.contains(site) {
                    sites.push((*site).clone());
                }
            } else {
                left_out.insert((*site).clone());
            }
        }
    }
    let sites = |metric: Metric| shared.get(&metric).cloned().unwrap_or_default();
    let (old_medians, new_medians) = (before.medians_over(sites), after.medians_over(sites));
    let mut rows = Vec::new();
    for ((config, metric), (now, _)) in &new_medians {
        let Some((before, _)) = old_medians.get(&(config.clone(), *metric)) else {
            continue;
        };
        rows.push(vec![
            config.clone(),
            metric.label().to_string(),
            number(*before, metric.places()),
            number(*now, metric.places()),
            change(*before, *now),
        ]);
    }
    for metric in METRICS {
        if let (Some(before), Some(now)) =
            (on_off(&old_medians, metric), on_off(&new_medians, metric))
        {
            rows.push(vec![
                "zephium on/off".to_string(),
                metric.label().to_string(),
                format!("{before:.2}x"),
                format!("{now:.2}x"),
                change(before, now),
            ]);
        }
    }
    let mut out = format!(
        "## Changes since {}\n\nMedians across sites, {} against {}.\n\n",
        if old.label.is_empty() {
            "the earlier run"
        } else {
            &old.label
        },
        if new.label.is_empty() {
            "this run"
        } else {
            &new.label
        },
        if old.label.is_empty() {
            "the earlier run"
        } else {
            &old.label
        },
    );
    if !left_out.is_empty() {
        let names: Vec<&str> = left_out.iter().map(String::as_str).collect();
        let _ = writeln!(
            out,
            "Only sites both runs measured count; left out of some or all metrics: {}.\n",
            names.join(", ")
        );
    }
    if rows.is_empty() {
        out.push_str("The two runs share no configuration and metric to compare.\n");
    } else {
        out.push_str(&table(
            &["config", "metric", "before", "now", "change"],
            &rows,
        ));
    }
    out
}

fn change(before: f64, now: f64) -> String {
    if before == 0.0 {
        return "-".to_string();
    }
    format!("{:+.1}%", (now / before - 1.0) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn visit(config: &str, site: &str, cpu_ms: f64, requests: f64, ads: f64) -> Visit {
        Visit {
            config: config.into(),
            site: site.into(),
            nav_ms: Some(1000.0),
            cpu_ms: Some(cpu_ms),
            footprint_mb: Some(500.0),
            probe: Some(json!({
                "requests": requests, "adFrames": ads, "adSlots": 0, "siteAds": 0, "adAreaKpx": ads * 100.0,
                "load": 900, "fcp": 400, "lcp": 700
            })),
            ..Visit::default()
        }
    }

    fn walled(mut visit: Visit) -> Visit {
        visit.consent_shown = true;
        visit.consent_cleared = Some(false);
        visit
    }

    fn results(visits: Vec<Visit>) -> Results {
        Results {
            label: "t".into(),
            mode: "sites".into(),
            configs: vec![ZEPHIUM_OFF.into(), ZEPHIUM_ON.into()],
            sites: vec!["a".into(), "b".into(), "c".into()],
            visits,
            ..Results::default()
        }
    }

    #[test]
    fn medians_are_taken_over_site_means_and_give_an_on_off_ratio() {
        let mut visits = Vec::new();
        for site in ["a", "b", "c"] {
            let size = match site {
                "a" => 10.0,
                "b" => 20.0,
                _ => 90.0,
            };
            visits.push(visit(ZEPHIUM_OFF, site, size, 100.0, 4.0));
            visits.push(visit(ZEPHIUM_OFF, site, size + 2.0, 100.0, 6.0));
            visits.push(visit(ZEPHIUM_ON, site, size / 2.0, 50.0, 0.0));
        }
        let tally = Tally::new(&results(visits));
        let medians = tally.medians();
        let get = |config: &str, metric| medians[&(config.to_string(), metric)];
        assert_eq!(get(ZEPHIUM_OFF, Metric::CpuMs), (21.0, 3));
        assert_eq!(get(ZEPHIUM_ON, Metric::CpuMs), (10.0, 3));
        assert_eq!(get(ZEPHIUM_OFF, Metric::Ads), (5.0, 3));
        assert_eq!(on_off(&medians, Metric::Requests), Some(0.5));
        assert_eq!(on_off(&medians, Metric::Ads), Some(0.0));
    }

    #[test]
    fn a_site_whose_wall_stayed_up_gives_no_ad_numbers_to_any_configuration() {
        let mut visits = Vec::new();
        for site in ["a", "b", "c"] {
            visits.push(visit(ZEPHIUM_OFF, site, 10.0, 100.0, 5.0));
            visits.push(visit(ZEPHIUM_ON, site, 10.0, 60.0, 1.0));
        }
        // Zephium never cleared the wall on "b", so its few ads are not a win.
        visits[3] = walled(visit(ZEPHIUM_ON, "b", 10.0, 5.0, 0.0));
        let tally = Tally::new(&results(visits));
        assert!(tally.flagged.contains("b"));
        let medians = tally.medians();
        assert_eq!(
            medians[&(ZEPHIUM_ON.to_string(), Metric::Requests)],
            (60.0, 2)
        );
        assert_eq!(
            medians[&(ZEPHIUM_OFF.to_string(), Metric::Requests)],
            (100.0, 2)
        );
        // Speed and memory still count every site.
        assert_eq!(medians[&(ZEPHIUM_ON.to_string(), Metric::CpuMs)], (10.0, 3));
        let text = render(&results(vec![
            visit(ZEPHIUM_OFF, "b", 10.0, 100.0, 5.0),
            walled(visit(ZEPHIUM_ON, "b", 10.0, 5.0, 0.0)),
        ]));
        assert!(text.contains("b*"));
        assert!(text.contains("wall"));
        assert!(text.contains("## Consent walls that stayed up"));
    }

    #[test]
    fn a_configuration_missing_a_site_does_not_move_the_others_medians() {
        let visits = vec![
            visit(ZEPHIUM_OFF, "a", 10.0, 100.0, 5.0),
            visit(ZEPHIUM_OFF, "b", 30.0, 100.0, 5.0),
            visit(ZEPHIUM_ON, "b", 20.0, 50.0, 1.0),
        ];
        let medians = Tally::new(&results(visits)).medians();
        assert_eq!(
            medians[&(ZEPHIUM_OFF.to_string(), Metric::CpuMs)],
            (30.0, 1)
        );
        assert_eq!(medians[&(ZEPHIUM_ON.to_string(), Metric::CpuMs)], (20.0, 1));
    }

    #[test]
    fn timed_out_visits_keep_their_numbers_and_a_failed_probe_has_none() {
        let mut slow = visit(ZEPHIUM_OFF, "a", 10.0, 100.0, 5.0);
        slow.timed_out = true;
        slow.nav_ms = Some(30000.0);
        let mut broken = visit(ZEPHIUM_OFF, "a", 10.0, 100.0, 5.0);
        broken.probe = None;
        broken.errors.push("probe failed".into());
        let tally = Tally::new(&results(vec![slow, broken]));
        let cell = tally.cell("a", ZEPHIUM_OFF).unwrap();
        assert_eq!((cell.visits, cell.failed), (2, 2));
        // The driver's timeout does not touch the page's own load time, and
        // the other numbers of a timed-out visit stay in.
        assert_eq!(cell.values.get(&Metric::Load).map(Vec::len), Some(1));
        assert_eq!(cell.values.get(&Metric::CpuMs).map(Vec::len), Some(2));
        assert_eq!(cell.values.get(&Metric::Requests).map(Vec::len), Some(1));
    }

    #[test]
    fn a_load_event_that_had_not_fired_is_counted_and_left_out_of_the_load_numbers() {
        let mut unfinished = visit("brave", "a", 10.0, 100.0, 5.0);
        unfinished.probe = Some(json!({
            "requests": 90, "adFrames": 0, "adSlots": 0, "siteAds": 0, "adAreaKpx": 0,
            "load": 0, "fcp": 300, "lcp": null
        }));
        let finished = visit("brave", "b", 10.0, 100.0, 5.0);
        let mut results = results(vec![unfinished, finished]);
        results.configs = vec!["brave".into()];
        let tally = Tally::new(&results);
        let cell = tally.cell("a", "brave").unwrap();
        assert_eq!(cell.no_load, 1);
        assert!(!cell.values.contains_key(&Metric::Load));
        assert_eq!(cell.values[&Metric::Fcp], [300.0]);
        assert!(!cell.values.contains_key(&Metric::Lcp));
        assert_eq!(tally.no_load["brave"], (1, 2));
        let text = render(&results);
        assert!(text.contains("[1 no load]"), "{text}");
        assert!(
            text.contains("No load event: 1 of 2 brave visits"),
            "{text}"
        );
    }

    #[test]
    fn a_wall_only_other_configurations_met_is_reported_as_hidden() {
        let shown = |mut visit: Visit| {
            visit.consent_shown = true;
            visit.consent_cleared = Some(true);
            visit
        };
        let visits = vec![
            shown(visit(ZEPHIUM_OFF, "a", 10.0, 100.0, 5.0)),
            visit("brave", "a", 10.0, 100.0, 5.0),
            visit(ZEPHIUM_OFF, "b", 10.0, 100.0, 5.0),
            visit("brave", "b", 10.0, 100.0, 5.0),
        ];
        let mut results = results(visits);
        results.configs = vec![ZEPHIUM_OFF.into(), "brave".into()];
        let tally = Tally::new(&results);
        assert_eq!(tally.hidden["brave"], ["a"]);
        assert!(!tally.hidden.contains_key(ZEPHIUM_OFF));
        let text = render(&results);
        assert!(text.contains("brave†"), "{text}");
        assert!(
            text.contains("† brave: the consent wall was hidden rather than cleared on a"),
            "{text}"
        );
    }

    #[test]
    fn visits_whose_page_was_not_visible_are_counted() {
        let mut hidden = visit(ZEPHIUM_ON, "a", 10.0, 100.0, 5.0);
        hidden.visibility = Some("hidden".into());
        let mut shown = visit(ZEPHIUM_ON, "b", 10.0, 100.0, 5.0);
        shown.visibility = Some("visible".into());
        let results = results(vec![hidden, shown]);
        assert_eq!(Tally::new(&results).background[ZEPHIUM_ON], (1, 2));
        assert!(render(&results).contains("1 of 2 zephium-on visits"));
    }

    #[test]
    fn spread_and_tables_are_formatted() {
        assert_eq!(spread(&[2.0, 4.0], 1), "3.0 ± 1.4");
        assert_eq!(spread(&[7.0], 0), "7");
        assert_eq!(spread(&[], 0), "-");
        let text = table(&["a", "long"], &[vec!["x".into(), "yy".into()]]);
        assert_eq!(text, "| a   | long |\n|-----|------|\n| x   | yy   |\n");
    }

    #[test]
    fn a_comparison_reports_the_change_of_each_median() {
        let old = results(vec![
            visit(ZEPHIUM_OFF, "a", 100.0, 100.0, 5.0),
            visit(ZEPHIUM_ON, "a", 80.0, 60.0, 1.0),
        ]);
        let new = results(vec![
            visit(ZEPHIUM_OFF, "a", 100.0, 100.0, 5.0),
            visit(ZEPHIUM_ON, "a", 60.0, 60.0, 1.0),
        ]);
        let text = compare(&old, &new);
        assert!(text.contains("| zephium-on"), "{text}");
        assert!(text.contains("-25.0%"), "{text}");
        assert!(text.contains("zephium on/off"), "{text}");
    }

    #[test]
    fn a_comparison_uses_only_the_sites_both_runs_measured() {
        let old = results(vec![
            visit(ZEPHIUM_OFF, "a", 100.0, 100.0, 5.0),
            visit(ZEPHIUM_OFF, "b", 300.0, 100.0, 5.0),
        ]);
        let mut new = results(vec![visit(ZEPHIUM_OFF, "a", 100.0, 100.0, 5.0)]);
        new.sites = vec!["a".into()];
        let text = compare(&old, &new);
        assert!(text.contains("+0.0%"), "{text}");
        assert!(!text.contains("-50.0%"), "{text}");
        assert!(
            text.contains("left out of some or all metrics: b."),
            "{text}"
        );
    }

    #[test]
    fn a_configuration_only_one_run_measured_does_not_empty_the_comparison() {
        let old = results(vec![
            visit(ZEPHIUM_OFF, "a", 100.0, 100.0, 5.0),
            visit(ZEPHIUM_ON, "a", 80.0, 60.0, 1.0),
        ]);
        let new = results(vec![
            visit(ZEPHIUM_OFF, "a", 100.0, 100.0, 5.0),
            visit(ZEPHIUM_ON, "a", 60.0, 60.0, 1.0),
            visit("ublock", "b", 70.0, 60.0, 1.0),
        ]);
        let text = compare(&old, &new);
        assert!(text.contains("-25.0%"), "{text}");
        assert!(!text.contains("share no configuration"), "{text}");
    }

    #[test]
    fn speedometer_scores_are_averaged_and_zephium_on_is_set_against_off() {
        let mut results = results(Vec::new());
        for (config, score) in [
            (ZEPHIUM_OFF, 90.0),
            (ZEPHIUM_OFF, 110.0),
            (ZEPHIUM_ON, 99.0),
            (ZEPHIUM_ON, 121.0),
        ] {
            results.speedometer.push(SpeedometerRun {
                config: config.into(),
                score: Some(score),
                range: Some(1.5),
                cpu_ms: Some(1000.0),
                footprint_mb: Some(500.0),
                ..SpeedometerRun::default()
            });
        }
        let text = render(&results);
        assert!(text.contains("100.0 ± 14.1"), "{text}");
        assert!(
            text.contains("score +10.0%, CPU +0.0%, memory +0.0%"),
            "{text}"
        );
    }

    #[test]
    fn launches_and_youtube_steps_get_their_own_tables() {
        let mut results = results(Vec::new());
        for startup in [900.0, 1100.0] {
            results.launches.push(Launch {
                config: ZEPHIUM_ON.into(),
                phase: "warm".into(),
                ready: Some(
                    json!({ "startupMs": startup, "bootstrapMs": 300, "policyReadyMs": 700 }),
                ),
                ..Launch::default()
            });
        }
        results.launches.push(Launch {
            config: ZEPHIUM_ON.into(),
            phase: "warm".into(),
            error: Some("exited".into()),
            ..Launch::default()
        });
        for preload in [Some(true), Some(false)] {
            results.launches.push(Launch {
                config: ZEPHIUM_ON.into(),
                phase: "run".into(),
                preload,
                ..Launch::default()
            });
        }
        results.youtube.push(YoutubeStep {
            config: ZEPHIUM_ON.into(),
            step: "watch".into(),
            result: Some(json!({ "adSeen": false, "adMs": 0, "adPlacements": 2, "playerAds": 0, "adSlots": 1, "played": 29.5, "stalledMs": 0, "contentFrameMs": 1840, "seekMs": [420, null] })),
            ..YoutubeStep::default()
        });
        results.youtube.push(YoutubeStep {
            config: ZEPHIUM_ON.into(),
            step: "watch".into(),
            result: Some(json!({ "seekMs": [null, null] })),
            timed_out: true,
            ..YoutubeStep::default()
        });
        let text = render(&results);
        assert!(
            text.contains(&format!("document-start script: {ZEPHIUM_ON} 1/2.")),
            "{text}"
        );
        assert!(text.contains("1000 ± 141"), "{text}");
        assert!(text.contains("| 2 "), "{text}");
        assert!(text.contains("0/1"), "{text}");
        assert!(text.contains("3.0"), "{text}");
        assert!(text.contains("| 1840 "), "{text}");
        assert!(text.contains("| 420 "), "{text}");
        assert!(text.contains("| 1/2 "), "{text}");
    }

    #[test]
    fn the_consent_line_counts_the_steps_that_answered_per_browser() {
        let mut results = results(Vec::new());
        results.configs = vec![ZEPHIUM_ON.into(), "brave".into()];
        results.options = json!({ "youtubeConsent": "accept" });
        for (config, answered) in [(ZEPHIUM_ON, true), (ZEPHIUM_ON, false), ("brave", false)] {
            results.youtube.push(YoutubeStep {
                config: config.into(),
                consent_answered: answered,
                ..YoutubeStep::default()
            });
        }
        assert_eq!(
            consent_line(&results),
            format!(
                "YouTube's consent prompt was answered \"accept all\" on these steps: {ZEPHIUM_ON} 1 of 2, brave 0 of 1."
            )
        );
    }
}
