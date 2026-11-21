use std::collections::{BTreeMap, BTreeSet, VecDeque};

use url::Url;
use zephium_core::search::SearchEngine;
use zephium_map::session::{
    EndOutcome, Expect, Failure, FetchResult as SessionFetch, Id, Phase, Stats, MAX_CONCURRENT,
    MAX_HOST_CONCURRENT, MAX_MEMO_BYTES,
};
use zephium_map::{
    read_query, Config, Doc, DocLimits, Effect, FirstLayer, Input, MapEvent, Place, QueryKind,
    Session, SiteKey, Source,
};
use zephium_public_fetch::{PublicBody, ReadRefusal, StatusRefusal};

use super::corpus::{CorpusQuery, Group, Kind};
use super::engine::{
    BenchIo, Depth, Engine, EngineFailure, Entry, Expectation, FetchRefusal, FetchRequest, Outcome,
    Passage, ResultsPage,
};

pub(super) const MAX_PLACES: usize = 2;
const PLACEHOLDER: &str = "https://stay-out.invalid/";

pub(super) struct SessionEngine;

impl Engine for SessionEngine {
    fn run(&mut self, query: &CorpusQuery, io: &mut dyn BenchIo) -> Outcome {
        let mut driver = Driver::new(query);
        if query.label.kind == Kind::Passage && query.pages.is_empty() {
            driver.outcome.needs_results_page = true;
            return driver.outcome;
        }
        let config = Config {
            session_id: io.session_id(),
            engine: SearchEngine::Google,
            now: io.now(),
            ..Config::default()
        };
        let Ok((session, effects)) = Session::start(config, &query.text) else {
            driver.outcome.failure = Some(EngineFailure::InvalidInput);
            return driver.outcome;
        };
        driver.run(session, effects, query, io)
    }
}

struct Driver {
    outcome: Outcome,
    places: [Option<Place>; MAX_PLACES],
    lookups: usize,
    memos: usize,
    terminal: bool,
    immediate: VecDeque<Input>,
    in_flight: BTreeMap<usize, (Id, Url, Expectation)>,
}

impl Driver {
    fn new(query: &CorpusQuery) -> Self {
        Self {
            outcome: Outcome::empty(query.label.kind),
            places: [None, None],
            lookups: 0,
            memos: 0,
            terminal: false,
            immediate: VecDeque::new(),
            in_flight: BTreeMap::new(),
        }
    }

    fn run(
        mut self,
        mut session: Session,
        mut effects: Vec<Effect>,
        query: &CorpusQuery,
        io: &mut dyn BenchIo,
    ) -> Outcome {
        loop {
            for effect in effects {
                self.effect(effect, query, io);
                if self
                    .outcome
                    .failure
                    .is_some_and(|failure| failure != EngineFailure::SessionFailed)
                {
                    break;
                }
            }
            if self.terminal || self.outcome.failure.is_some() {
                break;
            }
            let input = self
                .immediate
                .pop_front()
                .map(|input| (input, false))
                .or_else(|| self.completion(query, io));
            let Some((input, listed)) = input else {
                self.outcome
                    .failure
                    .get_or_insert(EngineFailure::MissingTerminal);
                break;
            };
            effects = session.handle(Input::Elapsed { time: io.elapsed() });
            if !effects.iter().any(is_terminal) {
                let before = listed.then(|| session.accepted_passage_pages());
                effects.extend(session.handle(input));
                if before.is_some_and(|count| session.accepted_passage_pages() > count) {
                    *self
                        .outcome
                        .stats
                        .entry("passage_pages_read".into())
                        .or_default() += 1;
                }
            }
        }
        let expected = match query.label.kind {
            Kind::Place => 1,
            Kind::Compare => 2,
            _ => 0,
        };
        self.outcome.no_official_site = self.outcome.synthetic_results
            && self.terminal
            && self.lookups < expected
            && (self.outcome.failure.is_none()
                || (self.outcome.failure == Some(EngineFailure::SessionFailed)
                    && ["terminal_failure.Unreadable", "terminal_failure.NoResults"]
                        .iter()
                        .any(|key| self.outcome.stats.contains_key(*key))));
        self.outcome
            .stats
            .insert("official_sites".into(), self.lookups as u64);
        for (index, place) in self.places.into_iter().enumerate() {
            if let Some(place) = place {
                flatten_place(&mut self.outcome, index, place);
            }
        }
        self.outcome
    }

    fn completion(&mut self, query: &CorpusQuery, io: &mut dyn BenchIo) -> Option<(Input, bool)> {
        if self.in_flight.is_empty() {
            return None;
        }
        let Some(((_, result), (id, url, expect))) = io.next_fetch().and_then(|item| {
            self.in_flight
                .remove(&item.0)
                .map(|pending| (item, pending))
        }) else {
            self.outcome.failure = Some(EngineFailure::UnexpectedCompletion);
            return None;
        };
        let listed = expect == Expectation::Html
            && listed_page(query, &url)
            && result.as_ref().is_ok_and(|body| {
                !body.truncated && SiteKey::from_url(&url) == SiteKey::from_url(&body.final_url)
            });
        Some((
            Input::Fetch {
                id,
                result: result
                    .map(|body| session_body(body, expect))
                    .map_err(failure),
            },
            listed,
        ))
    }

    fn effect(&mut self, effect: Effect, query: &CorpusQuery, io: &mut dyn BenchIo) {
        if self.terminal {
            self.outcome.failure = Some(EngineFailure::AfterTerminal);
            return;
        }
        match effect {
            Effect::LoadResults { id, url } => {
                let result = match io.results_page(Some((&url, synthetic_html(query)))) {
                    ResultsPage::Available {
                        url,
                        bytes,
                        synthetic,
                    } => {
                        self.outcome.synthetic_results = synthetic;
                        Doc::from_html(&url, &bytes, None, DocLimits::RESULTS)
                            .map_err(|_| Failure::Unreadable)
                    }
                    ResultsPage::Unavailable => Err(Failure::Unavailable),
                };
                self.immediate.push_back(Input::Results { id, result });
            }
            Effect::Fetch {
                id,
                url,
                expect,
                max_bytes,
                timeout,
            } => {
                if matches!(query.label.kind, Kind::Instant | Kind::List)
                    || url.as_str() == PLACEHOLDER
                {
                    self.outcome.failure = Some(EngineFailure::StayOutFetch);
                    return;
                }
                let host = url.host_str();
                if self.in_flight.len() >= MAX_CONCURRENT
                    || self
                        .in_flight
                        .values()
                        .filter(|(_, active, _)| active.host_str() == host)
                        .count()
                        >= MAX_HOST_CONCURRENT
                {
                    self.outcome.failure = Some(EngineFailure::Concurrency);
                    return;
                }
                let expect = match expect {
                    Expect::Html => Expectation::Html,
                    Expect::Text => Expectation::Text,
                    Expect::Xml => Expectation::Xml,
                };
                let token = io.start_fetch(FetchRequest {
                    url: url.clone(),
                    expect,
                    max_bytes: max_bytes as usize,
                    timeout,
                });
                if self.in_flight.insert(token, (id, url, expect)).is_some() {
                    self.outcome.failure = Some(EngineFailure::UnexpectedCompletion);
                }
            }
            Effect::Render { id, .. } => {
                *self
                    .outcome
                    .stats
                    .entry("render_wanted".into())
                    .or_default() += 1;
                self.immediate.push_back(Input::Render {
                    id,
                    result: Err(Failure::Unavailable),
                });
            }
            Effect::ReadResultsAnonymously { id, .. } => {
                *self
                    .outcome
                    .stats
                    .entry("anonymous_results_wanted".into())
                    .or_default() += 1;
                self.immediate.push_back(Input::Results {
                    id,
                    result: Err(Failure::Unavailable),
                });
            }
            Effect::LookupSite { id, .. } => {
                self.lookups += 1;
                self.immediate.push_back(Input::Cache { id, memo: None });
            }
            Effect::RememberSite { memo, .. } => {
                if self.memos >= MAX_PLACES || memo.len() > MAX_MEMO_BYTES {
                    self.outcome.failure = Some(EngineFailure::MemoLimit);
                    return;
                }
                io.remember(self.memos, &memo);
                self.outcome
                    .stats
                    .insert(format!("memo.{}.bytes", self.memos), memo.len() as u64);
                self.memos += 1;
            }
            Effect::Emit { event, .. } => self.emit(event, io),
        }
    }

    fn emit(&mut self, event: MapEvent, io: &dyn BenchIo) {
        match event {
            MapEvent::Kind { kind } => self.outcome.kind = Kind::from(&kind),
            MapEvent::FirstLayer { layer } if !self.outcome.synthetic_results => {
                self.outcome
                    .milestones
                    .first_layer
                    .get_or_insert(io.elapsed());
                self.outcome.groups = groups(&layer);
            }
            MapEvent::Place { slot, place, .. } => {
                self.outcome
                    .milestones
                    .first_place
                    .get_or_insert(io.elapsed());
                if let Some(previous) = self.places.get_mut(usize::from(slot)) {
                    *previous = Some(place);
                } else {
                    self.outcome.failure = Some(EngineFailure::UnexpectedPlace);
                }
            }
            MapEvent::Passages { passages } => {
                self.outcome.passages = passages
                    .into_iter()
                    .map(|passage| Passage {
                        text: passage.text,
                        source: passage.source.as_url().clone(),
                    })
                    .collect();
            }
            MapEvent::End { outcome, stats } => {
                self.terminal = true;
                self.outcome.milestones.full = Some(io.elapsed());
                flatten_stats(&mut self.outcome, stats);
                if let EndOutcome::Failed { failure } = outcome {
                    self.outcome
                        .stats
                        .insert(format!("terminal_failure.{failure:?}"), 1);
                    self.outcome.failure = Some(EngineFailure::SessionFailed);
                }
            }
            MapEvent::Compare { .. }
            | MapEvent::PlaceFailed { .. }
            | MapEvent::FirstLayer { .. } => {}
        }
    }
}

fn is_terminal(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::Emit {
            event: MapEvent::End { .. },
            ..
        }
    )
}

fn groups(layer: &FirstLayer) -> BTreeSet<Group> {
    [
        (
            Group::Discussion,
            layer.organic.iter().any(|r| r.discussion),
        ),
        (Group::Video, !layer.video.is_empty()),
        (Group::Social, !layer.social.is_empty()),
        (Group::Images, !layer.images.is_empty()),
        (Group::Featured, layer.featured.is_some()),
    ]
    .into_iter()
    .filter_map(|(group, present)| present.then_some(group))
    .collect()
}

fn synthetic_html(query: &CorpusQuery) -> Vec<u8> {
    use std::fmt::Write as _;

    fn escape(text: &str) -> String {
        let mut escaped = String::new();
        for ch in text.chars() {
            match ch {
                '&' => escaped.push_str("&amp;"),
                '<' => escaped.push_str("&lt;"),
                '>' => escaped.push_str("&gt;"),
                '"' => escaped.push_str("&quot;"),
                _ => escaped.push(ch),
            }
        }
        escaped
    }

    fn original_terms(text: &str, terms: &[String]) -> String {
        let mut words = text.split_whitespace();
        terms
            .iter()
            .filter_map(|term| words.find(|word| word.to_lowercase() == *term))
            .collect::<Vec<_>>()
            .join(" ")
    }

    let mut html = String::from("<main>");
    let mut organic = |url: &str, title: &str| {
        let _ = write!(
            html,
            "<article><h2><a href=\"{}\">{}</a></h2></article>",
            escape(url),
            escape(title)
        );
    };
    let read = read_query(&query.text).ok();
    match query.label.kind {
        Kind::Place => {
            if let Some(site) = &query.site {
                let title = match read.as_ref().map(|read| read.kind()) {
                    Some(QueryKind::Place { entity, .. }) if !entity.is_empty() => {
                        original_terms(&query.text, entity)
                    }
                    _ => query.text.clone(),
                };
                organic(&format!("https://{site}/"), &title);
            }
        }
        Kind::Compare => {
            let titles = match read.as_ref().map(|read| read.kind()) {
                Some(QueryKind::Compare { left, right, .. }) => [
                    original_terms(&query.text, left),
                    original_terms(&query.text, right),
                ],
                _ => query
                    .text
                    .split_whitespace()
                    .position(|word| {
                        matches!(
                            word.to_ascii_lowercase().as_str(),
                            "vs" | "vs." | "versus" | "or"
                        )
                    })
                    .map_or_else(
                        || [query.text.clone(), query.text.clone()],
                        |split| {
                            let mut words = query.text.split_whitespace();
                            let first = words.by_ref().take(split).collect::<Vec<_>>().join(" ");
                            [first, words.skip(1).collect::<Vec<_>>().join(" ")]
                        },
                    ),
            };
            for (site, title) in query.sites.iter().zip(titles) {
                organic(&format!("https://{site}/"), &title);
            }
        }
        Kind::Passage => {
            for page in &query.pages {
                organic(page.as_str(), "Result");
            }
        }
        Kind::Instant | Kind::List => organic(PLACEHOLDER, "Placeholder"),
    }
    html.push_str("</main>");
    html.into_bytes()
}

fn session_body(body: PublicBody, expect: Expectation) -> SessionFetch {
    let default_media = match expect {
        Expectation::Html => "text/html",
        Expectation::Xml => "application/xml",
        Expectation::Text => "text/plain",
    };
    SessionFetch {
        final_url: body.final_url,
        content_type: match body.media_type.as_deref() {
            Some("application/gzip" | "application/x-gzip") if expect == Expectation::Xml => {
                default_media.into()
            }
            _ => body.media_type.unwrap_or_else(|| default_media.into()),
        },
        charset: body.charset,
        bytes: body.bytes,
        truncated: body.truncated,
    }
}

fn failure(refusal: FetchRefusal) -> Failure {
    use ReadRefusal as R;
    use StatusRefusal as S;
    match refusal {
        FetchRefusal::Reader(
            R::NotPublicHttps | R::RedirectRefused | R::TooManyRedirects | R::DnsRefused,
        ) => Failure::External,
        FetchRefusal::Reader(
            R::ProxyUnsupported | R::RouteChanged | R::Transport | R::Status(S::Denied),
        )
        | FetchRefusal::NotRecorded
        | FetchRefusal::Interrupted
        | FetchRefusal::Storage => Failure::Unavailable,
        FetchRefusal::RobotsDenied => Failure::Robots,
        FetchRefusal::Reader(R::Timeout(_)) => Failure::Timeout,
        FetchRefusal::Reader(R::Status(S::Client)) => Failure::Client,
        FetchRefusal::Reader(R::Status(S::RateLimited | S::Server)) => Failure::Server,
        FetchRefusal::Reader(R::Status(S::Challenge)) => Failure::Challenge,
        FetchRefusal::Reader(R::Status(S::Unexpected) | R::Encoding) => Failure::Unreadable,
        FetchRefusal::Reader(R::WrongContentType) => Failure::ContentType,
        FetchRefusal::Reader(R::TooLarge(_)) | FetchRefusal::Limit => Failure::Limit,
    }
}

pub(super) const PHASES: [Phase; 9] = [
    Phase::Results,
    Phase::AnonymousResults,
    Phase::Robots,
    Phase::Entry,
    Phase::Render,
    Phase::Section,
    Phase::Llms,
    Phase::Sitemap,
    Phase::Passage,
];

fn flatten_stats(outcome: &mut Outcome, stats: Stats) {
    let mut put = |key: &str, value| {
        outcome.stats.insert(key.into(), value);
    };
    put("shells", stats.shells.into());
    put("walls", stats.walls.into());
    put("cancelled", stats.cancelled.into());
    put("fresh_memos", stats.fresh_memos.into());
    put("stale_memos", stats.stale_memos.into());
    put("rejected_memos", stats.rejected_memos.into());
    for (index, notes) in stats.places {
        for (key, value) in [
            ("robots_truncated", notes.robots_truncated),
            ("entry_truncated", notes.entry_truncated),
            ("sections_truncated", notes.sections_truncated),
            ("section_page_truncated", notes.section_page_truncated),
            ("llms_truncated", notes.llms_truncated),
            ("sitemap_truncated", notes.sitemap_truncated),
            ("sitelinks_truncated", notes.sitelinks_truncated),
            ("linked_data_truncated", notes.linked_data_truncated),
        ] {
            put(&format!("place.{index}.{key}"), u64::from(value));
        }
        for (key, value) in [
            ("links_seen", notes.links_seen),
            ("sitemap_urls", notes.sitemap_urls),
            ("section_pages", notes.section_pages),
        ] {
            put(&format!("place.{index}.{key}"), u64::from(value));
        }
    }
    for (phase, counts) in stats.phases {
        put(&format!("phase.{phase:?}.started"), counts.started.into());
        put(
            &format!("phase.{phase:?}.completed"),
            counts.completed.into(),
        );
        put(&format!("phase.{phase:?}.bytes"), counts.bytes);
        for (failure, count) in counts.refused {
            put(
                &format!("phase.{phase:?}.refused.{failure:?}"),
                count.into(),
            );
        }
    }
    for (outcome, count) in stats.robots {
        put(&format!("robots.{outcome:?}"), count.into());
    }
    for budget in stats.budgets {
        put(&format!("budget.{budget:?}"), 1);
    }
    for source in stats.sources {
        put(&format!("source.{}", source_tag(source)), 1);
    }
}

pub(super) const PLACE_SOURCES: [&str; 6] = [
    "navigation",
    "footer",
    "llms_txt",
    "sitemap",
    "linked_data",
    "section_pages",
];

fn source_tag(source: Source) -> &'static str {
    match source {
        Source::PrimaryNav => "primary_nav",
        Source::MenuGroup => "menu_group",
        Source::LinkCluster => "link_cluster",
        Source::Footer => "footer",
        Source::EngineResult => "engine_result",
        Source::EngineSitelink => "engine_sitelink",
        Source::SectionSidebar => "section_sidebar",
        Source::Heading => "heading",
        Source::Llms => "llms_txt",
        Source::Sitemap => "sitemap",
        Source::LinkedData => "linked_data",
    }
}

fn flatten_place(outcome: &mut Outcome, index: usize, place: Place) {
    let mut put = |key, value| {
        outcome.stats.insert(format!("place.{index}.{key}"), value);
    };
    let notes = place.notes;
    for (key, value) in [
        ("shell", notes.shell),
        ("wall", notes.wall),
        ("robots_disallowed", notes.robots_disallowed),
    ] {
        put(key, u64::from(value));
    }
    let primary = place
        .sections
        .iter()
        .filter(|e| e.tier == zephium_map::Tier::Primary)
        .count();
    put("primary", primary as u64);
    put("secondary", (place.sections.len() - primary) as u64);
    put("sections", place.sections.len() as u64);
    put(
        "children",
        place
            .sections
            .iter()
            .map(|section| section.children.len() as u64)
            .sum(),
    );
    let mut sources = BTreeSet::new();
    for section in place.sections {
        let children = section.children.iter().map(|child| (child, Depth::Two));
        for (entry, depth) in std::iter::once((&section, Depth::One)).chain(children) {
            sources.extend(
                entry
                    .sources
                    .iter()
                    .filter(|source| **source != Source::Heading || depth == Depth::Two),
            );
            let mut url = entry.key.as_url().clone();
            if let Some(fragment) = entry
                .locator
                .fragment()
                .filter(|_| url.fragment().is_none())
            {
                url.set_fragment(fragment.strip_prefix('#'));
            }
            outcome.entries.push(Entry {
                url,
                depth,
                focus: place.focus.as_ref() == Some(&entry.id),
                source: entry
                    .sources
                    .first()
                    .copied()
                    .map(source_tag)
                    .unwrap_or_default()
                    .into(),
                tags: entry
                    .sources
                    .iter()
                    .map(|&source| source_tag(source).into())
                    .chain(std::iter::once(
                        match entry.tier {
                            zephium_map::Tier::Primary => "primary",
                            zephium_map::Tier::Secondary => "secondary",
                        }
                        .into(),
                    ))
                    .collect(),
            });
        }
    }
    for source in sources {
        let class = match source {
            Source::PrimaryNav | Source::MenuGroup => "navigation",
            Source::SectionSidebar | Source::Heading => "section_pages",
            source => source_tag(source),
        };
        put(class, 1);
    }
}

pub(super) fn listed_page(query: &CorpusQuery, url: &Url) -> bool {
    let Ok(actual) = zephium_map::PageKey::from_url(url) else {
        return false;
    };
    query
        .pages
        .iter()
        .any(|page| zephium_map::PageKey::from_url(page).is_ok_and(|key| key == actual))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_bench::engine::FetchResult;
    use std::time::Duration;

    #[test]
    fn every_reader_refusal_maps_once() {
        use Failure as F;
        let expected = [
            F::External,
            F::External,
            F::External,
            F::External,
            F::Unavailable,
            F::Unavailable,
            F::Timeout,
            F::Timeout,
            F::Timeout,
            F::Timeout,
            F::Unavailable,
            F::Server,
            F::Challenge,
            F::Client,
            F::Server,
            F::Unreadable,
            F::ContentType,
            F::Limit,
            F::Limit,
            F::Unreadable,
            F::Unavailable,
        ];
        assert_eq!(
            ReadRefusal::ALL.map(|class| failure(FetchRefusal::Reader(class))),
            expected
        );
        assert_eq!(failure(FetchRefusal::NotRecorded), F::Unavailable);
        assert_eq!(failure(FetchRefusal::Interrupted), F::Unavailable);
        assert_eq!(failure(FetchRefusal::Storage), F::Unavailable);
        assert_eq!(failure(FetchRefusal::Limit), F::Limit);
    }

    #[derive(Default)]
    struct EffectsOnly {
        fetches: usize,
        memos: usize,
    }

    impl BenchIo for EffectsOnly {
        fn fetch_checked(
            &mut self,
            _: FetchRequest,
            _: &mut super::super::engine::LandingAdmission<'_>,
        ) -> FetchResult {
            panic!("expected asynchronous fetch");
        }

        fn start_fetch(&mut self, _: FetchRequest) -> usize {
            self.fetches += 1;
            self.fetches
        }

        fn next_fetch(&mut self) -> Option<(usize, FetchResult)> {
            None
        }

        fn results_page(&mut self, _: Option<(&Url, Vec<u8>)>) -> ResultsPage {
            ResultsPage::Unavailable
        }

        fn remember(&mut self, _: usize, _: &[u8]) {
            self.memos += 1;
        }

        fn now(&self) -> u64 {
            0
        }

        fn session_id(&self) -> u64 {
            0
        }

        fn elapsed(&self) -> Duration {
            Duration::from_secs(1)
        }
    }

    const ID: Id = Id {
        session: 0,
        sequence: 0,
    };

    fn place_query() -> CorpusQuery {
        super::super::corpus::fixture(
            "[[query]]\nid='a'\nkind='place'\ntext='Example docs'\nsite='example.com'",
        )
    }

    #[test]
    fn engine_faults_cover_concurrency_memos_and_effects_after_terminal() {
        let query = place_query();
        for (limit, same_host) in [(MAX_CONCURRENT, false), (MAX_HOST_CONCURRENT, true)] {
            let mut driver = Driver::new(&query);
            let mut io = EffectsOnly::default();
            for index in 0..=limit {
                let host = if same_host { 0 } else { index };
                driver.effect(
                    Effect::Fetch {
                        id: ID,
                        url: Url::parse(&format!("https://site-{host}.example.com/")).unwrap(),
                        expect: Expect::Html,
                        max_bytes: 1,
                        timeout: Duration::from_secs(1),
                    },
                    &query,
                    &mut io,
                );
            }
            assert_eq!(io.fetches, limit);
            assert_eq!(driver.outcome.failure, Some(EngineFailure::Concurrency));
        }
        let mut driver = Driver::new(&query);
        let mut io = EffectsOnly::default();
        let site =
            zephium_map::PlaceKey::from_url(&Url::parse("https://example.com").unwrap()).unwrap();
        let memo = |len| Effect::RememberSite {
            id: ID,
            memo: vec![0; len],
            site: site.clone(),
        };
        driver.effect(memo(MAX_MEMO_BYTES + 1), &query, &mut io);
        assert_eq!(driver.outcome.failure, Some(EngineFailure::MemoLimit));
        assert_eq!(io.memos, 0);
        let mut driver = Driver::new(&query);
        for _ in 0..=MAX_PLACES {
            driver.effect(memo(1), &query, &mut io);
        }
        assert_eq!(driver.outcome.failure, Some(EngineFailure::MemoLimit));
        assert_eq!(io.memos, MAX_PLACES);

        for effect in [
            Effect::Emit {
                id: ID,
                event: MapEvent::End {
                    outcome: EndOutcome::Done,
                    stats: Stats::default(),
                },
            },
            Effect::Render {
                id: ID,
                url: Url::parse("https://example.com").unwrap(),
            },
        ] {
            let mut driver = Driver::new(&query);
            driver.effect(
                Effect::Emit {
                    id: ID,
                    event: MapEvent::End {
                        outcome: EndOutcome::Done,
                        stats: Stats::default(),
                    },
                },
                &query,
                &mut io,
            );
            driver.effect(effect, &query, &mut io);
            assert_eq!(driver.outcome.failure, Some(EngineFailure::AfterTerminal));
            assert_eq!(driver.outcome.milestones.full, Some(io.elapsed()));
        }
    }
}
