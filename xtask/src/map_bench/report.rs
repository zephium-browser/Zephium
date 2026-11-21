use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;
use zephium_public_fetch::ReadRefusal;

use super::corpus::QueryLabel;
use super::engine::{FetchRefusal, ProbeFetch, RobotsStop};
use super::run::{latest_replay, Measure, QueryResult, RunManifest, MEASURE, OUTCOME};
use super::score::{Reason, Score, Verdict};
use super::store::{self, optional_json, query_dir, read_json, write_blob, write_json};
use super::{BenchError, Result};

#[derive(Default, Serialize)]
struct Distribution {
    samples: usize,
    p50: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p90: Option<u64>,
    max: Option<u64>,
}

fn distribution(mut samples: Vec<u64>, p90: bool) -> Distribution {
    samples.sort_unstable();
    let count = samples.len();
    let rank = |percent: usize| {
        samples
            .get((count * percent).div_ceil(100).saturating_sub(1))
            .copied()
    };
    Distribution {
        samples: count,
        p50: rank(50),
        p90: p90.then(|| rank(90)).flatten(),
        max: samples.last().copied(),
    }
}

#[derive(Default, Serialize)]
struct ProbeTotals {
    sites: u64,
    robots_answered: u64,
    homepage_answered: u64,
    llms_answered: u64,
    sitemap_answered: u64,
    blocked: u64,
    robots_blocks_all: u64,
}

type Counts = BTreeMap<&'static str, u64>;

fn counts() -> Counts {
    Verdict::ALL
        .into_iter()
        .map(|verdict| (verdict.label(), 0))
        .collect()
}

#[derive(Serialize)]
struct QueryScore {
    #[serde(flatten)]
    query: QueryLabel,
    #[serde(flatten)]
    score: Score,
}

#[derive(Serialize)]
struct Summary {
    totals: Counts,
    by_kind: BTreeMap<&'static str, Counts>,
    by_slice: BTreeMap<String, Counts>,
    metrics: BTreeMap<&'static str, Distribution>,
    refusals: BTreeMap<String, u64>,
    unrecorded_requests: u64,
    probe: ProbeTotals,
    queries: Vec<QueryScore>,
}

#[derive(Default)]
struct Samples {
    first_layer: Vec<u64>,
    first_place: Vec<u64>,
    full: Vec<u64>,
    wall: Vec<u64>,
    requests: Vec<u64>,
    wire_bytes: Vec<u64>,
    decoded_bytes: Vec<u64>,
    cpu: Vec<u64>,
    memory: Vec<u64>,
}

impl Samples {
    fn finish(self) -> BTreeMap<&'static str, Distribution> {
        [
            ("first_layer_ns", self.first_layer, true),
            ("first_place_ns", self.first_place, true),
            ("full_ns", self.full, true),
            ("wall_ns", self.wall, true),
            ("requests", self.requests, true),
            ("wire_bytes", self.wire_bytes, true),
            ("decoded_bytes", self.decoded_bytes, true),
            ("cpu_ns", self.cpu, false),
            ("peak_rss_delta_bytes", self.memory, false),
        ]
        .into_iter()
        .map(|(label, samples, p90)| (label, distribution(samples, p90)))
        .collect()
    }
}

struct Accumulator {
    summary: Summary,
    samples: Samples,
    refusal_classes: BTreeMap<String, u64>,
}

impl Default for Accumulator {
    fn default() -> Self {
        Self {
            summary: Summary {
                totals: counts(),
                by_kind: BTreeMap::new(),
                by_slice: BTreeMap::new(),
                metrics: BTreeMap::new(),
                refusals: BTreeMap::new(),
                unrecorded_requests: 0,
                probe: ProbeTotals::default(),
                queries: Vec::new(),
            },
            samples: Samples::default(),
            refusal_classes: ReadRefusal::ALL
                .into_iter()
                .map(FetchRefusal::Reader)
                .chain([
                    FetchRefusal::NotRecorded,
                    FetchRefusal::Interrupted,
                    FetchRefusal::Limit,
                    FetchRefusal::Storage,
                ])
                .map(|class| (class.to_string(), 0))
                .collect(),
        }
    }
}

impl Accumulator {
    fn push(
        &mut self,
        label: &QueryLabel,
        result: QueryResult,
        measure: Option<Measure>,
        replay: bool,
    ) {
        let verdict = result.score.verdict.label();
        *self.summary.totals.entry(verdict).or_default() += 1;
        let kind = self
            .summary
            .by_kind
            .entry(label.kind.label())
            .or_insert_with(counts);
        *kind.entry(verdict).or_default() += 1;
        let slice = self
            .summary
            .by_slice
            .entry(label.slice.clone().unwrap_or_else(|| "(unsliced)".into()))
            .or_insert_with(counts);
        *slice.entry(verdict).or_default() += 1;
        let ns = |time: std::time::Duration| u64::try_from(time.as_nanos()).unwrap_or(u64::MAX);
        if let Some(outcome) = result.outcome {
            self.samples
                .first_layer
                .extend(outcome.milestones.first_layer.map(ns));
            self.samples
                .first_place
                .extend(outcome.milestones.first_place.map(ns));
            self.samples.full.extend(outcome.milestones.full.map(ns));
            let probe = &mut self.summary.probe;
            for site in outcome.probe {
                probe.sites += 1;
                probe.robots_answered += u64::from(site.robots.answered());
                probe.homepage_answered += u64::from(
                    site.homepage.answered()
                        || site.www_retry.as_ref().is_some_and(ProbeFetch::answered),
                );
                probe.llms_answered += u64::from(site.llms.answered());
                probe.sitemap_answered += u64::from(site.sitemap.answered());
                probe.blocked += u64::from(site.blocked());
                probe.robots_blocks_all +=
                    u64::from(site.robots_stop == Some(RobotsStop::BlocksAll));
            }
        }
        if replay {
            self.summary.unrecorded_requests = self
                .summary
                .unrecorded_requests
                .saturating_add(result.activity.not_recorded);
            self.samples.requests.push(result.activity.site_requests);
            self.count_refusals(result.activity.refusals);
        } else if let Some(measure) = measure {
            self.samples.wall.push(ns(measure.wall));
            self.samples.requests.push(measure.requests);
            self.samples.wire_bytes.push(measure.wire_bytes);
            self.samples.decoded_bytes.push(measure.decoded_bytes);
            self.samples.cpu.extend(measure.cpu_ns);
            self.samples.memory.extend(measure.peak_rss_delta_bytes);
            self.count_refusals(measure.refusals);
        }
        self.summary.queries.push(QueryScore {
            query: label.clone(),
            score: result.score,
        });
    }

    fn count_refusals(&mut self, refusals: BTreeMap<String, u64>) {
        for (class, count) in refusals {
            if let Some(total) = self.refusal_classes.get_mut(&class) {
                *total = total.saturating_add(count);
            }
        }
    }

    fn finish(mut self) -> Summary {
        self.summary.metrics = self.samples.finish();
        self.summary.refusals = self
            .refusal_classes
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .collect();
        self.summary
    }
}

#[derive(Serialize)]
struct Report {
    recording: Summary,
    latest_replay: Option<Summary>,
}

impl Report {
    fn read(root: &Path) -> Result<Self> {
        let manifest = RunManifest::open(root)?;
        let mut recording = Accumulator::default();
        for query in &manifest.queries {
            let path = query_dir(root, &query.id, false)?;
            if !store::directory(&path, false)? {
                continue;
            }
            let outcome = optional_json(&path.join(OUTCOME))?;
            let measure = optional_json(&path.join(MEASURE))?;
            let outcome = outcome.unwrap_or_else(|| {
                QueryResult::unavailable(
                    Verdict::NotRecorded,
                    Reason::RecordingMissing,
                    Default::default(),
                )
            });
            recording.push(query, outcome, measure, false);
        }
        let latest_replay = latest_replay(root)?
            .map(|(path, manifest)| -> Result<_> {
                let mut replay = Accumulator::default();
                for query in &manifest.queries {
                    let path = query_dir(&path, &query.id, false)?;
                    let result = read_json(&path.join(OUTCOME))?;
                    replay.push(query, result, None, true);
                }
                Ok(replay.finish())
            })
            .transpose()?;
        Ok(Self {
            recording: recording.finish(),
            latest_replay,
        })
    }

    fn markdown(&self) -> String {
        let mut markdown = String::from("# Search map benchmark\n\n");
        self.recording.markdown(&mut markdown, "Recording", false);
        if let Some(replay) = &self.latest_replay {
            replay.markdown(&mut markdown, "Latest replay", true);
        }
        markdown
    }
}

impl Summary {
    fn markdown(&self, markdown: &mut String, title: &str, replay: bool) {
        let _ = writeln!(markdown, "## {title}\n");
        if replay {
            let _ = writeln!(
                markdown,
                "Unrecorded requests: {}\n",
                self.unrecorded_requests
            );
        }
        markdown.push_str("| Scope | pass | partial | fail | stayed_out_ok | stayed_out_broken | blocked | not_recorded |\n");
        markdown.push_str("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n");
        count_row(markdown, "Total", &self.totals);
        for (kind, counts) in &self.by_kind {
            count_row(markdown, kind, counts);
        }
        for (slice, counts) in &self.by_slice {
            count_row(markdown, &format!("Slice {slice}"), counts);
        }
        markdown.push_str(
            "\n| Metric | Samples | p50 | p90 | Max |\n| --- | ---: | ---: | ---: | ---: |\n",
        );
        for (metric, values) in &self.metrics {
            if replay
                && !matches!(
                    *metric,
                    "first_layer_ns" | "first_place_ns" | "full_ns" | "requests"
                )
            {
                continue;
            }
            let number =
                |value: Option<u64>| value.map_or_else(|| "—".into(), |value| value.to_string());
            let _ = writeln!(
                markdown,
                "| {metric} | {} | {} | {} | {} |",
                values.samples,
                number(values.p50),
                number(values.p90),
                number(values.max)
            );
        }
        markdown.push_str("\n| Refusal class | Count |\n| --- | ---: |\n");
        if self.refusals.is_empty() {
            markdown.push_str("| — | 0 |\n");
        }
        for (class, count) in &self.refusals {
            let _ = writeln!(markdown, "| {class} | {count} |");
        }
        let probe = &self.probe;
        markdown.push_str("\n| Probe | Sites |\n| --- | ---: |\n");
        for (label, count) in [
            ("Total", probe.sites),
            ("Robots answered", probe.robots_answered),
            ("Homepage answered", probe.homepage_answered),
            ("llms.txt answered", probe.llms_answered),
            ("Sitemap answered", probe.sitemap_answered),
            ("Blocked", probe.blocked),
            ("robots_blocks_all", probe.robots_blocks_all),
        ] {
            let _ = writeln!(markdown, "| {label} | {count} |");
        }
        markdown.push_str("\n| Query ID | Verdict | Reason |\n| --- | --- | --- |\n");
        for query in &self.queries {
            let score = &query.score;
            if !score.verdict.passed() {
                let _ = writeln!(
                    markdown,
                    "| `{}` | {} | {} |",
                    query.query.id,
                    score.verdict.label(),
                    score.reason.label()
                );
            }
        }
        if self
            .queries
            .iter()
            .all(|query| query.score.verdict.passed())
        {
            markdown.push_str("| — | — | — |\n");
        }
        markdown.push('\n');
    }
}

fn count_row(markdown: &mut String, name: &str, counts: &Counts) {
    let _ = write!(markdown, "| {name} |");
    for verdict in Verdict::ALL {
        let _ = write!(
            markdown,
            " {} |",
            counts.get(verdict.label()).copied().unwrap_or(0)
        );
    }
    markdown.push('\n');
}

pub(super) fn write(root: &Path, markdown: Option<&Path>) -> Result<()> {
    let _lock = store::record_lock(root, false)?;
    let default = root.join("summary.md");
    let markdown = markdown.unwrap_or(&default);
    let json = root.join("report.json");
    if markdown.starts_with(root)
        && (markdown.parent() != Some(root)
            || markdown
                .extension()
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("md")))
    {
        return Err(BenchError::new("markdown_overlaps_recording"));
    }
    let report = Report::read(root)?;
    let parent = markdown
        .parent()
        .ok_or(BenchError::new("markdown_has_no_parent"))?;
    store::create_parents(parent)?;
    write_blob(markdown, report.markdown().as_bytes())?;
    write_json(&json, &report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles_and_missing_samples() {
        let data = distribution(vec![8, 2, 7, 4, 6, 1, 9, 5, 3, 10], true);
        assert_eq!((data.p50, data.p90, data.max), (Some(5), Some(9), Some(10)));
        let data = distribution(vec![5], false);
        assert_eq!((data.p50, data.p90, data.max), (Some(5), None, Some(5)));
        let data = distribution(vec![], true);
        assert_eq!((data.p50, data.p90, data.max), (None, None, None));
    }
}
