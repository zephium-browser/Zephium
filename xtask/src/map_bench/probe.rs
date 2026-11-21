use url::Url;
use zephium_map::doc::MAX_CHARSET_LABEL_BYTES;
use zephium_map::session::MAX_MEDIA_TYPE_BYTES;
use zephium_map::{robots::Robots, PageKey, SiteKey};
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
            outcome.probe.push(probe(io, site));
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

fn probe(io: &mut dyn BenchIo, site: &str) -> ProbeSite {
    let robots = fetch(io, site, "/robots.txt", Expectation::Text);
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
    let mut policies = vec![(site.to_owned(), policy(site, &robots))];
    if !policies[0].1.allows("/") {
        result.robots_stop = Some(RobotsStop::BlocksAll);
        return result;
    }

    let home = fetch(io, site, "/", Expectation::Html);
    result.homepage = (&home).into();
    let home = if home.is_err() && !site.starts_with("www.") {
        let www = format!("www.{site}");
        let robots = fetch(io, &www, "/robots.txt", Expectation::Text);
        if let Some(stop) = robots_stop(&robots) {
            result.robots_stop = Some(stop);
            return result;
        }
        let www_policy = policy(&www, &robots);
        if !www_policy.allows("/") {
            result.robots_stop = Some(RobotsStop::BlocksAll);
            return result;
        }
        policies.push((www.clone(), www_policy));
        let retry = fetch(io, &www, "/", Expectation::Html);
        result.www_retry = Some((&retry).into());
        retry
    } else {
        home
    };

    if let Ok(home) = home {
        if let Some(host) = home.final_url.host_str() {
            result.homepage_final_host = Some(host.to_owned());
            let known = policies
                .into_iter()
                .find_map(|(origin, policy)| (origin == host).then_some(policy));
            let landed = known.or_else(|| {
                let robots = fetch(io, host, "/robots.txt", Expectation::Text);
                robots_stop(&robots)
                    .is_none()
                    .then(|| policy(host, &robots))
            });
            if let Some(landed) = landed {
                if landed.allows("/llms.txt") {
                    result.llms = (&fetch(io, host, "/llms.txt", Expectation::Text)).into();
                }
                if landed.allows("/sitemap.xml") {
                    result.sitemap = (&fetch(io, host, "/sitemap.xml", Expectation::Xml)).into();
                }
            }
        }
    }
    result
}

fn fetch(io: &mut dyn BenchIo, host: &str, path: &str, expect: Expectation) -> FetchResult {
    let url = Url::parse(&format!("https://{host}{path}"))
        .map_err(|_| FetchRefusal::Reader(ReadRefusal::NotPublicHttps))?;
    let limits = ReaderLimits::default();
    io.fetch(FetchRequest {
        url,
        expect,
        max_bytes: limits.max_decoded_bytes,
        timeout: limits.max_duration,
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
fn policy(host: &str, result: &FetchResult) -> Robots {
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
    let site = Url::parse(&format!("https://{host}/"))
        .ok()
        .and_then(|url| SiteKey::from_url(&url));
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
    use std::collections::VecDeque;
    use std::time::Duration;
    use zephium_public_fetch::{PublicBody, StatusClass, TimeoutPhase, Timings};

    struct FakeIo {
        results: VecDeque<FetchResult>,
        calls: Vec<(String, Expectation)>,
    }

    impl BenchIo for FakeIo {
        fn fetch(&mut self, request: FetchRequest) -> FetchResult {
            self.calls.push((request.url.to_string(), request.expect));
            self.results.pop_front().expect("unexpected request")
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
                "a.com",
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
            assert_eq!(policy("a.com", &robots).allows("/"), allowed, "{robots:?}");
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
