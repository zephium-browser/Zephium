use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;
use zephium_map::session::Budget;
use zephium_public_fetch::ReadRefusal;

use super::corpus::{Kind, QueryLabel};
use super::engine::{EngineFailure, EngineKind, FetchRefusal, ProbeFetch, RobotsStop};
use super::run::{
    latest_replay, Measure, QueryResult, ReplayMeasure, RunManifest, MEASURE, OUTCOME,
};
use super::score::{Reason, Score, Verdict};
use super::session::{MAX_PLACES, PHASES, PLACE_SOURCES};
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

fn ns(time: std::time::Duration) -> u64 {
    u64::try_from(time.as_nanos()).unwrap_or(u64::MAX)
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
    engine_failure: Option<EngineFailure>,
}

#[derive(Serialize)]
struct Summary {
    totals: Counts,
    by_kind: BTreeMap<&'static str, Counts>,
    by_slice: BTreeMap<String, Counts>,
    metrics: BTreeMap<&'static str, Distribution>,
    refusals: BTreeMap<String, u64>,
    unrecorded_requests: u64,
    matched_relaxed: u64,
    probe: ProbeTotals,
    synthetic_results: usize,
    session: Option<SessionTotals>,
    queries: Vec<QueryScore>,
}

#[derive(Default, Serialize)]
struct SessionTotals {
    sites: u64,
    places_with_three_sections: u64,
    sources: Counts,
    shells: u64,
    walls: u64,
    robots_denials: u64,
    render_wanted: u64,
    anonymous_results_wanted: u64,
    budgets: BTreeMap<String, u64>,
    requests_per_query_by_phase: BTreeMap<String, Distribution>,
    passage: Counts,
}

impl SessionTotals {
    fn collect(&mut self, label: &QueryLabel, result: &QueryResult, samples: &mut Samples) {
        let site_count = match label.kind {
            Kind::Place => 1,
            Kind::Compare => 2,
            _ => 0,
        };
        self.sites += site_count;
        if let Some(score) = &result.score.passage {
            for (class, occurred) in [
                (
                    "answered_from_listed_source",
                    score.answer_from_listed_source,
                ),
                ("answered_wrong", score.wrong_answer_shown),
                ("nothing_returned", score.nothing_returned),
                ("page_could_not_be_read", score.page_could_not_be_read),
            ] {
                *self.passage.entry(class).or_default() += u64::from(occurred);
            }
        }
        let Some(outcome) = &result.outcome else {
            return;
        };
        let count = |key: &str| outcome.stats.get(key).copied().unwrap_or(0);
        self.shells = self.shells.saturating_add(count("shells"));
        self.walls = self.walls.saturating_add(count("walls"));
        self.render_wanted = self.render_wanted.saturating_add(count("render_wanted"));
        self.anonymous_results_wanted = self
            .anonymous_results_wanted
            .saturating_add(count("anonymous_results_wanted"));
        for phase in PHASES {
            let phase = format!("{phase:?}");
            self.robots_denials = self
                .robots_denials
                .saturating_add(count(&format!("phase.{phase}.refused.Robots")));
            if outcome.stats.contains_key("cancelled") {
                samples
                    .phases
                    .entry(phase.clone())
                    .or_default()
                    .push(count(&format!("phase.{phase}.started")));
            }
        }
        for budget in [
            Budget::Fetches,
            Budget::HostFetches,
            Budget::Concurrent,
            Budget::HostConcurrent,
            Budget::TotalBytes,
            Budget::HtmlBytes,
            Budget::XmlBytes,
            Budget::TextBytes,
            Budget::SectionPages,
            Budget::ResultPages,
            Budget::Elapsed,
            Budget::LastStart,
            Budget::Timeout,
        ] {
            let class = format!("{budget:?}");
            let hits = count(&format!("budget.{class}"));
            if hits != 0 {
                let total = self.budgets.entry(class).or_default();
                *total = total.saturating_add(hits);
            }
        }
        for index in 0..MAX_PLACES {
            if let Some(&sections) = outcome.stats.get(&format!("place.{index}.sections")) {
                if site_count > 0 {
                    self.places_with_three_sections += u64::from(sections >= 3);
                    samples.sections.push(sections);
                    samples
                        .primary
                        .extend(outcome.stats.get(&format!("place.{index}.primary")));
                    samples
                        .secondary
                        .extend(outcome.stats.get(&format!("place.{index}.secondary")));
                    samples
                        .children
                        .push(count(&format!("place.{index}.children")));
                    for source in PLACE_SOURCES {
                        let total = self.sources.entry(source).or_default();
                        *total = total.saturating_add(count(&format!("place.{index}.{source}")));
                    }
                }
            }
            samples
                .memos
                .extend(outcome.stats.get(&format!("memo.{index}.bytes")));
        }
    }

    fn markdown(&self, markdown: &mut String) {
        let _ = writeln!(
            markdown,
            "
Places with at least 3 sections — {} of {} place and compare sites.
",
            self.places_with_three_sections, self.sites,
        );
        markdown.push_str(
            "| Session activity | Count |
| --- | ---: |
",
        );
        for (label, count) in [
            ("Shells", self.shells),
            ("Walls", self.walls),
            ("Robots denials", self.robots_denials),
            ("Renders wanted", self.render_wanted),
            (
                "Anonymous results reads wanted",
                self.anonymous_results_wanted,
            ),
        ] {
            let _ = writeln!(markdown, "| {label} | {count} |");
        }
        markdown.push_str(
            "
| Source | Places |
| --- | ---: |
",
        );
        for source in PLACE_SOURCES {
            let _ = writeln!(
                markdown,
                "| {source} | {} |",
                self.sources.get(source).copied().unwrap_or(0)
            );
        }
        markdown.push_str(
            "
| Budget hit | Queries |
| --- | ---: |
",
        );
        if self.budgets.is_empty() {
            markdown.push_str(
                "| — | 0 |
",
            );
        }
        for (class, count) in &self.budgets {
            let _ = writeln!(markdown, "| {class} | {count} |");
        }
        markdown.push_str(
            "
| Requests per query by phase | Samples | p50 | p90 | Max |
| --- | ---: | ---: | ---: | ---: |
",
        );
        for phase in PHASES {
            let name = format!("{phase:?}");
            if let Some(values) = self.requests_per_query_by_phase.get(&name) {
                metric_row(markdown, &name, values);
            } else {
                metric_row(markdown, &name, &Distribution::default());
            }
        }
        markdown.push_str(
            "
| Passage queries with listed pages | Count |
| --- | ---: |
",
        );
        for class in [
            "answered_from_listed_source",
            "answered_wrong",
            "nothing_returned",
            "page_could_not_be_read",
        ] {
            let _ = writeln!(
                markdown,
                "| {class} | {} |",
                self.passage.get(class).copied().unwrap_or(0)
            );
        }
    }
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
    primary: Vec<u64>,
    secondary: Vec<u64>,
    sections: Vec<u64>,
    children: Vec<u64>,
    memos: Vec<u64>,
    phases: BTreeMap<String, Vec<u64>>,
}

impl Samples {
    fn finish(self, session: bool) -> BTreeMap<&'static str, Distribution> {
        let mut metrics: BTreeMap<_, _> = [
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
        .collect();
        if session {
            metrics.insert(
                "primary_sections_per_place",
                distribution(self.primary, true),
            );
            metrics.insert(
                "secondary_sections_per_place",
                distribution(self.secondary, true),
            );
            metrics.insert("sections_per_place", distribution(self.sections, true));
            metrics.insert("children_per_place", distribution(self.children, true));
            metrics.insert("memo_bytes", distribution(self.memos, false));
        }
        metrics
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
                matched_relaxed: 0,
                probe: ProbeTotals::default(),
                synthetic_results: 0,
                session: None,
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
                    FetchRefusal::RobotsDenied,
                ])
                .map(|class| (class.to_string(), 0))
                .collect(),
        }
    }
}

impl Accumulator {
    fn new(engine: EngineKind) -> Self {
        let mut accumulator = Self::default();
        accumulator.summary.session = (engine == EngineKind::Session).then(SessionTotals::default);
        accumulator
    }

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
        if let Some(session) = &mut self.summary.session {
            session.collect(label, &result, &mut self.samples);
        }
        let mut engine_failure = None;
        if let Some(outcome) = result.outcome {
            if replay
                && self.summary.session.is_some()
                && (outcome.stats.contains_key("cancelled") || outcome.needs_results_page)
            {
                let bytes = PHASES
                    .iter()
                    .filter_map(|phase| outcome.stats.get(&format!("phase.{phase:?}.bytes")))
                    .fold(0u64, |total, bytes| total.saturating_add(*bytes));
                self.samples.decoded_bytes.push(bytes);
            }
            self.summary.synthetic_results += usize::from(outcome.synthetic_results);
            engine_failure = outcome.failure;
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
            self.summary.matched_relaxed = self
                .summary
                .matched_relaxed
                .saturating_add(result.activity.matched_relaxed);
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
            engine_failure,
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
        if let Some(session) = &mut self.summary.session {
            session.requests_per_query_by_phase = std::mem::take(&mut self.samples.phases)
                .into_iter()
                .map(|(phase, samples)| (phase, distribution(samples, true)))
                .collect();
        }
        self.summary.metrics = self.samples.finish(self.summary.session.is_some());
        if self.summary.synthetic_results != 0
            && self.summary.metrics["first_layer_ns"].samples == 0
        {
            self.summary.metrics.remove("first_layer_ns");
        }
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
    engine: EngineKind,
    recording: Summary,
    latest_replay: Option<Summary>,
}

impl Report {
    fn read(root: &Path) -> Result<Self> {
        let manifest = RunManifest::open(root)?;
        let mut recording = Accumulator::new(manifest.engine);
        for query in &manifest.queries {
            let path = query_dir(root, &query.id, false)?;
            if !store::directory(&path, false)? {
                continue;
            }
            let outcome = optional_json(&path.join(OUTCOME))?;
            let measure = optional_json(&path.join(MEASURE))?;
            if let (Some(outcome), Some(measure)) = (outcome, measure) {
                recording.push(query, outcome, Some(measure), false);
            } else {
                recording.push(
                    query,
                    QueryResult::unavailable(
                        Verdict::NotRecorded,
                        Reason::RecordingMissing,
                        Default::default(),
                    ),
                    None,
                    false,
                );
            }
        }
        let engine = manifest.engine;
        let latest_replay = latest_replay(root)?
            .map(|(path, manifest)| -> Result<_> {
                let mut replay = Accumulator::new(engine);
                for query in &manifest.queries {
                    let path = query_dir(&path, &query.id, false)?;
                    let result = read_json(&path.join(OUTCOME))?;
                    if let Some(measure) = optional_json::<ReplayMeasure>(&path.join(MEASURE))? {
                        replay.samples.wall.push(ns(measure.wall));
                        replay.samples.cpu.extend(measure.cpu_ns);
                        replay.samples.memory.extend(measure.peak_rss_delta_bytes);
                    }
                    replay.push(query, result, None, true);
                }
                Ok(replay.finish())
            })
            .transpose()?;
        Ok(Self {
            engine,
            recording: recording.finish(),
            latest_replay,
        })
    }

    fn markdown(&self) -> String {
        let mut markdown = String::from("# Search map benchmark\n\n");
        if self.recording.synthetic_results != 0
            || self
                .latest_replay
                .as_ref()
                .is_some_and(|replay| replay.synthetic_results != 0)
        {
            markdown.push_str("This run used synthetic results pages. Group checks and first-layer latency were not measured for those pages.\n\n");
        }
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
                "Unrecorded requests: {}\n\nmatched_relaxed: {}\n",
                self.unrecorded_requests, self.matched_relaxed
            );
        }
        markdown.push_str("| Scope |");
        for verdict in Verdict::ALL {
            let _ = write!(markdown, " {} |", verdict.label());
        }
        markdown.push_str("\n| --- |");
        for _ in Verdict::ALL {
            markdown.push_str(" ---: |");
        }
        markdown.push('\n');
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
                    "first_layer_ns"
                        | "first_place_ns"
                        | "full_ns"
                        | "requests"
                        | "decoded_bytes"
                        | "wall_ns"
                        | "peak_rss_delta_bytes"
                        | "cpu_ns"
                        | "primary_sections_per_place"
                        | "secondary_sections_per_place"
                        | "sections_per_place"
                        | "children_per_place"
                        | "memo_bytes"
                )
            {
                continue;
            }
            if replay && matches!(*metric, "wall_ns" | "cpu_ns" | "peak_rss_delta_bytes") {
                metric_row(markdown, &format!("replay_{metric}"), values);
            } else {
                metric_row(markdown, metric, values);
            }
        }
        markdown.push_str("\n| Refusal class | Count |\n| --- | ---: |\n");
        if self.refusals.is_empty() {
            markdown.push_str("| — | 0 |\n");
        }
        for (class, count) in &self.refusals {
            let _ = writeln!(markdown, "| {class} | {count} |");
        }
        if let Some(session) = &self.session {
            session.markdown(markdown);
        } else {
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
        }
        let locale = self
            .queries
            .iter()
            .filter(|q| {
                q.score
                    .destination
                    .as_ref()
                    .is_some_and(|d| d.matched_locale)
            })
            .count();
        let _ = writeln!(
            markdown,
            "\nDestinations scored as matched_locale — {locale}."
        );
        if self.queries.iter().any(|q| q.score.focus_right.is_some()) {
            markdown.push_str(
                "\n| Section query | Within two levels | Focus right |\n| --- | --- | --- |\n",
            );
            for q in &self.queries {
                if let Some(right) = q.score.focus_right {
                    let _ = writeln!(
                        markdown,
                        "| `{}` | {} | {} |",
                        q.query.id,
                        q.score.destination.is_some(),
                        right
                    );
                }
            }
        }
        markdown.push_str("\n| Query ID | Verdict | Reason |\n| --- | --- | --- |\n");
        for query in &self.queries {
            let score = &query.score;
            if !score.verdict.passed() {
                let reason = score.reason.label();
                let reason = if let (Reason::EngineFault, Some(class)) =
                    (score.reason, query.engine_failure)
                {
                    format!("{reason} / {class:?}")
                } else {
                    reason.into()
                };
                let _ = writeln!(
                    markdown,
                    "| `{}` | {} | {} |",
                    query.query.id,
                    score.verdict.label(),
                    reason
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

fn metric_row(markdown: &mut String, label: &str, values: &Distribution) {
    let number = |value: Option<u64>| value.map_or_else(|| "—".into(), |value| value.to_string());
    let _ = writeln!(
        markdown,
        "| {label} | {} | {} | {} | {} |",
        values.samples,
        number(values.p50),
        number(values.p90),
        number(values.max),
    );
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
