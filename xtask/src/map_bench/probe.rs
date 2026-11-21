use std::collections::HashMap;
use url::{Origin, Position, Url};
use zephium_map::doc::MAX_CHARSET_LABEL_BYTES;
use zephium_map::session::MAX_MEDIA_TYPE_BYTES;
use zephium_map::{robots::Robots, PageKey, SiteKey};
use zephium_public_fetch::guard::public_target;
use zephium_public_fetch::{ReadRefusal, ReaderLimits, StatusRefusal};

use super::corpus::{CorpusQuery, Kind};
use super::engine::{
    BenchIo, Engine, EngineFailure, Expectation, FetchRefusal, FetchRequest, FetchResult, Outcome,
    ProbeFetch, ProbeSite, RobotsStop,
};

pub(super) struct ProbeEngine;

impl Engine for ProbeEngine {
    fn run(&mut self, query: &CorpusQuery, io: &mut dyn BenchIo) -> Outcome {
        let mut outcome = Outcome::empty(query.label.kind);
        let Ok(measured) = zephium_map::read_query(&query.text) else {
            outcome.failure = Some(EngineFailure::InvalidInput);
            return outcome;
        };
        outcome.kind = Kind::from(measured.kind());
        let sites = match query.label.kind {
            Kind::Place => query.site.as_slice(),
            Kind::Compare => &query.sites,
            _ => &[],
        };
        for site in sites {
            if let Ok(site) = Url::parse(&format!("https://{site}/")) {
                outcome.probe.push(probe(io, &site));
            }
        }
        outcome.stats.insert(
            "results_page_available".into(),
            u64::from(io.results_page(None).available()),
        );
        outcome.stats.insert(
            "robots_blocks_all".into(),
            outcome
                .probe
                .iter()
                .filter(|site| site.robots_stop == Some(RobotsStop::BlocksAll))
                .count() as u64,
        );
        outcome.milestones.full = Some(io.elapsed());
        outcome
    }
}

fn probe(io: &mut dyn BenchIo, site: &Url) -> ProbeSite {
    let robots = fetch_robots(io, site);
    let mut result = ProbeSite {
        robots: (&robots).into(),
        homepage: ProbeFetch::Skipped,
        www_retry: None,
        llms: ProbeFetch::Skipped,
        sitemap: ProbeFetch::Skipped,
        homepage_final_host: None,
        robots_stop: robots_stop(&robots),
    };
    if result.robots_stop.is_some() {
        return result;
    }
    let initial = policy(site, &robots);
    if !initial.allows("/") {
        result.robots_stop = Some(RobotsStop::BlocksAll);
        return result;
    }
    let mut policies = HashMap::from([(site.origin(), initial)]);

    let home = fetch_page(io, site.clone(), Expectation::Html, &mut policies);
    result.homepage = (&home).into();
    let retry_host = site.host_str().filter(|host| {
        !host.starts_with("www.")
            && home
                .as_ref()
                .is_err_and(|class| *class != FetchRefusal::RobotsDenied)
    });
    let home = if let Some(host) = retry_host {
        let mut www = site.clone();
        if www.set_host(Some(&format!("www.{host}"))).is_err() {
            return result;
        }
        let robots = fetch_robots(io, &www);
        if let Some(stop) = robots_stop(&robots) {
            result.robots_stop = Some(stop);
            return result;
        }
        let www_policy = policy(&www, &robots);
        if !www_policy.allows("/") {
            result.robots_stop = Some(RobotsStop::BlocksAll);
            return result;
        }
        policies.insert(www.origin(), www_policy);
        let retry = fetch_page(io, www, Expectation::Html, &mut policies);
        result.www_retry = Some((&retry).into());
        retry
    } else {
        home
    };

    let landing = match home {
        Ok(home) => home.final_url,
        Err(FetchRefusal::RobotsDenied) => {
            result.robots_stop = Some(RobotsStop::BlocksAll);
            return result;
        }
        Err(_) => return result,
    };
    result.homepage_final_host = landing.host_str().map(str::to_owned);
    for (path, expect, slot) in [
        ("/llms.txt", Expectation::Text, &mut result.llms),
        ("/sitemap.xml", Expectation::Xml, &mut result.sitemap),
    ] {
        if policies
            .get(&landing.origin())
            .is_some_and(|p| p.allows(path))
        {
            let page = fetch_page(io, at_path(&landing, path), expect, &mut policies);
            *slot = (&page).into();
            if page == Err(FetchRefusal::RobotsDenied) {
                result.robots_stop = Some(RobotsStop::BlocksAll);
                break;
            }
        }
    }
    result
}

fn at_path(origin: &Url, path: &str) -> Url {
    let mut url = origin.clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

fn request(url: Url, expect: Expectation) -> FetchRequest {
    let limits = ReaderLimits::default();
    FetchRequest {
        url,
        expect,
        max_bytes: limits.max_decoded_bytes,
        timeout: limits.max_duration,
    }
}

fn fetch_robots(io: &mut dyn BenchIo, origin: &Url) -> FetchResult {
    io.fetch(request(at_path(origin, "/robots.txt"), Expectation::Text))
}

fn fetch_page(
    io: &mut dyn BenchIo,
    url: Url,
    expect: Expectation,
    policies: &mut HashMap<Origin, Robots>,
) -> FetchResult {
    io.fetch_checked(request(url, expect), &mut |io, landing| {
        if !public_target(landing) {
            return false;
        }
        let policy = policies
            .entry(landing.origin())
            .or_insert_with(|| policy(landing, &fetch_robots(io, landing)));
        policy.allows(&landing[Position::BeforePath..Position::AfterQuery])
    })
}

fn robots_stop(result: &FetchResult) -> Option<RobotsStop> {
    match result {
        Err(FetchRefusal::Interrupted) => Some(RobotsStop::Interrupted),
        Err(FetchRefusal::Reader(ReadRefusal::Timeout(_))) => Some(RobotsStop::Timeout),
        Err(FetchRefusal::Reader(ReadRefusal::Status(
            StatusRefusal::Server | StatusRefusal::RateLimited,
        ))) => Some(RobotsStop::Server),
        _ => None,
    }
}

/// The session engine's robots admission: `Session::receive` for `Work::Robots`
/// in `zephium-map/src/session/scheduler.rs`, fed by `failure` and `session_body`
/// in `map_bench/session.rs`.
fn policy(origin: &Url, result: &FetchResult) -> Robots {
    let body = match result {
        Ok(body) => body,
        Err(FetchRefusal::Reader(ReadRefusal::Status(StatusRefusal::Client))) => {
            return Robots::unavailable(true)
        }
        Err(_) => return Robots::unavailable(false),
    };
    let media = body
        .media_type
        .as_deref()
        .unwrap_or("text/plain")
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let site = SiteKey::from_url(origin);
    if body
        .media_type
        .as_ref()
        .is_some_and(|m| m.len() > MAX_MEDIA_TYPE_BYTES)
        || body
            .charset
            .as_ref()
            .is_some_and(|c| c.len() > MAX_CHARSET_LABEL_BYTES)
        || !matches!(media.as_str(), "text/plain" | "text/markdown")
        || PageKey::from_url(&body.final_url).is_err()
        || site.is_none()
        || SiteKey::from_url(&body.final_url) != site
        || !body.final_url.path().ends_with("/robots.txt")
    {
        return Robots::unavailable(false);
    }
    Robots::from_sample(&body.bytes, body.truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_bench::corpus::fixture;
    use crate::map_bench::engine::LandingAdmission;
    use std::collections::VecDeque;
    use std::time::Duration;
    use zephium_public_fetch::{PublicBody, StatusClass, TimeoutPhase, Timings};

    struct FakeIo {
        results: VecDeque<FetchResult>,
        calls: Vec<(String, Expectation)>,
    }

    impl BenchIo for FakeIo {
        fn fetch_checked(
            &mut self,
            request: FetchRequest,
            admit: &mut LandingAdmission<'_>,
        ) -> FetchResult {
            self.calls.push((request.url.to_string(), request.expect));
            let body = self.results.pop_front().expect("unexpected request")?;
            if admit(self, &body.final_url) {
                Ok(body)
            } else {
                Err(FetchRefusal::RobotsDenied)
            }
        }

        fn results_page(
            &mut self,
            _: Option<(&Url, Vec<u8>)>,
        ) -> super::super::engine::ResultsPage {
            super::super::engine::ResultsPage::Unavailable
        }

        fn start_fetch(&mut self, _: FetchRequest) -> usize {
            panic!("probe must remain sequential")
        }

        fn next_fetch(&mut self) -> Option<(usize, FetchResult)> {
            None
        }

        fn remember(&mut self, _: usize, _: &[u8]) {
            panic!("probe does not store memos")
        }

        fn session_id(&self) -> u64 {
            0
        }

        fn now(&self) -> u64 {
            0
        }

        fn elapsed(&self) -> Duration {
            Duration::from_millis(self.calls.len() as u64)
        }
    }

    fn body(url: &str, bytes: &[u8]) -> FetchResult {
        Ok(PublicBody {
            final_url: Url::parse(url).unwrap(),
            hops: 0,
            status: StatusClass::Ok,
            media_type: None,
            charset: None,
            bytes: bytes.to_vec(),
            truncated: false,
            wire_bytes: bytes.len() as u64,
            timings: Timings::default(),
        })
    }

    #[test]
    fn probe_retries_and_uses_final_host_in_request_order() {
        let query = fixture("[[query]]\nid='q'\ntext='q'\nkind='place'\nsite='example.com'");
        let mut io = FakeIo {
            calls: vec![],
            results: [
                Err(FetchRefusal::Reader(ReadRefusal::Status(
                    StatusRefusal::Client,
                ))),
                Err(FetchRefusal::Reader(ReadRefusal::Transport)),
                Err(FetchRefusal::Reader(ReadRefusal::Status(
                    StatusRefusal::Client,
                ))),
                body("https://docs.example.com/", b"home"),
                body(
                    "https://docs.example.com/robots.txt",
                    b"User-agent: *\nDisallow: /private",
                ),
                body("https://docs.example.com/llms.txt", b"llms"),
                body("https://docs.example.com/sitemap.xml", b"xml"),
            ]
            .into(),
        };
        let outcome = ProbeEngine.run(&query, &mut io);
        assert!(outcome.entries.is_empty() && outcome.passages.is_empty());
        assert_eq!(
            outcome.probe[0].homepage_final_host.as_deref(),
            Some("docs.example.com")
        );
        assert_eq!(
            io.calls,
            [
                ("https://example.com/robots.txt".into(), Expectation::Text),
                ("https://example.com/".into(), Expectation::Html),
                (
                    "https://www.example.com/robots.txt".into(),
                    Expectation::Text
                ),
                ("https://www.example.com/".into(), Expectation::Html),
                (
                    "https://docs.example.com/robots.txt".into(),
                    Expectation::Text
                ),
                (
                    "https://docs.example.com/llms.txt".into(),
                    Expectation::Text
                ),
                (
                    "https://docs.example.com/sitemap.xml".into(),
                    Expectation::Xml
                ),
            ]
        );
    }

    #[test]
    fn robot_stops_are_per_site_and_rules_must_be_exact() {
        for (robots, expected) in [
            (
                body(
                    "https://a.com/robots.txt",
                    b"User-agent: *\r\nDisallow: /\r\n",
                ),
                RobotsStop::BlocksAll,
            ),
            (
                Err(FetchRefusal::Reader(ReadRefusal::Status(
                    StatusRefusal::Server,
                ))),
                RobotsStop::Server,
            ),
            (
                Err(FetchRefusal::Reader(ReadRefusal::Timeout(
                    TimeoutPhase::Body,
                ))),
                RobotsStop::Timeout,
            ),
        ] {
            let query =
                fixture("[[query]]\nid='q'\ntext='q'\nkind='compare'\nsites=['a.com','b.com']");
            let mut io = FakeIo {
                calls: vec![],
                results: [
                    robots,
                    body(
                        "https://b.com/robots.txt",
                        b"User-agent: *\nDisallow: /internal\nUser-agent: another\nDisallow: /",
                    ),
                    body("https://b.com/", b"home"),
                    body("https://b.com/llms.txt", b"llms"),
                    body("https://b.com/sitemap.xml", b"xml"),
                ]
                .into(),
            };
            let outcome = ProbeEngine.run(&query, &mut io);
            assert_eq!(outcome.probe[0].robots_stop, Some(expected));
            assert_eq!(outcome.probe[1].robots_stop, None);
            assert_eq!(io.calls.len(), 5);
            assert_eq!(io.calls[1].0, "https://b.com/robots.txt");
        }
        for (robots, home) in [
            ("User-agent: *\n# Disallow: /", true),
            ("User-agent: *\nDisallow: / # comment", false),
            ("User-agent: specific\nDisallow: /", true),
            ("User-agent: *\nDisallow: /page", true),
            ("User-agent: other\nUser-agent: *\nDisallow: /", false),
            ("User-agent: *\nDisallow: /\nAllow: /$", true),
            ("<html><body>Not found</body></html>", false),
        ] {
            let policy = policy(
                &Url::parse("https://a.com/").unwrap(),
                &body("https://a.com/robots.txt", robots.as_bytes()),
            );
            assert_eq!(policy.allows("/"), home, "{robots:?}");
        }
    }

    #[test]
    fn robots_refusals_follow_the_session_engine() {
        let mut html = body("https://a.com/robots.txt", b"User-agent: *\nAllow: /");
        if let Ok(body) = &mut html {
            body.media_type = Some("text/html".into());
        }
        let refused = |r: ReadRefusal| Err(FetchRefusal::Reader(r));
        let labelled = |media: usize, charset: usize| {
            let mut robots = body("https://a.com/robots.txt", b"User-agent: *\nAllow: /");
            if let Ok(body) = &mut robots {
                body.media_type = Some(format!("text/plain{}", " ".repeat(media - 10)));
                body.charset = Some("u".repeat(charset));
            }
            robots
        };
        for (robots, allowed) in [
            (
                labelled(MAX_MEDIA_TYPE_BYTES, MAX_CHARSET_LABEL_BYTES),
                true,
            ),
            (labelled(MAX_MEDIA_TYPE_BYTES + 1, 1), false),
            (labelled(10, MAX_CHARSET_LABEL_BYTES + 1), false),
            (refused(ReadRefusal::Status(StatusRefusal::Client)), true),
            (
                body("https://a.com/robots.txt", b"User-agent: *\nAllow: /"),
                true,
            ),
            (
                body("https://www.a.com/robots.txt", b"User-agent: *\nAllow: /"),
                true,
            ),
            (refused(ReadRefusal::Transport), false),
            (refused(ReadRefusal::WrongContentType), false),
            (refused(ReadRefusal::Status(StatusRefusal::Denied)), false),
            (
                refused(ReadRefusal::Status(StatusRefusal::Unexpected)),
                false,
            ),
            (refused(ReadRefusal::Encoding), false),
            (
                refused(ReadRefusal::TooLarge(zephium_public_fetch::SizeLimit::Wire)),
                false,
            ),
            (refused(ReadRefusal::RedirectRefused), false),
            (Err(FetchRefusal::Limit), false),
            (html, false),
            (
                body("https://other.com/robots.txt", b"User-agent: *\nAllow: /"),
                false,
            ),
            (
                body("https://a.com/consent", b"User-agent: *\nAllow: /"),
                false,
            ),
        ] {
            assert_eq!(
                policy(&Url::parse("https://a.com/").unwrap(), &robots).allows("/"),
                allowed,
                "{robots:?}"
            );
        }
        let query = fixture("[[query]]\nid='q'\ntext='q'\nkind='place'\nsite='a.com'");
        let mut io = FakeIo {
            calls: vec![],
            results: [Err(FetchRefusal::Reader(ReadRefusal::Transport))].into(),
        };
        let outcome = ProbeEngine.run(&query, &mut io);
        assert_eq!(outcome.probe[0].robots_stop, Some(RobotsStop::BlocksAll));
        assert_eq!(io.calls.len(), 1);
        let mut io = FakeIo {
            calls: vec![],
            results: [Err(FetchRefusal::Reader(ReadRefusal::Status(
                StatusRefusal::RateLimited,
            )))]
            .into(),
        };
        let outcome = ProbeEngine.run(&query, &mut io);
        assert_eq!(outcome.probe[0].robots_stop, Some(RobotsStop::Server));
    }

    #[test]
    fn each_requested_path_and_each_new_origin_is_checked_against_robots() {
        let place = |site: &str| {
            fixture(&format!(
                "[[query]]\nid='q'\ntext='q'\nkind='place'\nsite='{site}'"
            ))
        };
        let mut io = FakeIo {
            calls: vec![],
            results: [
                body(
                    "https://a.com/robots.txt",
                    b"User-agent: *\nDisallow: /\nAllow: /$",
                ),
                body("https://a.com/", b"home"),
            ]
            .into(),
        };
        let outcome = ProbeEngine.run(&place("a.com"), &mut io);
        assert_eq!(outcome.probe[0].robots_stop, None);
        assert!(outcome.probe[0].homepage.answered());
        assert_eq!(outcome.probe[0].llms, ProbeFetch::Skipped);
        assert_eq!(outcome.probe[0].sitemap, ProbeFetch::Skipped);
        assert_eq!(io.calls.len(), 2);
        let mut io = FakeIo {
            calls: vec![],
            results: [
                body("https://a.com/robots.txt", b"User-agent: *\nAllow: /"),
                Err(FetchRefusal::Reader(ReadRefusal::Transport)),
                body(
                    "https://www.a.com/robots.txt",
                    b"User-agent: *\nDisallow: /",
                ),
            ]
            .into(),
        };
        let outcome = ProbeEngine.run(&place("a.com"), &mut io);
        assert_eq!(outcome.probe[0].robots_stop, Some(RobotsStop::BlocksAll));
        assert_eq!(outcome.probe[0].www_retry, None);
        assert_eq!(io.calls.last().unwrap().0, "https://www.a.com/robots.txt");
        assert!(io.results.is_empty());
        let mut io = FakeIo {
            calls: vec![],
            results: [
                body("https://a.com/robots.txt", b"User-agent: *\nAllow: /"),
                body("https://docs.a.com/", b"home"),
                body(
                    "https://docs.a.com/robots.txt",
                    b"User-agent: *\nDisallow: /llms.txt",
                ),
                body("https://docs.a.com/sitemap.xml", b"xml"),
            ]
            .into(),
        };
        let outcome = ProbeEngine.run(&place("a.com"), &mut io);
        assert_eq!(outcome.probe[0].llms, ProbeFetch::Skipped);
        assert!(outcome.probe[0].sitemap.answered());
        assert_eq!(io.calls[2].0, "https://docs.a.com/robots.txt");
        assert_eq!(io.calls[3].0, "https://docs.a.com/sitemap.xml");
    }

    #[test]
    fn redirected_homepage_rejects_a_denied_path() {
        let query = fixture("[[query]]\nid='q'\ntext='q'\nkind='place'\nsite='example.com'");
        let mut io = FakeIo {
            calls: vec![],
            results: [
                body(
                    "https://example.com/robots.txt",
                    b"User-agent: *\nDisallow: /private",
                ),
                body("https://example.com/private", b"denied"),
                body("https://example.com/llms.txt", b"llms"),
                body("https://example.com/sitemap.xml", b"sitemap"),
            ]
            .into(),
        };
        let outcome = ProbeEngine.run(&query, &mut io);
        let site = &outcome.probe[0];
        assert_eq!(site.robots_stop, Some(RobotsStop::BlocksAll));
        assert!(!site.homepage.answered());
        assert!(site.homepage_final_host.is_none());
        assert_eq!(io.calls.len(), 2);
    }

    #[test]
    fn redirected_homepage_rejects_a_denied_or_unknown_new_origin() {
        let query = fixture("[[query]]\nid='q'\ntext='q'\nkind='place'\nsite='example.com'");
        for robots in [
            body(
                "https://docs.example.com/robots.txt",
                b"User-agent: *\nDisallow: /private",
            ),
            Err(FetchRefusal::Reader(ReadRefusal::Transport)),
        ] {
            let mut io = FakeIo {
                calls: vec![],
                results: [
                    body("https://example.com/robots.txt", b"User-agent: *\nAllow: /"),
                    body("https://docs.example.com/private", b"denied"),
                    robots,
                    body("https://docs.example.com/llms.txt", b"llms"),
                    body("https://docs.example.com/sitemap.xml", b"sitemap"),
                ]
                .into(),
            };
            let outcome = ProbeEngine.run(&query, &mut io);
            let site = &outcome.probe[0];
            assert_eq!(site.robots_stop, Some(RobotsStop::BlocksAll));
            assert!(!site.homepage.answered());
            assert!(site.homepage_final_host.is_none());
            assert_eq!(io.calls.len(), 3);
            assert_eq!(io.calls[2].0, "https://docs.example.com/robots.txt");
        }
    }

    #[test]
    fn other_kinds_stay_out() {
        for fields in [
            "kind='list'",
            "kind='instant'",
            "kind='passage'\nanswer='text'",
        ] {
            let query = fixture(&format!("[[query]]\nid='q'\ntext='q'\n{fields}"));
            let mut io = FakeIo {
                calls: vec![],
                results: VecDeque::new(),
            };
            assert!(ProbeEngine.run(&query, &mut io).probe.is_empty());
            assert!(io.calls.is_empty());
        }
    }

    #[test]
    fn probe_measures_kind_independently_of_the_corpus_label() {
        let query = crate::map_bench::corpus::fixture(
            "[[query]]\nid='q'\ntext='Example docs'\nkind='instant'",
        );
        let mut io = FakeIo {
            calls: vec![],
            results: VecDeque::new(),
        };
        let outcome = ProbeEngine.run(&query, &mut io);
        assert_eq!(outcome.kind, Kind::Place);
        assert!(outcome.probe.is_empty());
    }
}
