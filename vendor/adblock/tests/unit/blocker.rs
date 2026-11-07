#[cfg(test)]
mod blocker_tests {

    use super::super::*;
    use crate::request::Request;
    use crate::resources::{Resource, ResourceStorage};
    use base64::{engine::Engine as _, prelude::BASE64_STANDARD};
    use std::collections::HashSet;
    use std::iter::FromIterator;

    #[test]
    fn single_slash() {
        let filters = ["/|"];
        let blocker = Blocker::new(filters);

        let request = Request::new(
            "https://example.com/test/",
            "https://example.com",
            "xmlhttprequest",
            "",
        )
        .unwrap();
        assert!(blocker.check(&request, &Default::default()).should_block());

        let request = Request::new(
            "https://example.com/test",
            "https://example.com",
            "xmlhttprequest",
            "",
        )
        .unwrap();
        assert!(!blocker.check(&request, &Default::default()).should_block());
    }

    fn test_requests_filters(
        filters: impl IntoIterator<Item = impl AsRef<str>>,
        requests: &[(Request, bool)],
    ) {
        let blocker = Blocker::new_debug(filters);

        requests.iter().for_each(|(req, expected_result)| {
            let matched_rule = blocker.check(req, &Default::default());
            if *expected_result {
                assert!(
                    matched_rule.should_block(),
                    "Expected match for {}",
                    req.url
                );
            } else {
                assert!(
                    !matched_rule.should_block(),
                    "Expected no match for {}, matched with {:?}",
                    req.url,
                    matched_rule.filter
                );
            }
        });
    }

    #[test]
    fn source_independent_scan_does_not_charge_skipped_attribution_filter() {
        let blocker = Blocker::new(["@@*$script,domain=publisher.example"]);
        let request =
            Request::new_source_independent("https://cdn.example/ad.js", "script", "get").unwrap();
        let mut regex_manager = blocker.borrow_regex_manager();
        let mut remaining_checks = 1;
        assert!(!blocker.exceptions().is_empty());

        assert!(
            blocker
                .exceptions()
                .check_with_attribution_bounded(
                    &request,
                    get_no_tags(),
                    &mut regex_manager,
                    true,
                    &mut remaining_checks,
                )
                .expect("an inapplicable candidate cannot exhaust the evaluation budget")
                .is_none(),
        );
        assert_eq!(
            remaining_checks, 1,
            "the budget measures evaluated candidates, not structurally skipped entries",
        );
    }

    #[test]
    fn redirect_blocking_exception() {
        let filters = [
            "||imdb-video.media-imdb.com$media,redirect=noop-0.1s.mp3",
            "@@||imdb-video.media-imdb.com^$domain=imdb.com",
        ];

        let request = Request::new(
            "https://imdb-video.media-imdb.com/kBOeI88k1o23eNAi",
            "https://www.imdb.com/video/13",
            "media",
            "",
        )
        .unwrap();
        let blocker = Blocker::new_debug(filters);
        let resources = ResourceStorage::in_memory_from_resources([Resource::simple(
            "noop-0.1s.mp3",
            crate::resources::MimeType::AudioMp3,
            "mp3",
        )]);

        let matched_rule = blocker.check(&request, &resources);
        assert!(!matched_rule.should_block());
        assert!(!matched_rule.important);
        assert_eq!(
            matched_rule.redirect,
            Some("data:audio/mp3;base64,bXAz".to_string())
        );
        assert_eq!(
            matched_rule.exception.map(|f| f.to_string()),
            Some("0:1: @@||imdb-video.media-imdb.com^$domain=imdb.com".to_string())
        );
    }

    #[test]
    fn redirect_exception() {
        let filters = [
            "||imdb-video.media-imdb.com$media,redirect=noop-0.1s.mp3",
            "@@||imdb-video.media-imdb.com^$domain=imdb.com,redirect=noop-0.1s.mp3",
        ];

        let request = Request::new(
            "https://imdb-video.media-imdb.com/kBOeI88k1o23eNAi",
            "https://www.imdb.com/video/13",
            "media",
            "",
        )
        .unwrap();
        let blocker = Blocker::new_debug(filters);
        let resources = ResourceStorage::in_memory_from_resources([Resource::simple(
            "noop-0.1s.mp3",
            crate::resources::MimeType::AudioMp3,
            "mp3",
        )]);

        let matched_rule = blocker.check(&request, &resources);
        assert!(!matched_rule.should_block());
        assert!(!matched_rule.important);
        assert_eq!(matched_rule.redirect, None);
        assert_eq!(
            matched_rule.exception.map(|f| f.to_string()),
            Some(
                "0:1: @@||imdb-video.media-imdb.com^$domain=imdb.com,redirect=noop-0.1s.mp3"
                    .to_string()
            )
        );
    }

    #[test]
    fn redirect_rule_redirection() {
        let filters = [
            "||doubleclick.net^",
            "||www3.doubleclick.net^$xmlhttprequest,redirect-rule=noop.txt,domain=lineups.fun",
        ];

        let request = Request::new(
            "https://www3.doubleclick.net",
            "https://lineups.fun",
            "xhr",
            "",
        )
        .unwrap();
        let blocker = Blocker::new_debug(filters);
        let resources = ResourceStorage::in_memory_from_resources([Resource::simple(
            "noop.txt",
            crate::resources::MimeType::TextPlain,
            "noop",
        )]);

        let matched_rule = blocker.check(&request, &resources);
        assert!(matched_rule.should_block());
        assert!(!matched_rule.important);
        assert_eq!(
            matched_rule.redirect,
            Some("data:text/plain;base64,bm9vcA==".to_string())
        );
        assert_eq!(matched_rule.exception, None);
    }

    #[test]
    fn badfilter_does_not_match() {
        let filters = ["||foo.com$badfilter"];
        let url_results = [(
            Request::new("https://foo.com", "https://bar.com", "image", "").unwrap(),
            false,
        )];

        let request_expectations: Vec<_> = url_results.into_iter().collect();

        test_requests_filters(filters, &request_expectations);
    }

    #[test]
    fn badfilter_cancels_with_same_id() {
        let filters = [
            "||foo.com$domain=bar.com|foo.com,badfilter",
            "||foo.com$domain=foo.com|bar.com",
        ];
        let url_results = [(
            Request::new("https://foo.com", "https://bar.com", "image", "").unwrap(),
            false,
        )];

        let request_expectations: Vec<_> = url_results.into_iter().collect();

        test_requests_filters(filters, &request_expectations);
    }

    #[test]
    fn badfilter_does_not_cancel_similar_filter() {
        let filters = [
            "||foo.com$domain=bar.com|foo.com,badfilter",
            "||foo.com$domain=foo.com|bar.com,image",
        ];
        let url_results = [(
            Request::new("https://foo.com", "https://bar.com", "image", "").unwrap(),
            true,
        )];

        let request_expectations: Vec<_> = url_results.into_iter().collect();

        test_requests_filters(filters, &request_expectations);
    }

    #[test]
    fn trailing_dot_domain() {
        let filters = ["||dot.example.com.^", "||test.example.com^"];
        let blocker = Blocker::new(filters);

        let request = Request::new(
            "https://dot.example.com",
            "https://dot.example.com",
            "document",
            "",
        )
        .unwrap();
        assert!(!blocker.check(&request, &Default::default()).should_block());

        let request = Request::new(
            "https://dot.example.com.",
            "https://dot.example.com.",
            "document",
            "",
        )
        .unwrap();
        assert!(blocker.check(&request, &Default::default()).should_block());

        let request = Request::new(
            "https://test.example.com",
            "https://test.example.com",
            "document",
            "",
        )
        .unwrap();
        assert!(blocker.check(&request, &Default::default()).should_block());

        let request = Request::new(
            "https://test.example.com.",
            "https://test.example.com.",
            "document",
            "",
        )
        .unwrap();
        assert!(blocker.check(&request, &Default::default()).should_block());
    }

    #[test]
    fn hostname_regex_filter_works() {
        let filters = [
            "||alimc*.top^$domain=letv.com",
            "||aa*.top^$domain=letv.com",
        ];
        let url_results = [
            (
                Request::new(
                    "https://r.alimc1.top/test.js",
                    "https://minisite.letv.com/",
                    "script",
                    "",
                )
                .unwrap(),
                true,
            ),
            (
                Request::new(
                    "https://www.baidu.com/test.js",
                    "https://minisite.letv.com/",
                    "script",
                    "",
                )
                .unwrap(),
                false,
            ),
            (
                Request::new(
                    "https://r.aabb.top/test.js",
                    "https://example.com/",
                    "script",
                    "",
                )
                .unwrap(),
                false,
            ),
            (
                Request::new(
                    "https://r.aabb.top/test.js",
                    "https://minisite.letv.com/",
                    "script",
                    "",
                )
                .unwrap(),
                true,
            ),
        ];

        let blocker = Blocker::new_debug(filters);
        let resources = ResourceStorage::default();

        url_results.into_iter().for_each(|(req, expected_result)| {
            let matched_rule = blocker.check(&req, &resources);
            if expected_result {
                assert!(
                    matched_rule.should_block(),
                    "Expected match for {}",
                    req.url
                );
            } else {
                assert!(
                    !matched_rule.should_block(),
                    "Expected no match for {}, matched with {:?}",
                    req.url,
                    matched_rule.filter
                );
            }
        });
    }

    #[test]
    fn get_csp_directives() {
        let filters = [
            "$csp=script-src 'self' * 'unsafe-inline',domain=thepiratebay.vip|pirateproxy.live|thehiddenbay.com|downloadpirate.com|thepiratebay10.org|kickass.vip|pirateproxy.app|ukpass.co|prox.icu|pirateproxy.life",
            "$csp=worker-src 'none',domain=pirateproxy.live|thehiddenbay.com|tpb.party|thepiratebay.org|thepiratebay.vip|thepiratebay10.org|flashx.cc|vidoza.co|vidoza.net",
            "||1337x.to^$csp=script-src 'self' 'unsafe-inline'",
            "@@^no-csp^$csp=script-src 'self' 'unsafe-inline'",
            "^duplicated-directive^$csp=worker-src 'none'",
            "@@^disable-all^$csp",
            "^first-party-only^$csp=script-src 'none',1p",
        ];
        let blocker = Blocker::new_debug(filters);

        {
            // No directives should be returned for requests that are not `document` or `subdocument` content types.
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://pirateproxy.live/static/custom_ads.js",
                        "https://pirateproxy.live",
                        "script",
                        ""
                    )
                    .unwrap()
                ),
                None
            );
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://pirateproxy.live/static/custom_ads.js",
                        "https://pirateproxy.live",
                        "image",
                        ""
                    )
                    .unwrap()
                ),
                None
            );
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://pirateproxy.live/static/custom_ads.js",
                        "https://pirateproxy.live",
                        "object",
                        ""
                    )
                    .unwrap()
                ),
                None
            );
        }
        {
            // A single directive should be returned if only one match is present in the engine, for both document and subdocument types
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new("https://example.com", "https://vidoza.co", "document", "")
                        .unwrap()
                ),
                Some(String::from("worker-src 'none'"))
            );
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://example.com",
                        "https://vidoza.net",
                        "subdocument",
                        ""
                    )
                    .unwrap()
                ),
                Some(String::from("worker-src 'none'"))
            );
        }
        {
            // Multiple merged directives should be returned if more than one match is present in the engine
            let possible_results = [
                Some(String::from(
                    "script-src 'self' * 'unsafe-inline',worker-src 'none'",
                )),
                Some(String::from(
                    "worker-src 'none',script-src 'self' * 'unsafe-inline'",
                )),
            ];
            assert!(
                possible_results.contains(
                    &blocker.get_csp_directives(
                        &Request::new(
                            "https://example.com",
                            "https://pirateproxy.live",
                            "document",
                            ""
                        )
                        .unwrap()
                    )
                )
            );
            assert!(
                possible_results.contains(
                    &blocker.get_csp_directives(
                        &Request::new(
                            "https://example.com",
                            "https://pirateproxy.live",
                            "subdocument",
                            ""
                        )
                        .unwrap()
                    )
                )
            );
        }
        {
            // A directive with an exception should not be returned
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new("https://1337x.to", "https://1337x.to", "document", "").unwrap()
                ),
                Some(String::from("script-src 'self' 'unsafe-inline'"))
            );
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://1337x.to/no-csp",
                        "https://1337x.to",
                        "subdocument",
                        ""
                    )
                    .unwrap()
                ),
                None
            );
        }
        {
            // Multiple identical directives should only appear in the output once
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://example.com/duplicated-directive",
                        "https://flashx.cc",
                        "document",
                        ""
                    )
                    .unwrap()
                ),
                Some(String::from("worker-src 'none'"))
            );
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://example.com/duplicated-directive",
                        "https://flashx.cc",
                        "subdocument",
                        ""
                    )
                    .unwrap()
                ),
                Some(String::from("worker-src 'none'"))
            );
        }
        {
            // A CSP exception with no corresponding directive should disable all CSP injections for the page
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://1337x.to/duplicated-directive/disable-all",
                        "https://thepiratebay10.org",
                        "document",
                        ""
                    )
                    .unwrap()
                ),
                None
            );
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://1337x.to/duplicated-directive/disable-all",
                        "https://thepiratebay10.org",
                        "document",
                        ""
                    )
                    .unwrap()
                ),
                None
            );
        }
        {
            // A CSP exception with a partyness modifier should only match where the modifier applies
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "htps://github.com/first-party-only",
                        "https://example.com",
                        "subdocument",
                        ""
                    )
                    .unwrap()
                ),
                None
            );
            assert_eq!(
                blocker.get_csp_directives(
                    &Request::new(
                        "https://example.com/first-party-only",
                        "https://example.com",
                        "document",
                        ""
                    )
                    .unwrap()
                ),
                Some(String::from("script-src 'none'"))
            );
        }
    }

    #[test]
    fn test_removeparam() {
        let filters = [
            "||example.com^$removeparam=test",
            "*$removeparam=fbclid",
            "/script.js$redirect-rule=noopjs",
            "^block^$important",
            "$removeparam=testCase,~xhr",
        ];
        let blocker = Blocker::new(filters);
        let resources = ResourceStorage::in_memory_from_resources([Resource::simple(
            "noopjs",
            crate::resources::MimeType::ApplicationJavascript,
            "(() => {})()",
        )]);

        let result = blocker.check(
            &Request::new(
                "https://example.com?q=1&test=2#blue",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com?q=1#blue".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?test=2&q=1#blue",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com?q=1#blue".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?test=2#blue",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com#blue".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?q=1#blue",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, None);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?q=1&test=2",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, Some("https://example.com?q=1".into()));
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?test=2&q=1",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, Some("https://example.com?q=1".into()));
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?test=2",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, Some("https://example.com".into()));
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?test=2",
                "https://antonok.com",
                "image",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, None);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new("https://example.com?q=1", "https://antonok.com", "xhr", "").unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, None);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?q=fbclid",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, None);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?fbclid=10938&q=1&test=2",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, Some("https://example.com?q=1".into()));
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://test.com?fbclid=10938&q=1&test=2",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://test.com?q=1&test=2".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?q1=1&q2=2&q3=3&test=2&q4=4&q5=5&fbclid=39",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com?q1=1&q2=2&q3=3&q4=4&q5=5".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?q1=1&q1=2&test=2&test=3",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com?q1=1&q1=2".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/script.js?test=2#blue",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com/script.js#blue".into())
        );
        assert_eq!(
            result.redirect,
            Some("data:application/javascript;base64,KCgpID0+IHt9KSgp".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/block/script.js?test=2",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, None);
        assert_eq!(
            result.redirect,
            Some("data:application/javascript;base64,KCgpID0+IHt9KSgp".into())
        );
        assert!(result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/Path/?Test=ABC&testcase=AbC&testCase=aBc",
                "https://antonok.com",
                "xhr",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, None);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/Path/?Test=ABC&testcase=AbC&testCase=aBc",
                "https://antonok.com",
                "image",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, None);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/Path/?Test=ABC&testcase=AbC&testCase=aBc",
                "https://antonok.com",
                "subdocument",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com/Path/?Test=ABC&testcase=AbC".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/Path/?Test=ABC&testcase=AbC&testCase=aBc",
                "https://antonok.com",
                "document",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com/Path/?Test=ABC&testcase=AbC".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?Test=ABC?123&test=3#&test=4#b",
                "https://antonok.com",
                "document",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com?Test=ABC?123#&test=4#b".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?Test=ABC&testCase=5",
                "https://antonok.com",
                "document",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(
            result.rewritten_url,
            Some("https://example.com?Test=ABC".into())
        );
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com?Test=ABC&testCase=5",
                "https://antonok.com",
                "image",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.rewritten_url, None);
        assert!(!result.should_block());
    }

    /// Tests ported from the previous query parameter stripping logic in brave-core
    #[test]
    fn removeparam_brave_core_tests() {
        let testcases = [
            // (original url, expected url after filtering)
            ("https://example.com/?fbclid=1234", "https://example.com/"),
            ("https://example.com/?fbclid=1234&", "https://example.com/"),
            ("https://example.com/?&fbclid=1234", "https://example.com/"),
            ("https://example.com/?gclid=1234", "https://example.com/"),
            (
                "https://example.com/?fbclid=0&gclid=1&msclkid=a&mc_eid=a1",
                "https://example.com/",
            ),
            (
                "https://example.com/?fbclid=&foo=1&bar=2&gclid=abc",
                "https://example.com/?fbclid=&foo=1&bar=2",
            ),
            (
                "https://example.com/?fbclid=&foo=1&gclid=1234&bar=2",
                "https://example.com/?fbclid=&foo=1&bar=2",
            ),
            (
                "http://u:p@example.com/path/file.html?foo=1&fbclid=abcd#fragment",
                "http://u:p@example.com/path/file.html?foo=1#fragment",
            ),
            ("https://example.com/?__s=1234-abcd", "https://example.com/"),
            // Obscure edge cases that break most parsers:
            (
                "https://example.com/?fbclid&foo&&gclid=2&bar=&%20",
                "https://example.com/?fbclid&foo&&bar=&%20",
            ),
            (
                "https://example.com/?fbclid=1&1==2&=msclkid&foo=bar&&a=b=c&",
                "https://example.com/?1==2&=msclkid&foo=bar&&a=b=c&",
            ),
            (
                "https://example.com/?fbclid=1&=2&?foo=yes&bar=2+",
                "https://example.com/?=2&?foo=yes&bar=2+",
            ),
            (
                "https://example.com/?fbclid=1&a+b+c=some%20thing&1%202=3+4",
                "https://example.com/?a+b+c=some%20thing&1%202=3+4",
            ),
            // Conditional query parameter stripping
            /*("https://example.com/?mkt_tok=123&foo=bar",
            "https://example.com/?foo=bar"),*/
        ];

        let filters = [
            "fbclid",
            "gclid",
            "msclkid",
            "mc_eid",
            "dclid",
            "oly_anon_id",
            "oly_enc_id",
            "_openstat",
            "vero_conv",
            "vero_id",
            "wickedid",
            "yclid",
            "__s",
            "rb_clickid",
            "s_cid",
            "ml_subscriber",
            "ml_subscriber_hash",
            "twclid",
            "gbraid",
            "wbraid",
            "_hsenc",
            "__hssc",
            "__hstc",
            "__hsfp",
            "hsCtaTracking",
            "oft_id",
            "oft_k",
            "oft_lk",
            "oft_d",
            "oft_c",
            "oft_ck",
            "oft_ids",
            "oft_sk",
            "ss_email_id",
            "bsft_uid",
            "bsft_clkid",
            "vgo_ee",
            "igshid",
        ]
        .iter()
        .map(|s| format!("*$removeparam={s}"))
        .collect::<Vec<_>>();
        let blocker = Blocker::new(filters);
        let resources = ResourceStorage::default();

        for (original, expected) in testcases.into_iter() {
            let result = blocker.check(
                &Request::new(original, "https://example.net", "xhr", "").unwrap(),
                &resources,
            );
            let expected = if original == expected {
                None
            } else {
                Some(expected.to_string())
            };
            assert_eq!(
                expected, result.rewritten_url,
                "Filtering parameters on {original} failed"
            );
        }
    }

    #[test]
    fn test_removeparam_same_tokens() {
        let filters = ["$removeparam=example1_", "$removeparam=example1-"];
        let blocker = Blocker::new(filters);

        let result = blocker.check(
            &Request::new(
                "https://example.com?example1_=1&example1-=2",
                "https://example.com",
                "xhr",
                "",
            )
            .unwrap(),
            &Default::default(),
        );
        assert_eq!(result.rewritten_url, Some("https://example.com".into()));
        assert!(!result.should_block());
    }

    #[test]
    fn test_redirect_priority() {
        let filters = [
            ".txt^$redirect-rule=a",
            "||example.com^$redirect-rule=b:10",
            "/text$redirect-rule=c:20",
            "@@^excepta^$redirect-rule=a",
            "@@^exceptb10^$redirect-rule=b:10",
            "@@^exceptc20^$redirect-rule=c:20",
        ];
        let blocker = Blocker::new(filters);
        fn simple_resource(identifier: &str) -> Resource {
            Resource::simple(
                identifier,
                crate::resources::MimeType::TextPlain,
                identifier,
            )
        }
        fn simple_redirect(identifier: &str) -> String {
            format!(
                "data:text/plain;base64,{}",
                BASE64_STANDARD.encode(identifier)
            )
        }
        let test_cases = ["a", "b", "c"];
        let resources = ResourceStorage::in_memory_from_resources(test_cases.map(simple_resource));
        let redirects = test_cases
            .into_iter()
            .map(simple_redirect)
            .collect::<Vec<_>>();
        let a_redirect = Some(redirects[0].clone());
        let b_redirect = Some(redirects[1].clone());
        let c_redirect = Some(redirects[2].clone());

        let result = blocker.check(
            &Request::new(
                "https://example.net/test",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, None);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.net/test.txt",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, a_redirect);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/test.txt",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, b_redirect);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/text.txt",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, c_redirect);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/exceptc20/text.txt",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, b_redirect);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/exceptb10/text.txt",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, c_redirect);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/exceptc20/exceptb10/text.txt",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, a_redirect);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/exceptc20/exceptb10/excepta/text.txt",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, None);
        assert!(!result.should_block());

        let result = blocker.check(
            &Request::new(
                "https://example.com/exceptc20/exceptb10/text",
                "https://example.com",
                "xmlhttprequest",
                "",
            )
            .unwrap(),
            &resources,
        );
        assert_eq!(result.redirect, None);
        assert!(!result.should_block());
    }

    #[test]
    fn tags_enable_works() {
        let filters = [
            "adv$tag=stuff",
            "somelongpath/test$tag=stuff",
            "||brianbondy.com/$tag=brian",
            "||brave.com$tag=brian",
        ];
        let url_results = [
            ("http://example.com/advert.html", true),
            ("http://example.com/somelongpath/test/2.html", true),
            ("https://brianbondy.com/about", false),
            ("https://brave.com/about", false),
        ];

        let request_expectations: Vec<_> = url_results
            .into_iter()
            .map(|(url, expected_result)| {
                let request = Request::new(url, "https://example.com", "other", "").unwrap();
                (request, expected_result)
            })
            .collect();

        let mut blocker = Blocker::new_debug(filters);
        let resources = Default::default();
        blocker.enable_tags(&["stuff"]);
        assert_eq!(
            blocker.tags_enabled,
            HashSet::from_iter([String::from("stuff")].into_iter())
        );

        request_expectations
            .into_iter()
            .for_each(|(req, expected_result)| {
                let matched_rule = blocker.check(&req, &resources);
                if expected_result {
                    assert!(
                        matched_rule.should_block(),
                        "Expected match for {}",
                        req.url
                    );
                } else {
                    assert!(
                        !matched_rule.should_block(),
                        "Expected no match for {}, matched with {:?}",
                        req.url,
                        matched_rule.filter
                    );
                }
            });
    }

    #[test]
    fn tags_enable_adds_tags() {
        let filters = [
            "adv$tag=stuff",
            "somelongpath/test$tag=stuff",
            "||brianbondy.com/$tag=brian",
            "||brave.com$tag=brian",
        ];
        let url_results = [
            ("http://example.com/advert.html", true),
            ("http://example.com/somelongpath/test/2.html", true),
            ("https://brianbondy.com/about", true),
            ("https://brave.com/about", true),
        ];

        let request_expectations: Vec<_> = url_results
            .into_iter()
            .map(|(url, expected_result)| {
                let request = Request::new(url, "https://example.com", "other", "").unwrap();
                (request, expected_result)
            })
            .collect();

        let mut blocker = Blocker::new_debug(filters);
        let resources = Default::default();
        blocker.enable_tags(&["stuff"]);
        blocker.enable_tags(&["brian"]);
        assert_eq!(
            blocker.tags_enabled,
            HashSet::from_iter([String::from("brian"), String::from("stuff")].into_iter())
        );

        request_expectations
            .into_iter()
            .for_each(|(req, expected_result)| {
                let matched_rule = blocker.check(&req, &resources);
                if expected_result {
                    assert!(
                        matched_rule.should_block(),
                        "Expected match for {}",
                        req.url
                    );
                } else {
                    assert!(
                        !matched_rule.should_block(),
                        "Expected no match for {}, matched with {:?}",
                        req.url,
                        matched_rule.filter
                    );
                }
            });
    }

    #[test]
    fn tags_disable_works() {
        let filters = [
            "adv$tag=stuff",
            "somelongpath/test$tag=stuff",
            "||brianbondy.com/$tag=brian",
            "||brave.com$tag=brian",
        ];
        let url_results = [
            ("http://example.com/advert.html", false),
            ("http://example.com/somelongpath/test/2.html", false),
            ("https://brianbondy.com/about", true),
            ("https://brave.com/about", true),
        ];

        let request_expectations: Vec<_> = url_results
            .into_iter()
            .map(|(url, expected_result)| {
                let request = Request::new(url, "https://example.com", "other", "").unwrap();
                (request, expected_result)
            })
            .collect();

        let mut blocker = Blocker::new_debug(filters);
        let resources = Default::default();
        blocker.enable_tags(&["brian", "stuff"]);
        assert_eq!(
            blocker.tags_enabled,
            HashSet::from_iter([String::from("brian"), String::from("stuff")].into_iter())
        );
        blocker.disable_tags(&["stuff"]);
        assert_eq!(
            blocker.tags_enabled,
            HashSet::from_iter([String::from("brian")].into_iter())
        );

        request_expectations
            .into_iter()
            .for_each(|(req, expected_result)| {
                let matched_rule = blocker.check(&req, &resources);
                if expected_result {
                    assert!(
                        matched_rule.should_block(),
                        "Expected match for {}",
                        req.url
                    );
                } else {
                    assert!(
                        !matched_rule.should_block(),
                        "Expected no match for {}, matched with {:?}",
                        req.url,
                        matched_rule.filter
                    );
                }
            });
    }

    #[test]
    fn exception_force_check() {
        let blocker = Blocker::new(["@@*ad_banner.png"]);

        let resources = Default::default();

        let request = Request::new(
            "http://example.com/ad_banner.png",
            "https://example.com",
            "other",
            "",
        )
        .unwrap();

        let matched_rule = blocker.check_parameterised(&request, &resources, false, true);
        assert!(!matched_rule.should_block());
        assert!(matched_rule.exception.is_some());
    }

    #[test]
    fn generichide() {
        let blocker = Blocker::new(["@@||example.com$generichide\n"]);

        assert!(blocker.check_generic_hide(
            &Request::new("https://example.com", "https://example.com", "other", "").unwrap()
        ));
    }
}

#[cfg(test)]
mod placeholder_string_tests {
    /// If this changes, be sure to update the documentation for [`BlockerResult`] as well.
    #[test]
    fn test_constant_placeholder_string() {
        let mut filter_set = crate::lists::FilterSet::new(false);
        filter_set.add_filter_list("||example.com^\n".to_string(), Default::default());
        let engine = crate::Engine::new_with_filter_set(filter_set);
        let block = engine.check_network_request(
            &crate::request::Request::new(
                "https://example.com",
                "https://example.com",
                "document",
                "",
            )
            .unwrap(),
        );
        assert_eq!(block.filter.and_then(|f| f.raw_line), None);
    }
}

#[cfg(test)]
mod legacy_rule_parsing_tests {
    use crate::blocker::Blocker;
    use crate::engine::Engine;
    use crate::filters::network::NetworkFilterMaskHelper;
    use crate::lists::{FilterFormat, FilterSet, ParseOptions, parse_filters};

    fn parsed_counts(rules: &str, format: FilterFormat) -> (usize, usize, usize) {
        let (network_filters, cosmetic_filters) = parse_filters(
            rules.lines(),
            true,
            ParseOptions {
                format,
                ..Default::default()
            },
        );
        (
            network_filters.len(),
            network_filters
                .iter()
                .filter(|filter| filter.is_exception())
                .count(),
            cosmetic_filters.len(),
        )
    }

    #[test]
    fn parses_authored_standard_corpus() {
        let rules = concat!(
            "||script-a.corpus.invalid^$script\n",
            "@@||script-a.corpus.invalid/allowed.js$script,",
            "domain=publisher.corpus.invalid\n",
            "publisher.corpus.invalid##.sponsor\n",
        );
        assert_eq!(parsed_counts(rules, FilterFormat::Standard), (2, 1, 1));

        let mut filter_set = FilterSet::new(true);
        filter_set.add_filter_list(rules.to_owned(), Default::default());
        let engine = Engine::new_with_filter_set_no_optimize(filter_set);
        let blocker = Blocker::from_context(engine.filter_data_context());
        assert!(
            blocker.exceptions().get_filter_map().total_size()
                + blocker.generic_hide().get_filter_map().total_size()
                >= 1
        );
        assert!(
            blocker.filters().get_filter_map().total_size()
                + blocker.importants().get_filter_map().total_size()
                + blocker.redirects().get_filter_map().total_size()
                + blocker.csp().get_filter_map().total_size()
                >= 1
        );
    }

    #[test]
    fn parses_generated_network_corpus() {
        let rules = crate::test_utils::synthetic_network_rules(4_096);
        let (network, exceptions, cosmetic) = parsed_counts(&rules, FilterFormat::Standard);
        assert_eq!(network, 4_096);
        assert_eq!(exceptions, 0);
        assert_eq!(cosmetic, 0);

        let mut filter_set = FilterSet::new(false);
        filter_set.add_filter_list(rules, Default::default());
        let engine = Engine::new_with_filter_set_no_optimize(filter_set);
        let blocker = Blocker::from_context(engine.filter_data_context());
        assert!(
            blocker.filters().get_filter_map().total_size()
                + blocker.importants().get_filter_map().total_size()
                >= 4_096
        );
    }

    #[test]
    fn parses_authored_hosts_corpus() {
        let rules = concat!(
            "0.0.0.0 ads-a.corpus.invalid\n",
            "127.0.0.1 ads-b.corpus.invalid\n",
            "::1 localhost\n",
        );
        let (network, exceptions, cosmetic) = parsed_counts(rules, FilterFormat::Hosts);
        assert_eq!(network, 2);
        assert_eq!(exceptions, 0);
        assert_eq!(cosmetic, 0);
    }
}
