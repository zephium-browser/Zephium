#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::blocker::{NetworkMatcherPreparationError, NetworkMatcherPreparationLimits};
    use crate::resources::MimeType;
    use crate::{
        FilterSet, lists::FilterFormat, lists::ParseOptions, test_utils::synthetic_network_rules,
    };
    use base64::{engine::Engine as _, prelude::BASE64_STANDARD};
    use seahash::hash;

    #[test]
    #[allow(deprecated)]
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

        let mut engine = Engine::new_with_list_text(filters.join("\n"));
        engine.enable_tags(&["stuff"]);
        engine.enable_tags(&["brian"]);

        url_results.into_iter().for_each(|(url, expected_result)| {
            let request = Request::new(url, url, "", "").unwrap();
            let matched_rule = engine.check_network_request(&request);
            if expected_result {
                assert!(matched_rule.should_block(), "Expected match for {url}");
            } else {
                assert!(
                    !matched_rule.should_block(),
                    "Expected no match for {}, matched with {:?}",
                    url,
                    matched_rule.filter
                );
            }
        });
    }

    #[test]
    #[allow(deprecated)]
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

        let mut engine = Engine::new_with_list_text(filters.join("\n"));
        engine.enable_tags(&["brian", "stuff"]);
        engine.disable_tags(&["stuff"]);

        url_results.into_iter().for_each(|(url, expected_result)| {
            let request = Request::new(url, url, "", "").unwrap();
            let matched_rule = engine.check_network_request(&request);
            if expected_result {
                assert!(matched_rule.should_block(), "Expected match for {url}");
            } else {
                assert!(
                    !matched_rule.should_block(),
                    "Expected no match for {}, matched with {:?}",
                    url,
                    matched_rule.filter
                );
            }
        });
    }

    #[test]
    #[allow(deprecated)]
    fn exception_tags_inactive_by_default() {
        let filters = [
            "adv",
            "||brianbondy.com/$tag=brian",
            "@@||brianbondy.com/$tag=brian",
        ];
        let url_results = [
            ("http://example.com/advert.html", true),
            ("https://brianbondy.com/about", false),
            ("https://brianbondy.com/advert", true),
        ];

        let engine = Engine::new_with_list_text(filters.join("\n"));

        url_results.into_iter().for_each(|(url, expected_result)| {
            let request = Request::new(url, url, "", "").unwrap();
            let matched_rule = engine.check_network_request(&request);
            if expected_result {
                assert!(matched_rule.should_block(), "Expected match for {url}");
            } else {
                assert!(
                    !matched_rule.should_block(),
                    "Expected no match for {}, matched with {:?}",
                    url,
                    matched_rule.filter
                );
            }
        });
    }

    #[test]
    #[allow(deprecated)]
    fn exception_tags_works() {
        let filters = [
            "adv",
            "||brianbondy.com/$tag=brian",
            "@@||brianbondy.com/$tag=brian",
        ];
        let url_results = [
            ("http://example.com/advert.html", true),
            ("https://brianbondy.com/about", false),
            ("https://brianbondy.com/advert", false),
        ];

        let mut engine = Engine::new_with_list_text(filters.join("\n"));
        engine.enable_tags(&["brian", "stuff"]);

        url_results.into_iter().for_each(|(url, expected_result)| {
            let request = Request::new(url, url, "", "").unwrap();
            let matched_rule = engine.check_network_request(&request);
            if expected_result {
                assert!(matched_rule.should_block(), "Expected match for {url}");
            } else {
                assert!(
                    !matched_rule.should_block(),
                    "Expected no match for {}, matched with {:?}",
                    url,
                    matched_rule.filter
                );
            }
        });
    }

    #[test]
    #[allow(deprecated)]
    fn serialization_retains_tags() {
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

        let mut engine = Engine::new_with_list_text(filters.join("\n"));
        engine.enable_tags(&["stuff"]);
        engine.enable_tags(&["brian"]);
        let serialized = engine.serialize();
        let mut deserialized_engine = Engine::default();
        deserialized_engine.enable_tags(&["stuff"]);
        deserialized_engine.deserialize(&serialized).unwrap();

        url_results.into_iter().for_each(|(url, expected_result)| {
            let request = Request::new(url, url, "", "").unwrap();
            let matched_rule = deserialized_engine.check_network_request(&request);
            if expected_result {
                assert!(matched_rule.should_block(), "Expected match for {url}");
            } else {
                assert!(
                    !matched_rule.should_block(),
                    "Expected no match for {}, matched with {:?}",
                    url,
                    matched_rule.filter
                );
            }
        });
    }

    const HASH_MISMATCH_MSG: &str = r#"
      A change has been detected in the serialized format! If the change is intentional:
      1. Update ADBLOCK_RUST_DAT_VERSION before updating the expected hashes
      2. DON'T rely on backwards compatibility with the old format
      Backwards compatibility isn't covered by the tests"#;

    #[test]
    fn deserialization_generate_simple() {
        let mut engine = Engine::new_with_list_text("ad-banner");
        let data = engine.serialize().to_vec();
        const EXPECTED_HASH: u64 = 2257902529834677439;
        assert_eq!(hash(&data), EXPECTED_HASH, "{HASH_MISMATCH_MSG}");
        engine.deserialize(&data).unwrap();
    }

    #[test]
    #[allow(deprecated)]
    fn deserialization_generate_tags() {
        let mut engine = Engine::new_with_list_text("ad-banner$tag=abc");
        engine.use_tags(&["abc"]);
        let data = engine.serialize().to_vec();
        const EXPECTED_HASH: u64 = 2434333752507121032;
        assert_eq!(hash(&data), EXPECTED_HASH, "{HASH_MISMATCH_MSG}");
        engine.deserialize(&data).unwrap();
    }

    #[test]
    fn deserialization_generate_resources() {
        let mut engine = Engine::new_with_list_text("ad-banner$redirect=nooptext");

        engine.use_resources([
            Resource::simple("nooptext", MimeType::TextPlain, ""),
            Resource::simple("noopcss", MimeType::TextCss, ""),
        ]);

        let serialized = engine.serialize().to_vec();
        println!("Engine serialized: {serialized:?}");
        engine.deserialize(&serialized).unwrap();
    }

    #[test]
    fn deserialization_generated_corpus() {
        let mut engine = Engine::new_with_list_text(synthetic_network_rules(4_096));
        let request = Request::new(
            "https://script-0.corpus.invalid/client.js",
            "https://publisher.corpus.invalid/",
            "script",
            "get",
        )
        .unwrap();
        assert!(engine.check_network_request(&request).should_block());

        let data = engine.serialize().to_vec();
        assert!(!data.is_empty());
        engine.deserialize(&data).unwrap();
        assert!(engine.check_network_request(&request).should_block());
    }

    #[test]
    fn redirect_resource_insertion_works() {
        let mut engine = Engine::new_with_list_text(
            ["ad-banner$redirect=nooptext", "script.js$redirect=noop.js"].join("\n"),
        );

        let script = r#"
(function() {
	;
})();

        "#;
        let mut resources = [
            Resource::simple("nooptext", MimeType::TextPlain, ""),
            Resource::simple("noopjs", MimeType::ApplicationJavascript, script),
        ];
        resources[1].aliases.push("noop.js".to_string());
        engine.use_resources(resources);

        let url = "http://example.com/ad-banner.gif";
        let request = Request::new(url, url, "", "").unwrap();
        let matched_rule = engine.check_network_request(&request);
        assert!(matched_rule.should_block(), "Expected match for {url}");
        assert_eq!(
            matched_rule.redirect,
            Some("data:text/plain;base64,".to_owned()),
            "Expected redirect to contain resource"
        );

        let url = "http://example.com/script.js";
        let request = Request::new(url, url, "", "").unwrap();
        let matched_rule = engine.check_network_request(&request);
        assert!(matched_rule.should_block(), "Expected match for {url}");
        assert_eq!(
            matched_rule.redirect,
            Some(format!(
                "data:application/javascript;base64,{}",
                BASE64_STANDARD.encode(script)
            )),
            "Expected redirect to contain resource"
        );
    }

    #[test]
    fn document() {
        let filters = ["||example.com$document", "@@||sub.example.com$document"];

        let engine = Engine::new_with_list_text(filters.join("\n"));

        assert!(
            engine
                .check_network_request(
                    &Request::new("https://example.com", "https://example.com", "document", "")
                        .unwrap()
                )
                .should_block()
        );
        assert!(
            !engine
                .check_network_request(
                    &Request::new("https://example.com", "https://example.com", "script", "")
                        .unwrap()
                )
                .should_block()
        );
        assert!(
            engine
                .check_network_request(
                    &Request::new(
                        "https://sub.example.com",
                        "https://sub.example.com",
                        "document",
                        ""
                    )
                    .unwrap()
                )
                .exception
                .is_some()
        );
    }

    #[test]
    fn implicit_all() {
        {
            let engine = Engine::new_with_list_text("||example.com^");
            assert!(
                engine
                    .check_network_request(
                        &Request::new("https://example.com", "https://example.com", "document", "")
                            .unwrap()
                    )
                    .should_block()
            );
        }
        {
            let engine = Engine::new_with_list_text("||example.com^$first-party");
            assert!(
                engine
                    .check_network_request(
                        &Request::new("https://example.com", "https://example.com", "document", "")
                            .unwrap()
                    )
                    .should_block()
            );
        }
        {
            let engine = Engine::new_with_list_text("||example.com^$script");
            assert!(
                !engine
                    .check_network_request(
                        &Request::new("https://example.com", "https://example.com", "document", "")
                            .unwrap()
                    )
                    .should_block()
            );
        }
        {
            let engine = Engine::new_with_list_text("||example.com^$~script");
            assert!(
                !engine
                    .check_network_request(
                        &Request::new("https://example.com", "https://example.com", "document", "")
                            .unwrap()
                    )
                    .should_block()
            );
        }
        {
            let engine = Engine::new_with_list_text(
                ["||example.com^$document", "@@||example.com^$generichide"].join("\n"),
            );
            assert!(
                engine
                    .check_network_request(
                        &Request::new("https://example.com", "https://example.com", "document", "")
                            .unwrap()
                    )
                    .should_block()
            );
        }
        {
            let mut filter_set = FilterSet::new(false);
            filter_set.add_filter_list(
                "example.com".to_string(),
                ParseOptions {
                    format: FilterFormat::Hosts,
                    ..Default::default()
                },
            );
            let engine = Engine::new_with_filter_set(filter_set);
            assert!(
                engine
                    .check_network_request(
                        &Request::new("https://example.com", "https://example.com", "document", "")
                            .unwrap()
                    )
                    .should_block()
            );
        }
        {
            let engine = Engine::new_with_list_text("||example.com/path");
            assert!(
                !engine
                    .check_network_request(
                        &Request::new(
                            "https://example.com/path",
                            "https://example.com/path",
                            "document",
                            ""
                        )
                        .unwrap()
                    )
                    .should_block()
            );
        }
        {
            let engine = Engine::new_with_list_text("||example.com/path^");
            assert!(
                !engine
                    .check_network_request(
                        &Request::new(
                            "https://example.com/path",
                            "https://example.com/path",
                            "document",
                            ""
                        )
                        .unwrap()
                    )
                    .should_block()
            );
        }
    }

    #[test]
    fn explicit_all() {
        let engine = Engine::new_with_list_text("*$all,domain=rarvinzp.click|ytrqcxat.click");
        for content_type in [
            "script",
            "document",
            "subdocument",
            "font",
            "xmlhttprequest",
        ] {
            assert!(
                engine
                    .check_network_request(
                        &Request::new(
                            "https://example.com",
                            "https://rarvinzp.click",
                            content_type,
                            ""
                        )
                        .unwrap()
                    )
                    .should_block()
            );
        }
    }

    #[test]
    fn generichide() {
        let filters = [
            "##.donotblock",
            "##a[href=\"generic.com\"]",
            "@@||example.com$generichide",
            "example.com##.block",
            "@@||example2.com/test.html$generichide",
            "example2.com##.block",
        ]
        .join("\n");
        let url_results = [
            ("https://example.com", vec![".block"], true),
            ("https://example.com/test.html", vec![".block"], true),
            (
                "https://example2.com",
                vec![".block", "a[href=\"generic.com\"]"],
                false,
            ),
            ("https://example2.com/test.html", vec![".block"], true),
        ];

        let engine = Engine::new_with_list_text(filters);

        url_results
            .into_iter()
            .for_each(|(url, expected_result, expected_generichide)| {
                let result = engine.url_cosmetic_resources(url);
                assert_eq!(
                    result.hide_selectors,
                    expected_result
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<HashSet<_>>()
                );
                assert_eq!(result.generichide, expected_generichide);
            });
    }

    #[test]
    fn important_redirect() {
        let mut filter_set = FilterSet::new(true);
        filter_set.add_filters([
            "||addthis.com^$important,3p,domain=~missingkids.com|~missingkids.org|~sainsburys.jobs|~sitecore.com|~amd.com",
            "||addthis.com/*/addthis_widget.js$script,redirect=addthis.com/addthis_widget.js",
        ], Default::default());
        let mut engine = Engine::new_with_filter_set(filter_set);

        engine.use_resources([Resource::simple(
            "addthis.com/addthis_widget.js",
            MimeType::ApplicationJavascript,
            "window.addthis = undefined",
        )]);

        let request = Request::new("https://s7.addthis.com/js/250/addthis_widget.js?pub=resto", "https://www.rhmodern.com/catalog/product/product.jsp?productId=prod14970086&categoryId=cat7150028", "script", "").unwrap();
        let result = engine.check_network_request(&request);

        assert!(result.redirect.is_some());
    }

    #[test]
    fn check_match_case_regex_filtering() {
        {
            // match case without regex is discarded
            let engine = Engine::new_with_list_text("ad.png$match-case");
            let request = Request::new(
                "https://example.com/ad.png",
                "https://example.com",
                "image",
                "",
            )
            .unwrap();
            assert!(!engine.check_network_request(&request).should_block());
        }
        {
            // /^https:\/\/[0-9a-z]{3,}\.[-a-z]{10,}\.(?:li[fv]e|top|xyz)\/[a-z]{8}\/\?utm_campaign=\w{40,}/$doc,match-case,domain=life|live|top|xyz
            let engine = Engine::new_with_list_text(
                r#"/^https:\/\/[0-9a-z]{3,}\.[-a-z]{10,}\.(?:li[fv]e|top|xyz)\/[a-z]{8}\/\?utm_campaign=\w{40,}/$doc,match-case,domain=life|live|top|xyz"#,
            );
            let request = Request::new("https://www.exampleaaa.xyz/testtest/?utm_campaign=aaaaaaaaaabbbbbbbbbbccccccccccdddddddddd", "https://www.exampleaaa.xyz/testtest/?utm_campaign=aaaaaaaaaabbbbbbbbbbccccccccccdddddddddd", "document", "").unwrap();
            assert!(engine.check_network_request(&request).should_block());
        }
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /^https?:\/\/((?!www)[a-z]{3,}|\d{2})?\.?[-0-9a-z]{6,}\.[a-z]{2,6}\/(?:[a-z]{6,8}\/)?\/?\?u=[0-9a-z]{7}&o=[0-9a-z]{7}/$doc,frame,match-case,domain=buzz|com|de|fun|guru|info|life|live|mobi|online|pw|site|space|top|us|xyz
            let engine = Engine::from_rules_debug([r#"/^https?:\/\/((?!www)[a-z]{3,}|\d{2})?\.?[-0-9a-z]{6,}\.[a-z]{2,6}\/(?:[a-z]{6,8}\/)?\/?\?u=[0-9a-z]{7}&o=[0-9a-z]{7}/$doc,frame,match-case,domain=buzz|com|de|fun|guru|info|life|live|mobi|online|pw|site|space|top|us|xyz"#], Default::default());
            let request = Request::new("https://example.com/aaaaaa/?u=aaaaaaa&o=bbbbbbb", "https://example.com/aaaaaa/?u=aaaaaaa&o=bbbbbbb", "document", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /^https:\/\/(?:www\d\.)?[-a-z]{6,}\.(?:com|info|net|org)\/(?=[-_a-zA-Z]{0,42}\d)(?=[-_0-9a-z]{0,42}[A-Z])[-_0-9a-zA-Z]{43}\/\?cid=[-_0-9a-zA-Z]{16,36}(?:&qs\d=\S+)?&sid=[_0-9a-f]{1,32}$/$doc,match-case,domain=com|info|net|org
            let engine = Engine::from_rules_debug([r#"/^https:\/\/(?:www\d\.)?[-a-z]{6,}\.(?:com|info|net|org)\/(?=[-_a-zA-Z]{0,42}\d)(?=[-_0-9a-z]{0,42}[A-Z])[-_0-9a-zA-Z]{43}\/\?cid=[-_0-9a-zA-Z]{16,36}(?:&qs\d=\S+)?&sid=[_0-9a-f]{1,32}$/$doc,match-case,domain=com|info|net|org"#], Default::default());
            let request = Request::new("https://www3.example.com/aaaaaaaaaabbbbbbbbbbccccccccccddddddddddAA5/?cid=aaaaaaaaaabbbbbb&qs5=\n&sid=a", "https://www3.example.com/aaaaaaaaaabbbbbbbbbbccccccccccddddddddddAA5/?cid=aaaaaaaaaabbbbbb&qs5=\n&sid=a", "document", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /^https:\/\/(?:www\d\.)?[-a-z]{6,}\.(?:com|info|net|org)\/(?=[-_a-zA-Z]{0,42}\d)(?=[-_0-9a-z]{0,42}[A-Z])[-_0-9a-zA-Z]{43}\/\?sid=[_0-9a-f]{1,32}(?:&qs\d=\S+)?&cid=[-_0-9a-zA-Z]{16,36}$/$doc,match-case,domain=com|info|net|org
            let engine = Engine::from_rules_debug([r#"/^https:\/\/(?:www\d\.)?[-a-z]{6,}\.(?:com|info|net|org)\/(?=[-_a-zA-Z]{0,42}\d)(?=[-_0-9a-z]{0,42}[A-Z])[-_0-9a-zA-Z]{43}\/\?cid=[-_0-9a-zA-Z]{16,36}(?:&qs\d=\S+)?&sid=[_0-9a-f]{1,32}$/$doc,match-case,domain=com|info|net|org"#], Default::default());
            let request = Request::new("https://www3.example.com/aaaaaaaaaabbbbbbbbbbccccccccccddddddddddAA5/?sid=1&qs1=\n&cid=aaaaaaaaaabbbbbb", "https://www3.example.com/aaaaaaaaaabbbbbbbbbbccccccccccddddddddddAA5/?sid=1&qs1=\n&cid=aaaaaaaaaabbbbbb", "document", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        {
            // /^http:\/\/[a-z]{5}\.[a-z]{5}\.com\/[a-z]{10}\.apk$/$doc,match-case,domain=com
            let engine = Engine::new_with_list_text(
                r#"/^http:\/\/[a-z]{5}\.[a-z]{5}\.com\/[a-z]{10}\.apk$/$doc,match-case,domain=com"#,
            );
            let request = Request::new(
                "http://abcde.abcde.com/aaaaabbbbb.apk",
                "http://abcde.abcde.com/aaaaabbbbb.apk",
                "document",
                "",
            )
            .unwrap();
            assert!(engine.check_network_request(&request).should_block());
        }
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /\/[A-Z]\/[-0-9a-z]{5,}\.com\/(?:[0-9a-f]{2}\/){3}[0-9a-f]{32}\.js$/$script,1p,match-case
            let engine = Engine::from_rules_debug([r#"/\/[A-Z]\/[-0-9a-z]{5,}\.com\/(?:[0-9a-f]{2}\/){3}[0-9a-f]{32}\.js$/$script,1p,match-case"#], Default::default());
            let request = Request::new("/A/aaaaa.com/aa/bb/cc/aaaaaaaabbbbbbbbccccccccdddddddd.js", "/A/aaaaa.com/aa/bb/cc/aaaaaaaabbbbbbbbccccccccdddddddd.js", "script", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /^https?:\/\/(?:[a-z]{2}\.)?[0-9a-z]{7,16}\.com\/[a-z](?=[a-z]{0,25}[0-9A-Z])[0-9a-zA-Z]{3,26}\/(?:[1-5]\d{4}|[3-9]\d{3})\??(?:_=\d+|v=\d)?$/$frame,script,xhr,popup,3p,match-case
            let engine = Engine::from_rules_debug([r#"/^https?:\/\/(?:[a-z]{2}\.)?[0-9a-z]{7,16}\.com\/[a-z](?=[a-z]{0,25}[0-9A-Z])[0-9a-zA-Z]{3,26}\/(?:[1-5]\d{4}|[3-9]\d{3})\??(?:_=\d+|v=\d)?$/$frame,script,xhr,popup,3p,match-case"#], Default::default());
            let request = Request::new("https://aa.example.com/aAaaa/12222", "https://aa.example.net/aAaaa/12222", "frame", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /^https?:\/\/(?:[a-z]{2}\.)?[0-9a-z]{7,16}\.website\/[a-z](?=[a-z]{0,25}[0-9A-Z])[0-9a-zA-Z]{3,26}\/(?:[1-5]\d{4}|[3-9]\d{3})\??(?:_=\d+|v=\d)?$/$frame,script,xhr,popup,3p,match-case
            let engine = Engine::from_rules_debug([r#"/^https?:\/\/(?:[a-z]{2}\.)?[0-9a-z]{7,16}\.website\/[a-z](?=[a-z]{0,25}[0-9A-Z])[0-9a-zA-Z]{3,26}\/(?:[1-5]\d{4}|[3-9]\d{3})\??(?:_=\d+|v=\d)?$/$frame,script,xhr,popup,3p,match-case"#], Default::default());
            let request = Request::new("https://aa.example.website/aAaaa/12222", "https://aa.example.website/aAaaa/12222", "frame", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /^https?:\/\/[a-z]{8,15}\.top(\/(?:\d{1,5}|0NaN|articles?|browse|index|movie|news|pages?|static|view|web|wiki)){1,4}(?:\.html|\/)$/$frame,3p,match-case
            let engine = Engine::from_rules_debug([r#"/^https?:\/\/[a-z]{8,15}\.top(\/(?:\d{1,5}|0NaN|articles?|browse|index|movie|news|pages?|static|view|web|wiki)){1,4}(?:\.html|\/)$/$frame,3p,match-case"#], Default::default());
            let request = Request::new("https://examples.top/articles.html", "https://examples.top/articles.html", "frame", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        {
            // /^https?:\/\/[a-z]{8,15}\.top\/[a-z]{4,}\.json$/$xhr,3p,match-case
            let engine = Engine::new_with_list_text(
                r#"/^https?:\/\/[a-z]{8,15}\.top\/[a-z]{4,}\.json$/$xhr,3p,match-case"#,
            );
            let request = Request::new(
                "https://examples.top/abcd.json",
                "https://examples.com/abcd.json",
                "xhr",
                "",
            )
            .unwrap();
            assert!(engine.check_network_request(&request).should_block());
        }
        // fails - inferring unescaped `$` inside regex pattern
        /*{
            // /^https?:\/\/[a-z]{8,15}\.top\/[-a-z]{4,}\.css\?aHR0c[\/0-9a-zA-Z]{33,}=?=?$/$css,3p,match-case
            let engine = Engine::from_rules_debug([r#"/^https?:\/\/[a-z]{8,15}\.top\/[-a-z]{4,}\.css\?aHR0c[\/0-9a-zA-Z]{33,}=?=?$/$css,3p,match-case"#], Default::default());
            let request = Request::new("https://examples.top/abcd.css?aHR0c/aaaaaaaaaaAAAAAAAAAA000000000012==", "https://examples.com/abcd.css?aHR0c/aaaaaaaaaaAAAAAAAAAA000000000012==", "stylesheet", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - inferring unescaped `$` inside regex pattern
        /*{
            // /^https?:\/\/[a-z]{8,15}\.top\/[a-z]{4,}\.png\?aHR0c[\/0-9a-zA-Z]{33,}=?=?$/$image,3p,match-case
            let engine = Engine::from_rules_debug([r#"/^https?:\/\/[a-z]{8,15}\.top\/[a-z]{4,}\.png\?aHR0c[\/0-9a-zA-Z]{33,}=?=?$/$image,3p,match-case"#], Default::default());
            let request = Request::new("https://examples.top/abcd.png?aHR0c/aaaaaaaaaaAAAAAAAAAA000000000012==", "https://examples.com/abcd.png?aHR0c/aaaaaaaaaaAAAAAAAAAA000000000012==", "image", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /^https?:\/\/[a-z]{8,15}\.xyz(\/(?:\d{1,5}|0NaN|articles?|browse|index|movie|news|pages?|static|view|web|wiki)){1,4}(?:\.html|\/)$/$frame,3p,match-case
            let engine = Engine::from_rules_debug([r#"/^https?:\/\/[a-z]{8,15}\.xyz(\/(?:\d{1,5}|0NaN|articles?|browse|index|movie|news|pages?|static|view|web|wiki)){1,4}(?:\.html|\/)$/$frame,3p,match-case"#], Default::default());
            let request = Request::new("https://examples.xyz/articles.html", "https://examples.xyz/articles.html", "frame", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        {
            // /^https?:\/\/cdn\.[a-z]{4,6}\.xyz\/app\.js$/$script,3p,match-case
            let engine = Engine::new_with_list_text(
                r#"/^https?:\/\/cdn\.[a-z]{4,6}\.xyz\/app\.js$/$script,3p,match-case"#,
            );
            let request = Request::new(
                "https://cdn.abcde.xyz/app.js",
                "https://cdn.abcde.com/app.js",
                "script",
                "",
            )
            .unwrap();
            assert!(engine.check_network_request(&request).should_block());
        }
        // fails - because of non-supported look around operator in rust regex https://github.com/rust-lang/regex/issues/127#issuecomment-154713666
        /*{
            // /^https:\/\/a\.[-0-9a-z]{4,16}\.(?:club|com?|cyou|info|net|ru|site|top?|xxx|xyz)\/(?=[a-z]{0,6}[0-9A-Z])[0-9a-zA-Z]{7}\.js$/$script,match-case
            let engine = Engine::from_rules_debug([r#"/^https:\/\/a\.[-0-9a-z]{4,16}\.(?:club|com?|cyou|info|net|ru|site|top?|xxx|xyz)\/(?=[a-z]{0,6}[0-9A-Z])[0-9a-zA-Z]{7}\.js$/$script,match-case"#], Default::default());
            let request = Request::new("https://a.abcd.club/aaaaaaA.js", "https://a.abcd.club/aaaaaaA.js", "script", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        {
            // /^https:\/\/cdn\.jsdelivr\.net\/npm\/[-a-z_]{4,22}@latest\/dist\/script\.min\.js$/$script,3p,match-case
            let engine = Engine::new_with_list_text(
                r#"/^https:\/\/cdn\.jsdelivr\.net\/npm\/[-a-z_]{4,22}@latest\/dist\/script\.min\.js$/$script,3p,match-case"#,
            );
            let request = Request::new(
                "https://cdn.jsdelivr.net/npm/abcd@latest/dist/script.min.js",
                "https://cdn.jsdelivr.com/npm/abcd@latest/dist/script.min.js",
                "script",
                "",
            )
            .unwrap();
            assert!(engine.check_network_request(&request).should_block());
        }
        // fails - inferring unescaped `$` inside regex pattern
        /*{
            // /^https?:\/\/[-.0-9a-z]+\/script\.js$/$script,1p,strict3p,match-case
            let engine = Engine::from_rules_debug([r#"/^https?:\/\/[-.0-9a-z]+\/script\.js$/$script,1p,strict3p,match-case"#], Default::default());
            let request = Request::new("https://www.example.com/script.js", "https://www.abc.com/script.js", "script", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - unicode not supported in network filter
        /*{
            let engine = Engine::from_rules_debug([r#"/tesT߶/$domain=example.com"#], Default::default());
            let request = Request::new("https://example.com/tesT߶", "https://example.com", "script", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
        // fails - unicode not supported in network filter
        /*{
            let engine = Engine::from_rules_debug([r#"/tesT߶/$domain=example.com"#], Default::default());
            let request = Request::new("https://example-tesT߶.com/tesT", "https://example.com", "script", "").unwrap();
            assert!(engine.check_network_request(&request).matched);
        }*/
    }

    #[test]
    fn scriptlet_permissions() {
        use crate::resources::{PermissionMask, ResourceType};
        const UBO_PERM: PermissionMask = PermissionMask::from_bits(0b00000001);
        const BRAVE_PERM: PermissionMask = PermissionMask::from_bits(0b00000011);

        let resources = [
            Resource::simple(
                "refresh-defuser.js",
                MimeType::ApplicationJavascript,
                "refresh-defuser",
            ),
            Resource {
                name: "trusted-set-cookie.js".to_string(),
                aliases: vec![],
                kind: ResourceType::Mime(MimeType::ApplicationJavascript),
                content: BASE64_STANDARD.encode("trusted-set-cookie"),
                dependencies: vec![],
                permission: UBO_PERM,
            },
            Resource {
                name: "brave-fix.js".to_string(),
                aliases: vec![],
                kind: ResourceType::Mime(MimeType::ApplicationJavascript),
                content: BASE64_STANDARD.encode("brave-fix"),
                dependencies: vec![],
                permission: BRAVE_PERM,
            },
        ];

        let mut filter_set = FilterSet::new(false);
        filter_set.add_filters(
            [
                "sub1.example.com##+js(refresh-defuser)",
                "sub2.example.com##+js(trusted-set-cookie)",
                "sub3.example.com##+js(brave-fix)",
            ],
            Default::default(),
        );
        filter_set.add_filters(
            [
                "sub4.example.com##+js(refresh-defuser)",
                "sub5.example.com##+js(trusted-set-cookie)",
                "sub6.example.com##+js(brave-fix)",
            ],
            ParseOptions {
                permissions: UBO_PERM,
                ..Default::default()
            },
        );
        filter_set.add_filters(
            [
                "sub7.example.com##+js(refresh-defuser)",
                "sub8.example.com##+js(trusted-set-cookie)",
                "sub9.example.com##+js(brave-fix)",
            ],
            ParseOptions {
                permissions: BRAVE_PERM,
                ..Default::default()
            },
        );

        let mut engine = Engine::new_with_filter_set(filter_set);
        engine.use_resources(resources);

        fn wrap_try(scriptlet_content: &str) -> String {
            format!("try {{\n{scriptlet_content}\n}} catch ( e ) {{ }}\n")
        }

        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub1.example.com")
                .injected_script,
            wrap_try("refresh-defuser")
        );
        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub2.example.com")
                .injected_script,
            ""
        );
        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub3.example.com")
                .injected_script,
            ""
        );

        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub4.example.com")
                .injected_script,
            wrap_try("refresh-defuser")
        );
        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub5.example.com")
                .injected_script,
            wrap_try("trusted-set-cookie")
        );
        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub6.example.com")
                .injected_script,
            ""
        );

        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub7.example.com")
                .injected_script,
            wrap_try("refresh-defuser")
        );
        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub8.example.com")
                .injected_script,
            wrap_try("trusted-set-cookie")
        );
        assert_eq!(
            engine
                .url_cosmetic_resources("https://sub9.example.com")
                .injected_script,
            wrap_try("brave-fix")
        );
    }

    #[test]
    fn quoted_scriptlet_args() {
        use crate::resources::{MimeType, ResourceType};

        let resources = [
            Resource {
                name: "trusted-set-local-storage-item.js".into(),
                aliases: vec![],
                kind: ResourceType::Mime(MimeType::ApplicationJavascript),
                content: BASE64_STANDARD.encode("function trustedSetLocalStorageItem(key = '', value = '') { setLocalStorageItemFn('local', true, key, value); }"),
                dependencies: vec![],
                permission: Default::default(),
            },
        ];

        let mut filter_set = FilterSet::new(false);
        filter_set.add_filters([
            r#"dailymail.co.uk##+js(trusted-set-local-storage-item, mol.ads.cmp.tcf.cache, '{"getTCData":{"cmpId":27,"cmpVersion":3,"gdprApplies":true,"tcfPolicyVersion":2,"tcString":"CPyz5QAPyz5QAAbADCENC6CgAAAAAAAAAAwIAAASjAJINW4gCLMscGaQEIoEAIgjCQggUAAFAILRAQAODgp2VgE6MIkAAAUARABAhwAQAQCAAASABCAAJAAwQAAAiAQAAAAQCAAAMCAILACgAAAABANAhRCgAECQAyIAIpTAgKgSCAFsKAAADJCQCAKgMAKARGgEACIIARGAAACwMAgBICFggABMQbBAAMACAESoBoCTEwBACDQFgBkADLAGzAPsA_ACAAEFAIwASYAp8BaAFpAOqAfIBDoCJgEiAKRAXIAyMBk4DlAI_gSKEQEwBkADLAGzAPsA_ACAAEYAJMAU8A6oB8gEOgJEAUiAuQBkYDJwHKAR_AkU.f_gAAagAAAAA","eventStatus":"useractioncomplete","cmpStatus":"loaded","isServiceSpecific":true,"useNonStandardStacks":false,"publisherCC":"GB","purposeOneTreatment":false,"addtlConsent":"1~","acmVersion":2,"molGvlVersion":"186.gb.web","nrvString":"1~","nrvVersion":1,"repromptVersion":5},"getStoredRepromptVersion":5,"hasUserConsentedToAll":false,"hasUserDissentedToAll":true,"getConsentDegree":"no","getValidTCData":{"cmpId":27,"cmpVersion":3,"gdprApplies":true,"tcfPolicyVersion":2,"tcString":"CPyz5QAPyz5QAAbADCENC6CgAAAAAAAAAAwIAAASjAJINW4gCLMscGaQEIoEAIgjCQggUAAFAILRAQAODgp2VgE6MIkAAAUARABAhwAQAQCAAASABCAAJAAwQAAAiAQAAAAQCAAAMCAILACgAAAABANAhRCgAECQAyIAIpTAgKgSCAFsKAAADJCQCAKgMAKARGgEACIIARGAAACwMAgBICFggABMQbBAAMACAESoBoCTEwBACDQFgBkADLAGzAPsA_ACAAEFAIwASYAp8BaAFpAOqAfIBDoCJgEiAKRAXIAyMBk4DlAI_gSKEQEwBkADLAGzAPsA_ACAAEYAJMAU8A6oB8gEOgJEAUiAuQBkYDJwHKAR_AkU.f_gAAagAAAAA","listenerId":1,"eventStatus":"useractioncomplete","cmpStatus":"loaded","isServiceSpecific":true,"useNonStandardStacks":false,"publisherCC":"GB","purposeOneTreatment":false,"addtlConsent":"1~","acmVersion":2,"molGvlVersion":"186.gb.web","nrvString":"1~","nrvVersion":1,"repromptVersion":5}}')"#,
            // invalid - unclosed quoted arg
            r#"example.com##+js(trusted-set-local-storage-item, "test)"#,
            // invalid - closing quote does not surround the argument
            r#"example.com##+js(trusted-set-local-storage-item, "test"test, 3)"#,
        ], Default::default());

        let mut engine = Engine::new_with_filter_set(filter_set);
        engine.use_resources(resources);

        assert_eq!(engine.url_cosmetic_resources("https://dailymail.co.uk").injected_script, r#"function trustedSetLocalStorageItem(key = '', value = '') { setLocalStorageItemFn('local', true, key, value); }
try {
trustedSetLocalStorageItem("mol.ads.cmp.tcf.cache", "{\"getTCData\":{\"cmpId\":27,\"cmpVersion\":3,\"gdprApplies\":true,\"tcfPolicyVersion\":2,\"tcString\":\"CPyz5QAPyz5QAAbADCENC6CgAAAAAAAAAAwIAAASjAJINW4gCLMscGaQEIoEAIgjCQggUAAFAILRAQAODgp2VgE6MIkAAAUARABAhwAQAQCAAASABCAAJAAwQAAAiAQAAAAQCAAAMCAILACgAAAABANAhRCgAECQAyIAIpTAgKgSCAFsKAAADJCQCAKgMAKARGgEACIIARGAAACwMAgBICFggABMQbBAAMACAESoBoCTEwBACDQFgBkADLAGzAPsA_ACAAEFAIwASYAp8BaAFpAOqAfIBDoCJgEiAKRAXIAyMBk4DlAI_gSKEQEwBkADLAGzAPsA_ACAAEYAJMAU8A6oB8gEOgJEAUiAuQBkYDJwHKAR_AkU.f_gAAagAAAAA\",\"eventStatus\":\"useractioncomplete\",\"cmpStatus\":\"loaded\",\"isServiceSpecific\":true,\"useNonStandardStacks\":false,\"publisherCC\":\"GB\",\"purposeOneTreatment\":false,\"addtlConsent\":\"1~\",\"acmVersion\":2,\"molGvlVersion\":\"186.gb.web\",\"nrvString\":\"1~\",\"nrvVersion\":1,\"repromptVersion\":5},\"getStoredRepromptVersion\":5,\"hasUserConsentedToAll\":false,\"hasUserDissentedToAll\":true,\"getConsentDegree\":\"no\",\"getValidTCData\":{\"cmpId\":27,\"cmpVersion\":3,\"gdprApplies\":true,\"tcfPolicyVersion\":2,\"tcString\":\"CPyz5QAPyz5QAAbADCENC6CgAAAAAAAAAAwIAAASjAJINW4gCLMscGaQEIoEAIgjCQggUAAFAILRAQAODgp2VgE6MIkAAAUARABAhwAQAQCAAASABCAAJAAwQAAAiAQAAAAQCAAAMCAILACgAAAABANAhRCgAECQAyIAIpTAgKgSCAFsKAAADJCQCAKgMAKARGgEACIIARGAAACwMAgBICFggABMQbBAAMACAESoBoCTEwBACDQFgBkADLAGzAPsA_ACAAEFAIwASYAp8BaAFpAOqAfIBDoCJgEiAKRAXIAyMBk4DlAI_gSKEQEwBkADLAGzAPsA_ACAAEYAJMAU8A6oB8gEOgJEAUiAuQBkYDJwHKAR_AkU.f_gAAagAAAAA\",\"listenerId\":1,\"eventStatus\":\"useractioncomplete\",\"cmpStatus\":\"loaded\",\"isServiceSpecific\":true,\"useNonStandardStacks\":false,\"publisherCC\":\"GB\",\"purposeOneTreatment\":false,\"addtlConsent\":\"1~\",\"acmVersion\":2,\"molGvlVersion\":\"186.gb.web\",\"nrvString\":\"1~\",\"nrvVersion\":1,\"repromptVersion\":5}}")
} catch ( e ) { }
"#.to_owned());

        assert_eq!(
            engine
                .url_cosmetic_resources("https://example.com")
                .injected_script,
            ""
        );
    }

    #[test]
    fn method_option_blocks_post_xhr_only() {
        let engine =
            Engine::new_with_list_text("||perplexity.ai/rest/metrics/collect^$xhr,1p,method=post");
        let url = "https://perplexity.ai/rest/metrics/collect?foo=bar";
        let source = "https://perplexity.ai/page";

        let post = Request::new(url, source, "xhr", "post").unwrap();
        assert!(
            engine.check_network_request(&post).should_block(),
            "POST xhr 1p should match"
        );

        let get = Request::new(url, source, "xhr", "get").unwrap();
        assert!(
            !engine.check_network_request(&get).should_block(),
            "GET xhr should not match method=post rule"
        );

        let post_3p = Request::new(url, "https://other.com/page", "xhr", "post").unwrap();
        assert!(
            !engine.check_network_request(&post_3p).should_block(),
            "POST xhr 3p should not match 1p rule"
        );

        let post_no_method = Request::new(url, source, "xhr", "").unwrap();
        assert!(
            !engine.check_network_request(&post_no_method).should_block(),
            "missing method should not match method=post rule"
        );
    }

    #[test]
    fn method_option_exception_head_get() {
        let engine = Engine::new_with_list_text(
            "||tracker.example.com^$xhr\n@@*$xhr,method=head|get,domain=app.axenthost.com,3p",
        );
        let url = "https://tracker.example.com/pixel";
        let source = "https://app.axenthost.com/page";

        for method in ["get", "head"] {
            let request = Request::new(url, source, "xhr", method).unwrap();
            assert!(
                !engine.check_network_request(&request).should_block(),
                "{method} xhr should be excepted"
            );
        }

        let post = Request::new(url, source, "xhr", "post").unwrap();
        assert!(
            engine.check_network_request(&post).should_block(),
            "POST xhr should still be blocked"
        );
    }

    #[test]
    fn rejected_network_matcher_preparation_is_transactional() {
        let engine = Engine::new_with_list_text(
            r#"/alpha[0-9]+\.js/$script
/Beta[0-9]+\.js/$script,match-case"#,
        );
        let request = Request::new(
            "https://cdn.example/alpha42.js",
            "https://site.example/",
            "script",
            "get",
        )
        .unwrap();

        let rejected = engine.prepare_and_freeze_network_matcher(NetworkMatcherPreparationLimits {
            max_regexes: 1,
            max_pattern_bytes: usize::MAX,
            max_patterns_per_regex: usize::MAX,
            max_pattern_bytes_per_regex: usize::MAX,
            max_regex_size_bytes: usize::MAX,
            max_regex_dfa_size_bytes: usize::MAX,
            max_filter_checks_per_request: usize::MAX,
        });
        assert_eq!(
            rejected,
            Err(NetworkMatcherPreparationError::RegexLimitExceeded)
        );
        assert!(
            matches!(
                engine.try_check_prepared_network_request(&request),
                Err(crate::blocker::PreparedNetworkMatcherError::Unprepared)
            ),
            "a rejected attempt must not publish a partially prepared matcher",
        );

        let prepared = engine
            .prepare_and_freeze_network_matcher(NetworkMatcherPreparationLimits {
                max_regexes: 2,
                max_pattern_bytes: usize::MAX,
                max_patterns_per_regex: usize::MAX,
                max_pattern_bytes_per_regex: usize::MAX,
                max_regex_size_bytes: usize::MAX,
                max_regex_dfa_size_bytes: usize::MAX,
                max_filter_checks_per_request: usize::MAX,
            })
            .expect("a full retry must see and prepare both regexes");
        assert_eq!(prepared.regexes, 2);
        assert!(
            engine
                .try_check_prepared_network_request(&request)
                .expect("the successful retry must publish a frozen matcher")
                .should_block()
        );
        assert_eq!(
            engine.prepare_and_freeze_network_matcher(NetworkMatcherPreparationLimits {
                max_regexes: 2,
                max_pattern_bytes: usize::MAX,
                max_patterns_per_regex: usize::MAX,
                max_pattern_bytes_per_regex: usize::MAX,
                max_regex_size_bytes: usize::MAX,
                max_regex_dfa_size_bytes: usize::MAX,
                max_filter_checks_per_request: 1,
            }),
            Err(NetworkMatcherPreparationError::PreparedLimitMismatch)
        );
    }

    #[test]
    fn source_independent_match_budget_exhaustion_is_fail_open() {
        let engine =
            Engine::new_with_list_text(concat!("*$script\n", "@@||cdn.example^$script\n",));
        engine
            .prepare_and_freeze_network_matcher(NetworkMatcherPreparationLimits {
                max_regexes: 32,
                max_pattern_bytes: 1024 * 1024,
                max_patterns_per_regex: 32,
                max_pattern_bytes_per_regex: 64 * 1024,
                max_regex_size_bytes: 256 * 1024,
                max_regex_dfa_size_bytes: 64 * 1024,
                max_filter_checks_per_request: 1,
            })
            .unwrap();
        let request =
            Request::new_source_independent("https://cdn.example/ad.js", "script", "get").unwrap();
        assert!(
            matches!(
                engine.try_check_prepared_source_independent_network_request(&request),
                Err(crate::blocker::PreparedNetworkMatcherError::CandidateBudgetExhausted)
            ),
            "a partial scan must not publish a block before exception coverage is complete",
        );
    }

    #[test]
    fn a_source_sensitive_exception_does_not_lift_a_block_without_the_source() {
        let engine = Engine::new_with_list_text(concat!(
            "*$script\n",
            "@@*$script,domain=publisher.example\n",
        ));
        engine
            .prepare_and_freeze_network_matcher(NetworkMatcherPreparationLimits {
                max_regexes: 32,
                max_pattern_bytes: 1024 * 1024,
                max_patterns_per_regex: 32,
                max_pattern_bytes_per_regex: 64 * 1024,
                max_regex_size_bytes: 256 * 1024,
                max_regex_dfa_size_bytes: 64 * 1024,
                max_filter_checks_per_request: 2,
            })
            .unwrap();
        let request =
            Request::new_source_independent("https://cdn.example/ad.js", "script", "get").unwrap();
        let result = engine
            .try_check_prepared_source_independent_network_request(&request)
            .expect("the blocking candidate fits; the source-sensitive exception is skipped");
        assert!(
            result.should_block(),
            "an exception that needs the initiating document cannot apply without it",
        );
        assert!(
            result.filter.is_some() && result.exception.is_none(),
            "the block is reported as the exact rule that matched",
        );
    }

    #[test]
    fn source_independent_nonmatching_exception_is_scanned_once() {
        let engine = Engine::new_with_list_text(concat!(
            "*$script\n",
            "@@*$image,domain=publisher.example\n",
        ));
        engine
            .prepare_and_freeze_network_matcher(NetworkMatcherPreparationLimits {
                max_regexes: 32,
                max_pattern_bytes: 1024 * 1024,
                max_patterns_per_regex: 32,
                max_pattern_bytes_per_regex: 64 * 1024,
                max_regex_size_bytes: 256 * 1024,
                max_regex_dfa_size_bytes: 64 * 1024,
                max_filter_checks_per_request: 2,
            })
            .unwrap();
        let request =
            Request::new_source_independent("https://cdn.example/ad.js", "script", "get").unwrap();
        assert!(
            engine
                .try_check_prepared_source_independent_network_request(&request)
                .expect("one block and one nonmatching exception candidate fit")
                .should_block(),
        );
    }

    #[test]
    fn source_independent_exception_partition_preserves_exact_matches() {
        let engine = Engine::new_with_list_text(concat!("*$script\n", "@@*$script\n",));
        engine
            .prepare_and_freeze_network_matcher(NetworkMatcherPreparationLimits {
                max_regexes: 32,
                max_pattern_bytes: 1024 * 1024,
                max_patterns_per_regex: 32,
                max_pattern_bytes_per_regex: 64 * 1024,
                max_regex_size_bytes: 256 * 1024,
                max_regex_dfa_size_bytes: 64 * 1024,
                max_filter_checks_per_request: 2,
            })
            .unwrap();
        let request =
            Request::new_source_independent("https://cdn.example/ad.js", "script", "get").unwrap();
        let result = engine
            .try_check_prepared_source_independent_network_request(&request)
            .expect("the blocking and exact exception candidates fit");
        assert!(!result.should_block());
        assert!(
            result.filter.is_some() && result.exception.is_some(),
            "a source-independent exception remains an exact reported match",
        );
    }

    #[test]
    fn exact_match_budget_is_shared_with_exception_scans() {
        let rules = concat!("*$script\n", "@@*$script,domain=publisher.example\n",);
        let request = Request::new(
            "https://cdn.example/ad.js",
            "https://publisher.example/",
            "script",
            "get",
        )
        .unwrap();
        let prepare = |engine: &Engine, max_filter_checks_per_request| {
            engine
                .prepare_and_freeze_network_matcher(NetworkMatcherPreparationLimits {
                    max_regexes: 32,
                    max_pattern_bytes: 1024 * 1024,
                    max_patterns_per_regex: 32,
                    max_pattern_bytes_per_regex: 64 * 1024,
                    max_regex_size_bytes: 256 * 1024,
                    max_regex_dfa_size_bytes: 64 * 1024,
                    max_filter_checks_per_request,
                })
                .unwrap();
        };

        let exhausted = Engine::new_with_list_text(rules);
        prepare(&exhausted, 1);
        assert!(
            matches!(
                exhausted.try_check_prepared_network_request(&request),
                Err(crate::blocker::PreparedNetworkMatcherError::CandidateBudgetExhausted)
            ),
            "an exact block must not escape before its exception scan fits the shared budget",
        );

        let complete = Engine::new_with_list_text(rules);
        prepare(&complete, 2);
        assert!(
            !complete
                .try_check_prepared_network_request(&request)
                .expect("the exact block and exception fit the shared budget")
                .should_block()
        );
    }

    #[test]
    fn serialization_retains_unknown_attribution_matching() {
        let limits = NetworkMatcherPreparationLimits {
            max_regexes: 1_024,
            max_pattern_bytes: 1024 * 1024,
            max_patterns_per_regex: 1_024,
            max_pattern_bytes_per_regex: 256 * 1024,
            max_regex_size_bytes: 1024 * 1024,
            max_regex_dfa_size_bytes: 1024 * 1024,
            max_filter_checks_per_request: 4_096,
        };
        let rules = concat!(
            "||tracker.example^\n",
            "@@/tracker[0-9]+\\.js/$domain=publisher.example\n",
            "||critical.example^$important\n",
            "@@||critical.example^$domain=publisher.example\n",
        );
        let original = Engine::new_with_list_text(rules);
        original.prepare_and_freeze_network_matcher(limits).unwrap();

        let mut restored = Engine::default();
        restored.deserialize(&original.serialize()).unwrap();
        restored.prepare_and_freeze_network_matcher(limits).unwrap();

        for (url, expected_block) in [
            ("https://tracker.example/tracker42.js", true),
            ("https://critical.example/ad.js", true),
        ] {
            let request = Request::new_source_independent(url, "script", "get").unwrap();
            let expected = original
                .try_check_prepared_source_independent_network_request(&request)
                .unwrap();
            let actual = restored
                .try_check_prepared_source_independent_network_request(&request)
                .unwrap();
            assert_eq!(expected.should_block(), expected_block);
            assert_eq!(actual.should_block(), expected_block);
            assert_eq!(actual.important, expected.important);
        }
    }
}
