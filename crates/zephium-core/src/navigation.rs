tp://localhost:8080/"),
            ("app.localhost", "http://app.localhost/"),
            (
                "app.localhost:3000/x?y=1#z",
                "http://app.localhost:3000/x?y=1#z",
            ),
            ("127.0.0.1", "http://127.0.0.1/"),
            (
                "127.0.0.1:8080/admin?q=1#frag",
                "http://127.0.0.1:8080/admin?q=1#frag",
            ),
            ("127.42.7.9:8080", "http://127.42.7.9:8080/"),
            ("[::1]", "http://[::1]/"),
            ("[::1]:7474/path?q=1#f", "http://[::1]:7474/path?q=1#f"),
        ] {
            assert!(!is_query(input), "{input}");
            assert_eq!(classify(input).unwrap().as_str(), expected, "{input}");
        }
        assert_eq!(
            web_address(" app.localhost:3000/x ").unwrap().as_str(),
            "http://app.localhost:3000/x"
        );
    }

    #[test]
    fn loopback_lookalikes_and_non_loopback_hosts_keep_https() {
        for input in [
            "localhost.evil.com",
            "evil.localhost.com",
            "mylocalhost:8080",
            "127.0.0.1.evil.com",
            "127.0.0.1.example:8080",
            "example.com",
            "192.168.1.1:8080",
            "[::2]:8080",
            "[2001:db8::1]:8080",
        ] {
            let url = classify(input).unwrap_or_else(|| panic!("classified {input}"));
            assert_eq!(url.scheme(), "https", "{input}");
        }
    }

    #[test]
    fn loopback_scheme_default_keeps_explicit_and_security_rules() {
        for input in [
            "https://localhost:8080/a?b#c",
            "https://app.localhost/a",
            "https://127.0.0.1:8080/a",
            "https://[::1]:8080/a",
        ] {
            assert_eq!(classify(input).unwrap().as_str(), input);
        }
        assert_eq!(
            classify("http://localhost:8080/a").unwrap().as_str(),
            "http://localhost:8080/a"
        );
        // User info is never turned into a bare loopback navigation.
        assert!(is_query("user@localhost:8080"));
        assert_eq!(
            classify("user@localhost:8080").unwrap().host_str(),
            Some("duckduckgo.com")
        );
        for input in [
            "http://user:pass@localhost:8080/",
            "https://user:pass@localhost:8080/",
        ] {
            assert!(classify(input).is_none(), "{input}");
        }
        // Reserved application hosts stay denied even as `.localhost` names.
        for host in [
            "asset.localhost",
            "ipc.localhost",
            "tauri.localhost",
            "zephium-onboarding.localhost",
        ] {
            for input in [host.to_owned(), format!("{host}:8080/x")] {
                assert!(classify(&input).is_none(), "{input}");
                assert!(!is_query(&input), "{input}");
            }
        }
    }

    #[test]
    fn bare_loopback_urls_are_never_attempted_over_https_first() {
        for url in [
            "http://localhost/",
            "http://localhost:8080/",
            "http://app.localhost/",
            "http://app.localhost:3000/x",
            "http://127.0.0.1:8080/x",
            "http://[::1]:7474/x",
        ] {
            assert_eq!(https_upgrade(&Url::parse(url).unwrap()), None, "{url}");
        }
    }

    #[test]
    fn only_a_page_acting_alone_with_no_app_stays_silent() {
        assert_eq!(app_link_outcome(true, true), AppLinkOutcome::Ask);
        assert_eq!(app_link_outcome(true, false), AppLinkOutcome::Ask);
        assert_eq!(app_link_outcome(false, true), AppLinkOutcome::Show);
        assert_eq!(app_link_outcome(false, false), AppLinkOutcome::Silent);
    }

    #[test]
    fn frames_may_hold_documents_their_page_builds() {
        for frame in [
            "about:srcdoc",
            "about:blank#top",
            "data:text/html,<p>hi</p>",
            "blob:https://challenges.example/0f1e",
        ] {
            assert!(super::is_subframe_document(frame), "{frame}");
        }
        for frame in [
            "about:settings",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "zoommtg://x",
        ] {
            assert!(!super::is_subframe_document(frame), "{frame}");
        }
    }

    #[test]
    fn only_public_names_on_the_default_port_are_tried_over_https_first() {
        for (from, to) in [
            (
                "http://example.com/a?b#c",
                Some("https://example.com/a?b#c"),
            ),
            (
                "http://www.example.co.uk/",
                Some("https://www.example.co.uk/"),
            ),
            ("https://example.com/", None),
            ("http://example.com:8080/", None),
            ("http://93.184.216.34/", None),
            ("http://[2001:db8::1]/", None),
            ("http://router.lan/", None),
            ("http://printer.local/", None),
            ("http://intranet/", None),
            ("http://localhost/", None),
            ("http://localhost:8080/", None),
            ("http://app.localhost/", None),
            ("http://127.0.0.1:8080/", None),
            ("http://[::1]:7474/", None),
        ] {
            let url = Url::parse(from).unwrap();
            assert_eq!(
                super::https_upgrade(&url).map(|url| url.to_string()),
                to.map(str::to_owned),
                "{from}"
            );
        }
        // An address at the limit is not pushed past it by the extra "s".
        let longest = format!("http://example.com/{}", "a".repeat(MAX_URL_BYTES - 19));
        assert_eq!(longest.len(), MAX_URL_BYTES);
        assert_eq!(super::https_upgrade(&Url::parse(&longest).unwrap()), None);
    }

    #[test]
    fn only_application_links_are_handed_to_applications() {
        for link in [
            "zoommtg://zoom.us/join?confno=123",
            "mailto:someone@example.com",
            "msteams:/l/meetup-join/1",
            "slack://open",
            "tel:+15551234",
        ] {
            assert!(super::external_app_link(link).is_some(), "{link}");
        }
        for link in [
            "https://zoom.us/j/1",
            "http://example.com/",
            "about:blank",
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "blob:https://example.com/1",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "smb://host/share",
            "zephium://settings",
            "chrome-extension://abc/page.html",
            "mailto://user:secret@example.com",
            "not a url",
        ] {
            assert!(super::external_app_link(link).is_none(), "{link}");
        }
    }

    use super::*;
    use proptest::prelude::*;

    #[test]
    fn bare_host_gets_https() {
        assert_eq!(
            classify("example.com").unwrap().as_str(),
            "https://example.com/"
        );
        assert_eq!(
            classify("  github.com  ").unwrap().as_str(),
            "https://github.com/"
        );
        assert_eq!(
            classify("sub.example.com:8080/x").unwrap().scheme(),
            "https"
        );
    }

    #[test]
    fn explicit_url_passes_through() {
        assert_eq!(
            classify("https://x.com/a").unwrap().as_str(),
            "https://x.com/a"
        );
    }

    #[test]
    fn is_query_splits_urls_from_text() {
        assert!(is_query("hello world"));
        assert!(!is_query("example.com"));
        assert!(!is_query("https://x.com/a"));
        assert!(!is_query("file:///etc/passwd"));
        assert!(!is_query("tauri.localhost/index.html"));
        assert!(!is_query(""));
    }

    #[test]
    fn text_becomes_search() {
        let u = classify("hello world").unwrap();
        assert_eq!(u.host_str(), Some("duckduckgo.com"));
        assert_eq!(u.query(), Some("q=hello+world"));
    }

    #[test]
    fn search_text_may_contain_web_urls() {
        let query = "compare (https://example.com/a?x=1), with https://example.org/b#two";
        assert!(is_query(query));
        let u = classify(query).expect("embedded URLs are search text");
        assert_eq!(u.host_str(), Some("duckduckgo.com"));
        assert!(u
            .query_pairs()
            .any(|(key, value)| key == "q" && value == query));
        assert!(search_query(query).is_some());
    }

    #[test]
    fn embedded_targets_keep_the_search_exfiltration_guard() {
        for query in [
            "compare https://user:secret@example.com with docs",
            "find file:///private.txt online",
            "open steam://run/123 in docs",
            "compare https:// with docs",
            "compare http://tauri.localhost/index.html with docs",
            "compare https://example.com/path,steam://run/123 with docs",
            "compare https://example.com/path?next=file:///private with docs",
        ] {
            assert!(!is_query(query), "searched {query}");
            assert!(classify(query).is_none(), "classified {query}");
            assert!(search_query(query).is_none(), "sent {query}");
        }
    }

    #[test]
    fn explicit_search_never_turns_host_shaped_text_into_navigation() {
        assert_eq!(
            search_query("example.com").unwrap().as_str(),
            "https://duckduckgo.com/?q=example.com"
        );
        assert!(search_query("https://user:secret@example.com/").is_none());
        assert!(search_query("file:///private.txt").is_none());
        assert!(search_query("https://").is_none());
        assert!(search_query("/Users/alice/private.txt").is_none());
    }

    #[test]
    fn percent_expanded_search_must_fit_the_native_url_ceiling() {
        assert!(classify(&"x".repeat(MAX_URL_BYTES)).is_none());
        assert!(classify(&"💣".repeat(MAX_URL_BYTES / 4)).is_none());

        let accepted = classify(&"x".repeat(1024)).expect("small search");
        assert!(accepted.as_str().len() <= MAX_URL_BYTES);
        assert!(is_allowed(&accepted));
    }

    #[test]
    fn scheme_gate_blocks_dangerous() {
        assert!(!is_allowed(&Url::parse("javascript:alert(1)").unwrap()));
        assert!(!is_allowed(&Url::parse("file:///etc/passwd").unwrap()));
        assert!(!is_allowed(&Url::parse("about:config").unwrap()));
        assert!(!is_allowed(&Url::parse("about:srcdoc").unwrap()));
        assert!(!is_allowed(
            &Url::parse("https://user:secret@example.com/").unwrap()
        ));
        assert!(!is_allowed(
            &Url::parse("http://tauri.localhost/index.html").unwrap()
        ));
        for reserved in [
            "http://tauri.localhost./index.html",
            "https://tauri.localhost./browser.html",
            "http://tauri.localhost../index.html",
            "HTTP://TAURI.LOCALHOST./index.html",
            "http://ipc.localhost./",
            "https://asset.localhost./app.js",
            "http://zephium-onboarding.localhost./onboarding.html",
        ] {
            assert!(!is_allowed(&Url::parse(reserved).unwrap()), "{reserved}");
            assert!(!is_allowed_str(reserved), "{reserved}");
            assert_eq!(external_target(reserved), None, "{reserved}");
        }
        assert!(is_allowed(&Url::parse("https://localhost./").unwrap()));
        assert!(is_allowed(&Url::parse("https://example.com").unwrap()));
        assert!(is_allowed(&Url::parse("about:blank").unwrap()));
        assert!(is_allowed_str("https://x.com/"));
        assert!(!is_allowed_str("javascript:1"));
        assert!(!is_allowed_str("not a url"));
    }

    #[test]
    fn omnibox_rejects_explicit_forbidden_targets_without_searching() {
        for input in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,secret",
            "about:config",
            "tauri.localhost/index.html",
            "tauri.localhost./index.html",
            "ipc.localhost./",
            "asset.localhost./",
            "zephium-onboarding.localhost/onboarding.html",
            "zephium-onboarding.localhost./onboarding.html",
            "compare http://tauri.localhost./index.html with docs",
            "https://user:secret@example.com/",
            "https://",
            "/home/alice/private.txt",
            "~/private.txt",
            "./private.txt",
            "../private.txt",
            r"C:\Users\alice\private.txt",
            r"C:/Users/alice/private.txt",
            r"\\server\share\private.txt",
        ] {
            assert!(classify(input).is_none(), "accepted {input}");
            assert!(!is_query(input), "searched {input}");
        }
    }

    #[test]
    fn userinfo_input_searches_instead_of_spoofing() {
        assert_eq!(
            classify("paypal.com@evil.com").unwrap().host_str(),
            Some("duckduckgo.com")
        );
        assert_eq!(
            classify("someone@example.com").unwrap().host_str(),
            Some("duckduckgo.com")
        );
        assert!(is_query("paypal.com@evil.com"));
        assert!(classify("https://user:secret@example.com/").is_none());
    }

    #[test]
    fn ipv4_literals_navigate() {
        assert_eq!(
            classify("192.168.1.1").unwrap().as_str(),
            "https://192.168.1.1/"
        );
        assert_eq!(
            classify("192.168.1.1:8080/admin").unwrap().as_str(),
            "https://192.168.1.1:8080/admin"
        );
        // not a valid address: searched, not navigated
        assert!(is_query("999.1.1.1"));
    }

    proptest! {
        #[test]
        fn classify_never_panics(s in "\\PC*") {
            let _ = classify(&s);
        }
    }

    #[test]
    fn hand_offs_admit_only_ordinary_web_pages() {
        assert_eq!(
            external_target(" https://example.com/a?b=1 ").map(|url| url.to_string()),
            Some("https://example.com/a?b=1".into())
        );
        assert!(external_target("http://example.com").is_some());
        for refused in [
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "about:blank",
            "zephium://settings",
            "https://user:pass@example.com/",
            "/Applications/Zephium.app",
            "--flag",
            "",
        ] {
            assert_eq!(external_target(refused), None, "{refused}");
        }
        assert_eq!(
            external_target(&format!(
                "https://example.com/{}",
                "a".repeat(MAX_URL_BYTES)
            )),
            None
        );
    }

    #[test]
    fn a_typed_web_address_is_never_a_search() {
        assert_eq!(
            web_address(" example.com/docs ").map(|url| url.to_string()),
            Some("https://example.com/docs".into())
        );
        assert!(web_address("https://a.example/").is_some());
        for refused in [
            "",
            "what is rust",
            "about:blank",
            "file:///etc/hosts",
            "javascript:1",
        ] {
            assert!(web_address(refused).is_none(), "{refused}");
        }
    }
}
