use url::Url;
use zephium_public_fetch::{ReadRefusal, ReaderLimits, StatusRefusal};

use super::corpus::{CorpusQuery, Kind};
use super::engine::{
    BenchIo, Engine, Expectation, FetchRefusal, FetchRequest, FetchResult, Outcome, ProbeFetch,
    ProbeSite, RobotsStop,
};

pub(super) struct ProbeEngine;

impl Engine for ProbeEngine {
    fn run(&mut self, query: &CorpusQuery, io: &mut dyn BenchIo) -> Outcome {
        let mut outcome = Outcome::empty(query.label.kind);
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
            u64::from(io.results_page(query).available()),
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

    let home = fetch(io, site, "/", Expectation::Html);
    result.homepage = (&home).into();
    let home = if home.is_err() && !site.starts_with("www.") {
        let retry = fetch(io, &format!("www.{site}"), "/", Expectation::Html);
        result.www_retry = Some((&retry).into());
        retry
    } else {
        home
    };

    if let Ok(home) = home {
        if let Some(host) = home.final_url.host_str() {
            result.homepage_final_host = Some(host.to_owned());
            result.llms = (&fetch(io, host, "/llms.txt", Expectation::Text)).into();
            result.sitemap = (&fetch(io, host, "/sitemap.xml", Expectation::Xml)).into();
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
        Err(FetchRefusal::Reader(ReadRefusal::Status(StatusRefusal::Server))) => {
            Some(RobotsStop::Server)
        }
        Ok(body) if blocks_all(&body.bytes) => Some(RobotsStop::BlocksAll),
        _ => None,
    }
}

fn blocks_all(bytes: &[u8]) -> bool {
    let mut wildcard = false;
    let mut saw_rule = false;
    for raw in bytes.split(|&byte| byte == b'\n') {
        let Ok(raw) = std::str::from_utf8(raw) else {
            continue;
        };
        let line = raw.trim().trim_start_matches('\u{feff}');
        let field = line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = field.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.eq_ignore_ascii_case("user-agent") {
            if saw_rule {
                wildcard = false;
                saw_rule = false;
            }
            wildcard |= value.trim() == "*";
        } else if key.eq_ignore_ascii_case("allow") || key.eq_ignore_ascii_case("disallow") {
            saw_rule = true;
            if wildcard && line == "Disallow: /" {
                return true;
            }
        }
    }
    false
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

        fn results_page(&mut self, _: &CorpusQuery) -> super::super::engine::ResultsPage {
            super::super::engine::ResultsPage::Unavailable
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
                body("https://docs.example.com/", b"home"),
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
                ("https://www.example.com/".into(), Expectation::Html),
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
        for robots in [
            "User-agent: *\n# Disallow: /",
            "User-agent: *\nDisallow: / # comment",
            "User-agent: specific\nDisallow: /",
            "User-agent: *\nDisallow: /page",
        ] {
            assert!(!blocks_all(robots.as_bytes()));
        }
        assert!(blocks_all(b"User-agent: other\nUser-agent: *\nDisallow: /"));
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
}
