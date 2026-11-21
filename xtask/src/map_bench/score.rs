use serde::{Deserialize, Serialize};

use super::corpus::{CorpusQuery, Group, Kind};
use super::engine::{Activity, Outcome, RobotsStop};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Verdict {
    Pass,
    Partial,
    Fail,
    StayedOutOk,
    StayedOutBroken,
    Blocked,
    NotRecorded,
    NotMeasured,
    NoOfficialSite,
}

impl Verdict {
    pub(super) const ALL: [Self; 9] = [
        Self::Pass,
        Self::Partial,
        Self::Fail,
        Self::StayedOutOk,
        Self::StayedOutBroken,
        Self::Blocked,
        Self::NotRecorded,
        Self::NotMeasured,
        Self::NoOfficialSite,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Partial => "partial",
            Self::Fail => "fail",
            Self::StayedOutOk => "stayed_out_ok",
            Self::StayedOutBroken => "stayed_out_broken",
            Self::Blocked => "blocked",
            Self::NotRecorded => "not_recorded",
            Self::NotMeasured => "not_measured",
            Self::NoOfficialSite => "no_official_site",
        }
    }

    pub(super) fn passed(self) -> bool {
        matches!(self, Self::Pass | Self::StayedOutOk)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Reason {
    Matched,
    StayedOut,
    SideEffects,
    KindMismatch,
    MissingGroups,
    DestinationMissing,
    PassageMissing,
    PassageUnreadable,
    WrongAnswerShown,
    AnswerFromUnlistedSource,
    ReaderBlocked,
    #[serde(alias = "robots_blocks_all")]
    RobotsDenied,
    RobotsServer,
    RobotsTimeout,
    RecordingMissing,
    RequestNotRecorded,
    RequestInterrupted,
    RequestLimit,
    EnginePanic,
    EngineInvalidInput,
    EngineSessionFailed,
    EngineFault,
    InvalidOutcome,
    ArtifactError,
    NeedsResultsPage,
    NoOfficialSite,
}

impl Reason {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::StayedOut => "stayed_out",
            Self::SideEffects => "site_activity_or_content",
            Self::KindMismatch => "kind_mismatch",
            Self::MissingGroups => "missing_groups",
            Self::DestinationMissing => "destination_missing",
            Self::PassageMissing => "passage_missing",
            Self::PassageUnreadable => "passage_unreadable",
            Self::WrongAnswerShown => "wrong_answer_shown",
            Self::AnswerFromUnlistedSource => "answer_from_unlisted_source",
            Self::ReaderBlocked => "reader_blocked",
            Self::RobotsDenied => "robots_denied",
            Self::RobotsServer => "robots_server",
            Self::RobotsTimeout => "robots_timeout",
            Self::RecordingMissing => "recording_missing",
            Self::RequestNotRecorded => "request_not_recorded",
            Self::RequestInterrupted => "request_interrupted",
            Self::RequestLimit => "request_limit",
            Self::EnginePanic => "engine_panic",
            Self::EngineInvalidInput => "engine_invalid_input",
            Self::EngineSessionFailed => "engine_session_failed",
            Self::EngineFault => "engine_fault",
            Self::InvalidOutcome => "invalid_outcome",
            Self::ArtifactError => "artifact_error",
            Self::NeedsResultsPage => "needs_results_page",
            Self::NoOfficialSite => "no_official_site",
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct DestinationMatch {
    pub(super) depth: u8,
    #[serde(default)]
    pub(super) matched_locale: bool,
    pub(super) source: String,
}

#[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
pub(super) struct PassageScore {
    pub(super) answer_from_listed_source: bool,
    pub(super) answer_from_unlisted_source: bool,
    pub(super) wrong_answer_shown: bool,
    pub(super) nothing_returned: bool,
    #[serde(default)]
    pub(super) page_could_not_be_read: bool,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct Score {
    pub(super) verdict: Verdict,
    pub(super) reason: Reason,
    pub(super) kind_matched: bool,
    pub(super) destination: Option<DestinationMatch>,
    #[serde(default)]
    pub(super) focus_right: Option<bool>,
    pub(super) passage: Option<PassageScore>,
    pub(super) missing_groups: Option<Vec<Group>>,
    pub(super) stayed_out: Option<bool>,
}

impl Score {
    pub(super) fn unavailable(verdict: Verdict, reason: Reason) -> Self {
        Self {
            verdict,
            reason,
            kind_matched: false,
            destination: None,
            focus_right: None,
            passage: None,
            missing_groups: None,
            stayed_out: None,
        }
    }
}

pub(super) fn score(query: &CorpusQuery, outcome: &Outcome, activity: &Activity) -> Score {
    if outcome.needs_results_page {
        return Score::unavailable(Verdict::NotMeasured, Reason::NeedsResultsPage);
    }
    let kind_matched = outcome.kind == query.label.kind;
    let missing_groups: Option<Vec<_>> = (!outcome.synthetic_results)
        .then(|| query.group.difference(&outcome.groups).copied().collect());
    let missing_count = missing_groups.as_ref().map_or(0, Vec::len);
    let matched = |entry: &super::engine::Entry| {
        query
            .want
            .iter()
            .filter_map(|w| w.matched_locale(&entry.url))
            .min()
    };
    let destination = matches!(query.label.kind, Kind::Place | Kind::Compare)
        .then(|| {
            outcome
                .entries
                .iter()
                .filter_map(|e| Some((e, matched(e)?)))
                .min_by_key(|(e, locale)| (u8::from(e.depth), *locale))
                .map(|(entry, matched_locale)| DestinationMatch {
                    depth: entry.depth.into(),
                    source: entry.source.clone(),
                    matched_locale,
                })
        })
        .flatten();
    let focus_right = (query.label.kind == Kind::Place
        && !query.want.is_empty()
        && zephium_map::read_query(&query.text).is_ok_and(|q| {
            matches!(
                q.kind(),
                zephium_map::QueryKind::Place {
                    section: Some(_),
                    ..
                }
            )
        }))
    .then(|| {
        outcome
            .entries
            .iter()
            .any(|e| e.focus && matched(e).is_some())
    });
    let passage = query
        .answer
        .as_deref()
        .filter(|_| query.label.kind == Kind::Passage)
        .map(|answer| passages(query, outcome, answer));
    let stayed_out = matches!(query.label.kind, Kind::Instant | Kind::List).then(|| {
        outcome.entries.is_empty() && outcome.passages.is_empty() && activity.site_requests == 0
    });
    let (verdict, reason) = if activity.not_recorded > 0 {
        (Verdict::NotRecorded, Reason::RequestNotRecorded)
    } else if activity.interrupted > 0 {
        (Verdict::NotRecorded, Reason::RequestInterrupted)
    } else if outcome.no_official_site {
        (Verdict::NoOfficialSite, Reason::NoOfficialSite)
    } else if let Some(stayed_out) = stayed_out {
        match (stayed_out, kind_matched, missing_count == 0) {
            (false, _, _) => (Verdict::StayedOutBroken, Reason::SideEffects),
            (true, false, _) => (Verdict::StayedOutBroken, Reason::KindMismatch),
            (true, true, false) => (Verdict::StayedOutBroken, Reason::MissingGroups),
            (true, true, true) => (Verdict::StayedOutOk, Reason::StayedOut),
        }
    } else {
        let main = match &passage {
            Some(passage) if passage.answer_from_listed_source => Ok(()),
            Some(passage) if passage.answer_from_unlisted_source => {
                Err(Reason::AnswerFromUnlistedSource)
            }
            Some(passage) if passage.wrong_answer_shown => Err(Reason::WrongAnswerShown),
            Some(passage) if passage.page_could_not_be_read => Err(Reason::PassageUnreadable),
            Some(_) => Err(Reason::PassageMissing),
            None if destination.is_some() => Ok(()),
            None => Err(Reason::DestinationMissing),
        };
        match main {
            Ok(()) if !kind_matched => (Verdict::Partial, Reason::KindMismatch),
            Ok(()) if missing_count != 0 => (Verdict::Partial, Reason::MissingGroups),
            Ok(()) => (Verdict::Pass, Reason::Matched),
            Err(Reason::AnswerFromUnlistedSource) => {
                (Verdict::Partial, Reason::AnswerFromUnlistedSource)
            }
            Err(reason) => {
                if let Some(block) = blocked(outcome, activity) {
                    (Verdict::Blocked, block)
                } else if missing_groups.is_some() && missing_count < query.group.len() {
                    (Verdict::Partial, reason)
                } else {
                    (Verdict::Fail, reason)
                }
            }
        }
    };
    Score {
        verdict,
        reason,
        kind_matched,
        destination,
        focus_right,
        passage,
        missing_groups,
        stayed_out,
    }
}

fn passages(query: &CorpusQuery, outcome: &Outcome, answer: &str) -> PassageScore {
    let answer = answer.to_lowercase();
    let unreadable = outcome.synthetic_results
        && !query.pages.is_empty()
        && outcome.kind == Kind::Passage
        && outcome.passages.is_empty()
        && outcome
            .stats
            .get("passage_pages_read")
            .copied()
            .unwrap_or(0)
            == 0;
    let mut score = PassageScore {
        nothing_returned: outcome.passages.is_empty() && !unreadable,
        page_could_not_be_read: unreadable,
        ..PassageScore::default()
    };
    for passage in &outcome.passages {
        if !passage.text.to_lowercase().contains(&answer) {
            score.wrong_answer_shown = true;
            continue;
        }
        let domain = passage
            .source
            .domain()
            .and_then(|host| psl::domain_str(host.trim_end_matches('.')));
        if domain.is_some_and(|domain| query.from.iter().any(|source| source == domain))
            || (query.from.is_empty() && super::session::listed_page(query, &passage.source))
        {
            score.answer_from_listed_source = true;
        } else {
            score.answer_from_unlisted_source = true;
        }
    }
    score
}

fn blocked(outcome: &Outcome, activity: &Activity) -> Option<Reason> {
    if outcome.probe.is_empty() {
        if outcome.entries.is_empty()
            && outcome.passages.is_empty()
            && ["Entry", "Passage"].iter().any(|phase| {
                outcome
                    .stats
                    .get(&format!("phase.{phase}.refused.Robots"))
                    .copied()
                    .unwrap_or(0)
                    > 0
            })
        {
            return Some(Reason::RobotsDenied);
        }
        return (outcome.entries.is_empty() && outcome.passages.is_empty() && activity.blocked())
            .then_some(Reason::ReaderBlocked);
    }
    if !outcome.probe.iter().all(|site| site.blocked()) {
        return None;
    }
    Some(
        match outcome.probe.first().and_then(|site| site.robots_stop) {
            Some(RobotsStop::BlocksAll) => Reason::RobotsDenied,
            Some(RobotsStop::Server) => Reason::RobotsServer,
            Some(RobotsStop::Timeout) => Reason::RobotsTimeout,
            Some(RobotsStop::Interrupted) | None => Reason::ReaderBlocked,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_bench::corpus::fixture;
    use crate::map_bench::engine::{Depth, Entry, Passage};
    use url::Url;

    fn query(fields: &str) -> CorpusQuery {
        fixture(&format!("[[query]]\nid='q'\ntext='private text'\n{fields}"))
    }

    fn place() -> CorpusQuery {
        query("kind='place'\nsite='example.com'\nwant=['www.example.com/docs','any.example.org']")
    }

    fn entry(url: &str, depth: Depth, source: &str) -> Entry {
        Entry {
            url: Url::parse(url).unwrap(),
            depth,
            source: source.into(),
            focus: false,
            tags: Default::default(),
        }
    }

    #[test]
    fn destination_matches_host_and_segment_boundary_then_prefers_depth_one() {
        for (url, matches) in [
            ("https://example.com/docs", true),
            ("https://www.example.com/docs/foo", true),
            ("https://example.com/docs?query=x", true),
            ("https://www.example.com/docs-more", false),
            ("https://example.com/docs%2Ffoo", false),
            ("https://www.example.com.evil.org/docs", false),
            ("https://docs.example.com/docs", false),
            ("https://www.any.example.org/anything", true),
        ] {
            let mut outcome = Outcome::empty(Kind::Place);
            outcome.entries.push(entry(url, Depth::Two, "deep"));
            assert_eq!(
                score(&place(), &outcome, &Activity::default())
                    .destination
                    .is_some(),
                matches,
                "{url}"
            );
        }
        let mut outcome = Outcome::empty(Kind::Place);
        outcome.entries = vec![
            entry("https://example.com/docs", Depth::Two, "deep"),
            entry("https://example.com/docs/manual", Depth::One, "nav"),
        ];
        assert_eq!(
            score(&place(), &outcome, &Activity::default()).destination,
            Some(DestinationMatch {
                depth: 1,
                matched_locale: false,
                source: "nav".into()
            }),
        );
    }

    #[test]
    fn locale_added_by_site_and_focus_are_independent_scores() {
        let q = fixture("[[query]]\nid='a'\ntext='Example docs'\nkind='place'\nsite='example.com'\nwant=['example.com/docs']");
        for (path, expected) in [
            ("en/docs", Some(true)),
            ("en-pl/docs/start", Some(true)),
            ("zh-Hant/docs", Some(true)),
            ("es-419/docs", Some(true)),
            ("docs", Some(false)),
            ("eng/docs", None),
            ("en-x/docs", None),
            ("en-abcde/docs", None),
            ("en/gb/docs", None),
            ("es-4_9/docs", None),
        ] {
            let mut outcome = Outcome::empty(Kind::Place);
            outcome.entries.push(entry(
                &format!("https://example.com/{path}"),
                Depth::Two,
                "footer",
            ));
            let s = score(&q, &outcome, &Activity::default());
            assert_eq!(s.destination.map(|d| d.matched_locale), expected);
            assert_eq!(s.focus_right, Some(false));
            outcome.entries[0].focus = true;
            assert_eq!(
                score(&q, &outcome, &Activity::default()).focus_right,
                Some(expected.is_some())
            );
        }
    }

    #[test]
    fn passage_subcases_include_registrable_domains_and_mixed_results() {
        let query = query("kind='passage'\nanswer='The Answer'\nfrom=['example.co.uk']");
        for (input, expected, verdict) in [
            (vec![], (false, false, false, true), Verdict::Fail),
            (
                vec![("wrong", "https://blog.example.co.uk/")],
                (false, false, true, false),
                Verdict::Fail,
            ),
            (
                vec![("THE ANSWER", "https://example.co.uk.evil.com/")],
                (false, true, false, false),
                Verdict::Partial,
            ),
            (
                vec![("The ANswer", "https://blog.example.co.uk/")],
                (true, false, false, false),
                Verdict::Pass,
            ),
            (
                vec![
                    ("the answer", "https://elsewhere.org/"),
                    ("wrong", "https://example.co.uk/"),
                    ("the answer", "https://example.co.uk/"),
                ],
                (true, true, true, false),
                Verdict::Pass,
            ),
        ] {
            let mut outcome = Outcome::empty(Kind::Passage);
            outcome.passages = input
                .into_iter()
                .map(|(text, url)| Passage {
                    text: text.into(),
                    source: Url::parse(url).unwrap(),
                })
                .collect();
            let result = score(&query, &outcome, &Activity::default());
            assert_eq!(result.verdict, verdict);
            let passage = result.passage.unwrap();
            assert_eq!(
                (
                    passage.answer_from_listed_source,
                    passage.answer_from_unlisted_source,
                    passage.wrong_answer_shown,
                    passage.nothing_returned
                ),
                expected
            );
        }
    }

    #[test]
    fn overall_verdicts_cover_every_class() {
        use crate::map_bench::engine::FetchRefusal;
        use zephium_public_fetch::{ReadRefusal, StatusRefusal};

        let mut denied = Activity::default();
        denied.called(&Err(FetchRefusal::Reader(ReadRefusal::Status(
            StatusRefusal::Denied,
        ))));
        let mut missing = Activity::default();
        missing.called(&Err(FetchRefusal::NotRecorded));
        let mut interrupted = Activity::default();
        interrupted.called(&Err(FetchRefusal::Interrupted));
        let list = || query("kind='list'");
        let mut with_destination = Outcome::empty(Kind::Place);
        with_destination
            .entries
            .push(entry("https://example.com/docs", Depth::One, "nav"));
        let groups =
            query("kind='place'\nsite='example.com'\nwant=['example.com/docs']\ngroup=['video']");
        for (query, outcome, activity, verdict, reason) in [
            (
                place(),
                &with_destination,
                Activity::default(),
                Verdict::Pass,
                Reason::Matched,
            ),
            (
                groups,
                &with_destination,
                Activity::default(),
                Verdict::Partial,
                Reason::MissingGroups,
            ),
            (
                place(),
                &Outcome::empty(Kind::Place),
                Activity::default(),
                Verdict::Fail,
                Reason::DestinationMissing,
            ),
            (
                list(),
                &Outcome::empty(Kind::List),
                Activity::default(),
                Verdict::StayedOutOk,
                Reason::StayedOut,
            ),
            (
                list(),
                &Outcome::empty(Kind::List),
                denied.clone(),
                Verdict::StayedOutBroken,
                Reason::SideEffects,
            ),
            (
                list(),
                &Outcome::empty(Kind::Instant),
                Activity::default(),
                Verdict::StayedOutBroken,
                Reason::KindMismatch,
            ),
            (
                place(),
                &Outcome::empty(Kind::Place),
                denied,
                Verdict::Blocked,
                Reason::ReaderBlocked,
            ),
            (
                place(),
                &Outcome::empty(Kind::Place),
                missing,
                Verdict::NotRecorded,
                Reason::RequestNotRecorded,
            ),
            (
                place(),
                &Outcome::empty(Kind::Place),
                interrupted,
                Verdict::NotRecorded,
                Reason::RequestInterrupted,
            ),
        ] {
            let result = score(&query, outcome, &activity);
            assert_eq!((result.verdict, result.reason), (verdict, reason));
        }
    }
}
