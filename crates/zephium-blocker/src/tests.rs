use proptest::prelude::*;

use super::*;

fn id(value: &str) -> SourceId {
    SourceId::new(value).unwrap()
}

fn source(value: &str) -> FilterSource {
    FilterSource::new(id("test"), SourceFormat::Standard, value.to_string())
}

#[cfg(feature = "runtime-exact")]
fn request<'a>(url: &'a str, source_url: &'a str) -> NetworkRequest<'a> {
    NetworkRequest::new(url, source_url, ResourceType::Script, RequestMethod::Get)
}

#[cfg(feature = "runtime")]
fn source_independent_request(url: &str) -> NetworkRequest<'_> {
    NetworkRequest::source_independent(url, ResourceType::Script, RequestMethod::Get)
}

#[cfg(feature = "runtime")]
#[test]
fn a_catalog_with_scriptlets_and_no_hiding_rules_keeps_its_program() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source("||ads.example^\nexample.com##+js(set, a.b, true)")],
        )
        .unwrap();
    let cosmetics = rules.cosmetic_policy().expect("a cosmetic policy");
    assert_eq!(cosmetics.report().accepted, 0);
    assert!(cosmetics
        .scriptlet_program()
        .unwrap()
        .contains("zephiumSetConstant("));
}

#[cfg(feature = "runtime-exact")]
#[test]
fn the_official_canary_refuses_lists_that_stop_blocking() {
    let official = |easylist: &str| {
        Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![
                    FilterSource::new(id("easylist"), SourceFormat::Standard, easylist.to_owned()),
                    FilterSource::new(
                        id("easyprivacy"),
                        SourceFormat::Standard,
                        "||googletagmanager.com^".to_owned(),
                    ),
                ],
            )
            .unwrap()
    };
    assert_eq!(
        official("||doubleclick.com^").verify_official_canary(),
        Ok(())
    );
    // The list's block is gone.
    assert!(official("||unrelated.example^")
        .verify_official_canary()
        .is_err());
    // A wildcard exception scoped to one site fails open when no page is known.
    assert!(
        official("||doubleclick.com^\n@@*$script,3p,from=elsewhere.example")
            .verify_official_canary()
            .is_err()
    );
    // A list that blocks the harmless request is refused too.
    assert!(official("||doubleclick.com^\n||example.com^")
        .verify_official_canary()
        .is_err());
    // Lists that are not official are not held to it.
    assert_eq!(
        Compiler::default()
            .compile(CompileTarget::Runtime, vec![source("||unrelated.example^")])
            .unwrap()
            .verify_official_canary(),
        Ok(())
    );
}

#[test]
fn source_ids_are_canonical_and_bounded() {
    assert!(SourceId::new("easylist-default").is_ok());
    assert_eq!(SourceId::new(""), Err(SourceIdError::Empty));
    assert_eq!(
        SourceId::new("EasyList"),
        Err(SourceIdError::InvalidCharacter)
    );
    assert_eq!(SourceId::new("a/b"), Err(SourceIdError::InvalidCharacter));
    assert!(matches!(
        SourceId::new("a".repeat(129)),
        Err(SourceIdError::TooLong { .. })
    ));
}

#[cfg(feature = "runtime-exact")]
#[test]
fn runtime_blocks_and_honors_exceptions() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source("||ads.example^\n@@||ads.example/allowed.js$script")],
        )
        .unwrap();

    assert_eq!(
        rules
            .evaluate(request(
                "https://ads.example/tracker.js",
                "https://site.example/"
            ))
            .unwrap()
            .action(),
        NetworkAction::Block
    );
    let allowed = rules
        .evaluate(request(
            "https://ads.example/allowed.js",
            "https://site.example/",
        ))
        .unwrap();
    assert_eq!(allowed.action(), NetworkAction::Allow);
    assert!(allowed.matched_exception());
}

#[cfg(feature = "runtime-exact")]
#[test]
fn runtime_request_attribution_shape_is_enforced() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source("||ads.example^$script")],
        )
        .unwrap();
    let exact = request("https://ads.example/ad.js", "https://publisher.example/");
    let independent = source_independent_request("https://ads.example/ad.js");

    assert_eq!(rules.evaluate(independent), Err(MatchError::InvalidRequest));
    assert_eq!(
        rules.evaluate_source_independent(exact),
        Err(MatchError::InvalidRequest)
    );
}

#[cfg(feature = "runtime-exact")]
#[test]
fn approximate_attribution_never_evaluates_source_sensitive_rules() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source(concat!(
                "||domain-rule.example^$domain=publisher.example\n",
                "||party-rule.example^$third-party\n",
                "||generic-rule.example^\n",
            ))],
        )
        .unwrap();

    let domain_request = request(
        "https://domain-rule.example/ad.js",
        "https://publisher.example/",
    );
    assert_eq!(
        rules.evaluate(domain_request).unwrap().action(),
        NetworkAction::Block
    );
    assert_eq!(
        rules
            .evaluate_source_independent(source_independent_request(
                "https://domain-rule.example/ad.js"
            ))
            .unwrap()
            .action(),
        NetworkAction::Allow
    );

    let party_request = request(
        "https://party-rule.example/ad.js",
        "https://publisher.example/",
    );
    assert_eq!(
        rules.evaluate(party_request).unwrap().action(),
        NetworkAction::Block
    );
    assert_eq!(
        rules
            .evaluate_source_independent(source_independent_request(
                "https://party-rule.example/ad.js"
            ))
            .unwrap()
            .action(),
        NetworkAction::Allow
    );

    let generic_request = request(
        "https://generic-rule.example/ad.js",
        "https://publisher.example/",
    );
    assert_eq!(
        rules.evaluate(generic_request).unwrap().action(),
        NetworkAction::Block
    );
    assert_eq!(
        rules
            .evaluate_source_independent(source_independent_request(
                "https://generic-rule.example/ad.js"
            ))
            .unwrap()
            .action(),
        NetworkAction::Block
    );
}

#[cfg(feature = "runtime-exact")]
#[test]
fn unknown_attribution_never_bypasses_a_possible_scoped_exception() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source(concat!(
                "||tracker.example^\n",
                "@@||tracker.example^$domain=publisher.example\n",
            ))],
        )
        .unwrap();
    let protected = request(
        "https://tracker.example/ad.js",
        "https://publisher.example/",
    );
    let unprotected = request("https://tracker.example/ad.js", "https://other.example/");

    assert_eq!(
        rules.evaluate(protected).unwrap().action(),
        NetworkAction::Allow
    );
    assert_eq!(
        rules.evaluate(unprotected).unwrap().action(),
        NetworkAction::Block
    );
    assert_eq!(
        rules
            .evaluate_source_independent(source_independent_request(
                "https://tracker.example/ad.js"
            ))
            .unwrap()
            .action(),
        NetworkAction::Allow
    );
}

#[cfg(feature = "runtime")]
#[test]
fn source_independent_matching_preserves_target_semantics_and_fails_open_on_source_semantics() {
    fn action(rules: &str, url: &str, method: RequestMethod) -> NetworkAction {
        Compiler::default()
            .compile(CompileTarget::Runtime, vec![source(rules)])
            .unwrap()
            .evaluate_source_independent(NetworkRequest::source_independent(
                url,
                ResourceType::Script,
                method,
            ))
            .unwrap()
            .action()
    }

    let target = "https://tracker.example/tracker42.js";
    for scoped_exception in [
        "@@||tracker.example^$domain=publisher.example",
        "@@||tracker.example^$domain=~excluded.example",
        "@@||tracker.example^$third-party",
        "@@||tracker.example^$first-party",
        "@@/tracker[0-9]+\\.js/$domain=publisher.example",
        "@@*$script,domain=publisher.example",
    ] {
        let rules = format!("||tracker.example^\n{scoped_exception}");
        assert_eq!(
            action(&rules, target, RequestMethod::Get),
            NetworkAction::Allow,
            "unknown source attribution could activate {scoped_exception}"
        );
    }

    assert_eq!(
        action(
            "||tracker.example^\n@@|https://tracker.example^",
            target,
            RequestMethod::Get,
        ),
        NetworkAction::Allow,
        "target protocol predicates remain exactly evaluable"
    );
    assert_eq!(
        action(
            "||tracker.example^\n@@|http://tracker.example^",
            target,
            RequestMethod::Get,
        ),
        NetworkAction::Block
    );
    assert_eq!(
        action(
            "||tracker.example^\n@@||tracker.example^$method=post",
            target,
            RequestMethod::Post,
        ),
        NetworkAction::Allow,
        "request methods remain exactly evaluable"
    );
    assert_eq!(
        action(
            "||tracker.example^\n@@||tracker.example^$method=post",
            target,
            RequestMethod::Get,
        ),
        NetworkAction::Block
    );
    assert_eq!(
        action(
            "||tracker.example^$important\n@@||tracker.example^$domain=publisher.example",
            target,
            RequestMethod::Get,
        ),
        NetworkAction::Block,
        "source-independent important rules retain their priority"
    );
    assert_eq!(
        action(
            concat!(
                "||tracker.example^\n",
                "||tracker.example^$badfilter\n",
                "||effective.example^\n",
            ),
            target,
            RequestMethod::Get,
        ),
        NetworkAction::Allow,
        "$badfilter must suppress source-independent matching too"
    );
}

#[cfg(feature = "runtime")]
#[test]
fn runtime_coverage_reports_unavailable_webview2_contexts() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source(concat!(
                "||document.example^$document\n",
                "||frame.example^$subdocument\n",
                "||socket.example^$websocket\n",
                "||object.example^$object\n",
                "||script.example^$script\n",
                "||generic.example^\n",
                "||domain.example^$script,domain=publisher.example\n",
            ))],
        )
        .unwrap();
    let report = rules.report();

    assert_eq!(report.accepted_rules(), 7);
    assert!(report.native_blocking_rule_entries() > 0);
    // Four native-only contexts plus the source-sensitive rule are omitted.
    assert_eq!(report.runtime_omitted_rules(), 5);
    // The generic rule still covers exact subresource contexts but not every
    // resource type represented by its default mask. It and the explicit
    // script rule also have partial reachability because V1 intentionally
    // excludes service/shared-worker source kinds.
    assert_eq!(report.runtime_approximated_rules(), 2);
    assert_eq!(report.runtime_resource_approximated_rules(), 1);
    assert_eq!(report.runtime_source_kind_approximated_rules(), 2);
}

#[cfg(feature = "runtime")]
#[test]
fn runtime_coverage_counts_excluded_worker_source_kinds_without_double_counting() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source(concat!(
                "||script.example^$script\n",
                "||fetch.example^$xmlhttprequest\n",
                "||image.example^$image\n",
                "||generic.example^\n",
            ))],
        )
        .unwrap();
    let report = rules.report();

    assert_eq!(report.accepted_rules(), 4);
    assert_eq!(report.runtime_omitted_rules(), 0);
    assert_eq!(report.runtime_resource_approximated_rules(), 1);
    assert_eq!(report.runtime_source_kind_approximated_rules(), 4);
    assert_eq!(
        report.runtime_approximated_rules(),
        4,
        "the generic rule is counted once across both partial dimensions"
    );
}

#[cfg(feature = "runtime")]
#[test]
fn runtime_rejects_negated_method_sets_instead_of_narrowing_unknown_methods() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source(concat!(
                "||tracker.example^\n",
                "@@||tracker.example^$method=~GET\n",
                "@@||tracker.example^$method=POST|~HEAD\n",
                "@@||tracker.example^$method=GET|DELETE\n",
            ))],
        )
        .unwrap();
    assert_eq!(
        rules
            .report()
            .dropped_for(InputDropReason::UnsupportedMethodPredicate),
        3
    );

    for method in [
        RequestMethod::Connect,
        RequestMethod::Delete,
        RequestMethod::Get,
        RequestMethod::Head,
        RequestMethod::Options,
        RequestMethod::Patch,
        RequestMethod::Post,
        RequestMethod::Put,
        RequestMethod::Other,
    ] {
        let decision = rules
            .evaluate_source_independent(NetworkRequest::source_independent(
                "https://tracker.example/ad.js",
                ResourceType::Script,
                method,
            ))
            .unwrap();
        assert_eq!(decision.action(), NetworkAction::Block, "{method:?}");
    }
}

#[cfg(feature = "runtime")]
#[test]
fn malformed_domain_predicates_cannot_create_broad_fail_open_exceptions() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source(concat!(
                "||tracker.example^\n",
                "@@||tracker.example^$domain=*\n",
                "@@||tracker.example^$domain=foo/bar\n",
                "@@||tracker.example^$domain=\u{200d}.example\n",
                "@@||tracker.example^$from=-example.com\n",
            ))],
        )
        .unwrap();
    assert_eq!(
        rules
            .report()
            .dropped_for(InputDropReason::InvalidDomainPredicate),
        4
    );
    assert_eq!(
        rules
            .evaluate_source_independent(source_independent_request(
                "https://tracker.example/ad.js"
            ))
            .unwrap()
            .action(),
        NetworkAction::Block
    );
}

#[cfg(feature = "webkit")]
#[test]
fn forbidden_active_rules_are_removed_and_reported() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::WebKit,
            vec![source(concat!(
                "||allowed.example^\n",
                "||redirect.example^$redirect=noop.js\n",
                "||csp.example^$csp=script-src 'none'\n",
                "||params.example^$removeparam=utm_source\n",
                "@@||hide.example^$generichide\n",
                "example.com##+js(noop)\n",
                "example.com##.ad:has-text(sponsored)\n",
                "example.com##.ad:style(color: red)\n",
            ))],
        )
        .unwrap();
    let report = rules.report();

    assert_eq!(report.accepted_rules(), 1);
    assert_eq!(report.candidate_rules(), 8);
    assert_eq!(report.rejected_rules(), 7);
    assert_eq!(report.dropped_for(InputDropReason::ForbiddenRedirect), 1);
    assert_eq!(report.dropped_for(InputDropReason::ForbiddenCsp), 1);
    assert_eq!(report.dropped_for(InputDropReason::ForbiddenRemoveParam), 1);
    assert_eq!(report.dropped_for(InputDropReason::ForbiddenGenericHide), 1);
    assert_eq!(
        report.dropped_for(InputDropReason::UnsupportedCosmeticRule),
        3
    );
    let webkit = rules.webkit().unwrap();
    assert!(!webkit.json().contains("redirect"));
    assert!(!webkit.json().contains("scriptlet"));
}

#[cfg(feature = "webkit")]
#[test]
fn webkit_artifacts_contain_only_audited_network_actions() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::WebKit,
            vec![source(concat!(
                "||ads.example^$script\n",
                "@@||ads.example/allowed.js$script\n",
            ))],
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(rules.webkit().unwrap().json()).unwrap();
    let actions: Vec<_> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["action"]["type"].as_str().unwrap())
        .collect();

    assert!(actions.contains(&"block"));
    assert!(actions.contains(&"ignore-previous-rules"));
    assert!(actions
        .iter()
        .all(|action| matches!(*action, "block" | "ignore-previous-rules")));
}

#[cfg(feature = "webkit")]
#[test]
fn webkit_separator_rules_preserve_byte_and_end_of_url_branches() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::WebKit,
            vec![source(concat!(
                "||example.com^$script\n",
                "@@||allowed.example^$script\n",
                "||internal.example/path^segment$script\n",
            ))],
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(rules.webkit().unwrap().json()).unwrap();
    let native_rules = json.as_array().unwrap();
    let filters: Vec<_> = native_rules
        .iter()
        .map(|rule| rule["trigger"]["url-filter"].as_str().unwrap())
        .collect();

    assert!(filters.iter().any(|filter| {
        filter.ends_with("example\\.com[^A-Za-z0-9_.%-]") && !filter.contains("allowed\\.example")
    }));
    assert!(filters
        .iter()
        .any(|filter| filter.ends_with("allowed\\.example[^A-Za-z0-9_.%-]")));
    assert!(filters
        .iter()
        .any(|filter| filter.ends_with("/path[^A-Za-z0-9_.%-]segment")));
    assert!(filters.iter().all(|filter| !filter.contains("(?:")));

    let coverage = rules.report().webkit().unwrap();
    assert_eq!(coverage.converted_input_rules(), 3);
    assert_eq!(coverage.blocking_rule_entries(), 2);
    assert_eq!(coverage.emitted_rules(), 3);
}

#[cfg(feature = "webkit")]
#[test]
fn webkit_drops_one_pathological_expanded_url_filter_without_poisoning_the_list() {
    let input = format!(
        "||allowed.example^$script\n||oversized.example/{}$script\n",
        "*".repeat(5_000)
    );
    let rules = Compiler::default()
        .compile(CompileTarget::WebKit, vec![source(&input)])
        .unwrap();
    let coverage = rules.report().webkit().unwrap();
    assert_eq!(coverage.dropped_for(WebKitDropReason::UrlFilterTooLarge), 1);
    assert_eq!(coverage.blocking_rule_entries(), 1);
}

#[cfg(feature = "webkit")]
#[test]
fn webkit_conversion_loss_is_explicit() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::WebKit,
            vec![source(concat!(
                "||portable.example^$script\n",
                "/ad-[0-9]{2}/$script\n",
                "google.*##.ad\n",
                "example.com##.广告\n",
            ))],
        )
        .unwrap();
    let coverage = rules.report().webkit().unwrap();

    assert_eq!(coverage.accepted_input_rules(), 2);
    assert_eq!(coverage.converted_input_rules(), 1);
    assert_eq!(coverage.omitted_input_rules(), 1);
    assert_eq!(
        rules
            .report()
            .dropped_for(InputDropReason::InvalidNetworkRule),
        0
    );
    assert_eq!(
        rules
            .report()
            .dropped_for(InputDropReason::UnsupportedCosmeticRule),
        2
    );
    assert_eq!(
        coverage.dropped_for(WebKitDropReason::FullRegularExpression),
        1
    );
    assert_eq!(
        coverage.converted_input_rules()
            + coverage
                .dropped()
                .iter()
                .map(|entry| entry.count())
                .sum::<usize>(),
        coverage.accepted_input_rules()
    );
}

#[cfg(feature = "webkit")]
#[test]
fn owned_webkit_converter_preserves_modern_resource_semantics() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::WebKit,
            vec![source(concat!(
                "||default.example/ads\n",
                "||types.example^$script,ping\n",
                "||xhr.example^$xmlhttprequest\n",
                "||doc.example^$document\n",
                "||method.example^$method=POST\n",
                "||important.example^$important\n",
                "||tag.example^$tag=off\n",
            ))],
        )
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(rules.webkit().unwrap().json()).unwrap();
    let native_rules = json.as_array().unwrap();

    let resources_for = |host: &str| {
        native_rules
            .iter()
            .find(|rule| {
                rule["trigger"]["url-filter"]
                    .as_str()
                    .is_some_and(|filter| filter.contains(host))
            })
            .and_then(|rule| rule["trigger"]["resource-type"].as_array())
            .map(|resources| {
                resources
                    .iter()
                    .map(|resource| resource.as_str().unwrap())
                    .collect::<Vec<_>>()
            })
    };

    let default = resources_for("default").unwrap();
    assert!(default.contains(&"child-document"));
    assert!(default.contains(&"fetch"));
    assert!(default.contains(&"ping"));
    assert!(default.contains(&"websocket"));
    assert!(!default.contains(&"top-document"));
    assert!(!default.contains(&"raw"));
    assert_eq!(resources_for("types").unwrap(), ["ping", "script"]);
    assert_eq!(resources_for("xhr").unwrap(), ["fetch"]);
    assert_eq!(resources_for("doc").unwrap(), ["top-document"]);
    assert!(resources_for("method").is_none());
    assert!(resources_for("important").is_none());
    assert!(resources_for("tag").is_none());

    let coverage = rules.report().webkit().unwrap();
    assert_eq!(coverage.accepted_input_rules(), 6);
    assert_eq!(coverage.converted_input_rules(), 4);
    assert_eq!(coverage.omitted_input_rules(), 2);
    assert_eq!(coverage.dropped_for(WebKitDropReason::RequestMethod), 1);
    assert_eq!(coverage.dropped_for(WebKitDropReason::ImportantPriority), 1);
    assert_eq!(coverage.resource_approximated_input_rules(), 1);
    assert_eq!(coverage.attribution_approximated_input_rules(), 0);
    assert_eq!(coverage.approximated_input_rules(), 1);
    assert_eq!(
        rules.report().dropped_for(InputDropReason::UnsupportedTag),
        1
    );
}

#[cfg(feature = "webkit")]
#[test]
fn owned_webkit_converter_accepts_modern_only_resource_types() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::WebKit,
            vec![source(concat!(
                "||ping.example^$ping\n",
                "||socket.example^$websocket\n",
                "||other.example^$other\n",
                "||object.example^$object\n",
            ))],
        )
        .unwrap();
    let webkit = rules.webkit().unwrap();

    assert!(webkit.json().contains("\"resource-type\":[\"ping\"]"));
    assert!(webkit.json().contains("\"resource-type\":[\"websocket\"]"));
    assert!(webkit.json().contains("\"resource-type\":[\"other\"]"));
    assert!(webkit
        .json()
        .contains("\"resource-type\":[\"svg-document\"]"));
    assert!(!webkit.json().contains("csp-report"));
    let coverage = rules.report().webkit().unwrap();
    assert_eq!(coverage.converted_input_rules(), 4);
    assert_eq!(coverage.resource_approximated_input_rules(), 2);
    assert_eq!(coverage.attribution_approximated_input_rules(), 0);
    assert_eq!(coverage.approximated_input_rules(), 2);
}

#[cfg(feature = "runtime-exact")]
#[test]
fn runtime_treats_fetch_as_xml_http_request() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source("||api.example^$xmlhttprequest")],
        )
        .unwrap();
    for resource_type in [ResourceType::Fetch, ResourceType::XmlHttpRequest] {
        assert_eq!(
            rules
                .evaluate(NetworkRequest::new(
                    "https://api.example/data",
                    "https://site.example/",
                    resource_type,
                    RequestMethod::Get,
                ))
                .unwrap()
                .action(),
            NetworkAction::Block
        );
    }
    assert_eq!(
        rules
            .evaluate(NetworkRequest::new(
                "https://api.example/data",
                "https://site.example/",
                ResourceType::Other,
                RequestMethod::Get,
            ))
            .unwrap()
            .action(),
        NetworkAction::Allow
    );
}

#[cfg(feature = "webkit")]
#[test]
fn exact_webkit_bytes_define_the_native_cache_identity() {
    let first = Compiler::default()
        .compile(CompileTarget::WebKit, vec![source("||one.example^$script")])
        .unwrap();
    let second = Compiler::default()
        .compile(CompileTarget::WebKit, vec![source("||two.example^$script")])
        .unwrap();
    assert_ne!(
        first.webkit().unwrap().digest(),
        second.webkit().unwrap().digest()
    );
}

#[cfg(all(feature = "runtime", feature = "webkit"))]
#[test]
fn target_artifacts_do_not_retain_the_other_backend() {
    let runtime = Compiler::default()
        .compile(CompileTarget::Runtime, vec![source("||ads.example^")])
        .unwrap();
    assert!(runtime.webkit().is_none());

    let webkit = Compiler::default()
        .compile(CompileTarget::WebKit, vec![source("||ads.example^")])
        .unwrap();
    assert!(matches!(
        webkit
            .evaluate_source_independent(source_independent_request("https://ads.example/ad.js",)),
        Err(MatchError::WrongArtifactTarget)
    ));
}

#[cfg(feature = "webkit")]
#[test]
fn canonical_output_is_independent_of_source_order() {
    let first = FilterSource::new(
        id("a"),
        SourceFormat::Standard,
        "||a.example^$image,script\n".to_string(),
    );
    let second = FilterSource::new(
        id("b"),
        SourceFormat::Standard,
        "b.example##.advert\n".to_string(),
    );
    let left = Compiler::default()
        .compile(CompileTarget::WebKit, vec![first, second])
        .unwrap();
    let right = Compiler::default()
        .compile(
            CompileTarget::WebKit,
            vec![
                FilterSource::new(
                    id("b"),
                    SourceFormat::Standard,
                    "b.example##.advert\n".to_string(),
                ),
                FilterSource::new(
                    id("a"),
                    SourceFormat::Standard,
                    "||a.example^$image,script\n".to_string(),
                ),
            ],
        )
        .unwrap();

    assert_eq!(left.digest(), right.digest());
    assert_eq!(
        left.webkit().unwrap().json(),
        right.webkit().unwrap().json()
    );
    assert_eq!(left.report(), right.report());
    assert_eq!(
        left.report()
            .sources()
            .iter()
            .map(|report| report.id().as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[cfg(all(feature = "runtime-exact", feature = "webkit"))]
#[test]
fn badfilter_semantics_are_preserved_and_covered() {
    let rules = concat!(
        "||disabled.example^\n",
        "||disabled.example^$badfilter\n",
        "||effective.example^\n",
    );
    let runtime = Compiler::default()
        .compile(CompileTarget::Runtime, vec![source(rules)])
        .unwrap();

    assert_eq!(
        runtime
            .evaluate(request(
                "https://disabled.example/ad.js",
                "https://site.example/"
            ))
            .unwrap()
            .action(),
        NetworkAction::Allow
    );
    let webkit = Compiler::default()
        .compile(CompileTarget::WebKit, vec![source(rules)])
        .unwrap();
    assert_eq!(
        webkit
            .report()
            .webkit()
            .unwrap()
            .dropped_for(WebKitDropReason::BadFilterControl),
        1
    );
    assert_eq!(
        webkit
            .report()
            .webkit()
            .unwrap()
            .dropped_for(WebKitDropReason::SuppressedByRuleSemantics),
        1
    );
}

#[test]
fn enabled_artifacts_require_a_native_blocking_rule_entry() {
    for target in [
        cfg!(feature = "runtime").then_some(CompileTarget::Runtime),
        cfg!(feature = "webkit").then_some(CompileTarget::WebKit),
    ]
    .into_iter()
    .flatten()
    {
        assert!(matches!(
            Compiler::default().compile(
                target,
                vec![source(
                    "||disabled.example^\n||disabled.example^$badfilter\n"
                )],
            ),
            Err(CompileError::NoNativeBlockingRules)
        ));
        assert!(matches!(
            Compiler::default().compile(target, vec![source("@@||allowed.example^")]),
            Err(CompileError::NoNativeBlockingRules)
        ));
    }

    #[cfg(feature = "runtime")]
    {
        assert!(matches!(
            Compiler::default().compile(
                CompileTarget::Runtime,
                vec![source("||domain-only.example^$domain=publisher.example")],
            ),
            Err(CompileError::NoNativeBlockingRules)
        ));
        assert!(matches!(
            Compiler::default().compile(
                CompileTarget::Runtime,
                vec![source("||document-only.example^$document")],
            ),
            Err(CompileError::NoNativeBlockingRules)
        ));
    }
}

#[cfg(feature = "runtime")]
#[test]
fn hosts_sources_compile_without_localhost_entries() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![FilterSource::new(
                id("hosts"),
                SourceFormat::Hosts,
                "# comment\n0.0.0.0 ads.example\n127.0.0.1 localhost\n".to_string(),
            )],
        )
        .unwrap();

    assert_eq!(rules.report().accepted_rules(), 1);
    assert_eq!(
        rules
            .evaluate_source_independent(source_independent_request(
                "https://ads.example/banner.js",
            ))
            .unwrap()
            .action(),
        NetworkAction::Block
    );
}

#[cfg(feature = "runtime")]
#[test]
fn every_expensive_boundary_is_limited() {
    let limits = CompileLimits::new(CompileLimitValues {
        max_sources: 1,
        max_source_bytes: 32,
        max_total_source_bytes: 32,
        max_line_bytes: 16,
        max_rules: 1,
        max_physical_lines: 8,
        max_webkit_rules: 8,
        max_webkit_json_bytes: 1_024,
        max_request_url_bytes: 32,
        max_source_url_bytes: 32,
    })
    .unwrap();
    let compiler = Compiler::new(limits);

    assert!(matches!(
        compiler.compile(
            CompileTarget::Runtime,
            vec![source("||a.example^\n||b.example^")]
        ),
        Err(CompileError::TooManyRules { .. })
    ));
    assert!(matches!(
        compiler.compile(CompileTarget::Runtime, vec![source("0123456789abcdefg")]),
        Err(CompileError::LineTooLong { .. })
    ));
    assert!(matches!(
        compiler.compile(
            CompileTarget::Runtime,
            vec![source("!\n!\n!\n!\n!\n!\n!\n!\n||a.example^")]
        ),
        Err(CompileError::TooManyPhysicalLines { .. })
    ));

    let rules = compiler
        .compile(CompileTarget::Runtime, vec![source("||a.example^")])
        .unwrap();
    assert!(matches!(
        rules.evaluate_source_independent(source_independent_request(
            "https://a.example/this-is-too-long",
        )),
        Err(MatchError::RequestUrlTooLong { .. })
    ));
}

#[cfg(feature = "runtime")]
#[test]
fn generated_representative_corpus_fits_the_runtime_matcher_hard_limits() {
    use std::fmt::Write as _;

    let mut corpus = String::with_capacity(20_000 * 64);
    for index in 0..20_000 {
        writeln!(
            corpus,
            "||tracker-{index}.representative.corpus.invalid^$script"
        )
        .unwrap();
    }
    let rules = Compiler::default()
        .compile(CompileTarget::Runtime, vec![source(&corpus)])
        .expect("the generated corpus exceeded the synchronous matcher budget");
    assert!(rules.report().native_blocking_rule_entries() > 0);
}

#[cfg(feature = "runtime")]
#[test]
fn runtime_distinguishes_candidate_budget_exhaustion() {
    assert_eq!(
        crate::rules::map_prepared_match_error(
            adblock::blocker::PreparedNetworkMatcherError::Unprepared,
        ),
        MatchError::MatcherUnprepared
    );
    assert_eq!(
        crate::rules::map_prepared_match_error(
            adblock::blocker::PreparedNetworkMatcherError::Unavailable,
        ),
        MatchError::MatcherUnavailable
    );
    assert_eq!(
        crate::rules::map_prepared_match_error(
            adblock::blocker::PreparedNetworkMatcherError::CandidateBudgetExhausted,
        ),
        MatchError::CandidateBudgetExhausted
    );
}

#[test]
fn callers_cannot_weaken_audited_hard_limits() {
    let mut values = CompileLimitValues::default();
    values.max_request_url_bytes += 1;
    assert!(matches!(
        CompileLimits::new(values),
        Err(LimitConfigurationError::AboveHardMaximum {
            name: "max_request_url_bytes",
            ..
        })
    ));

    let mut values = CompileLimitValues::default();
    values.max_rules += 1;
    assert!(matches!(
        CompileLimits::new(values),
        Err(LimitConfigurationError::AboveHardMaximum {
            name: "max_rules",
            ..
        })
    ));
}

#[cfg(feature = "runtime")]
#[test]
fn enabled_compilation_requires_a_real_usable_catalog() {
    assert!(matches!(
        Compiler::default().compile(CompileTarget::Runtime, Vec::new()),
        Err(CompileError::NoSources)
    ));
    assert!(matches!(
        Compiler::default().compile(CompileTarget::Runtime, vec![source("example.com##.advert")]),
        Err(CompileError::NoUsableRules)
    ));
}

#[cfg(feature = "runtime")]
#[test]
fn compiled_rules_are_shareable_but_not_mutable() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CompiledRules>();

    let rules = Compiler::default()
        .compile(CompileTarget::Runtime, vec![source("||ads.example^")])
        .unwrap();
    let clone = rules.clone();
    assert_eq!(rules.digest(), clone.digest());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[cfg(feature = "webkit")]
    #[test]
    fn bounded_arbitrary_lists_never_escape_declared_accounting(
        text in ".{0,2048}"
    ) {
        let limits = CompileLimits::new(CompileLimitValues {
            max_sources: 1,
            max_source_bytes: 2_048,
            max_total_source_bytes: 2_048,
            max_line_bytes: 2_048,
            max_rules: 2_049,
            max_physical_lines: 2_049,
            max_webkit_rules: 4_098,
            max_webkit_json_bytes: 256 * 1024,
            max_request_url_bytes: 4_096,
            max_source_url_bytes: 4_096,
        }).unwrap();

        if let Ok(rules) = Compiler::new(limits)
            .compile(CompileTarget::WebKit, vec![source(&text)])
        {
            let report = rules.report();
            let dropped: usize = report.sources()[0]
                .dropped()
                .iter()
                .map(|entry| entry.count())
                .sum();
            prop_assert!(
                report.accepted_rules() + dropped
                    <= report.sources()[0].total_lines()
            );
            prop_assert!(report.native_blocking_rule_entries() > 0);
            prop_assert!(
                report.native_blocking_rule_entries() <= report.accepted_rules()
            );
            let webkit = report.webkit().unwrap();
            let webkit_dropped: usize = webkit
                .dropped()
                .iter()
                .map(|entry| entry.count())
                .sum();
            prop_assert_eq!(
                webkit.converted_input_rules() + webkit_dropped,
                webkit.accepted_input_rules()
            );
            prop_assert!(webkit.blocking_rule_entries() > 0);
            prop_assert!(
                webkit.blocking_rule_entries() <= webkit.converted_input_rules()
            );
        }
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn semantic_rule_changes_change_the_digest(
        left in "[a-z]{1,20}",
        right in "[a-z]{1,20}"
    ) {
        prop_assume!(left != right);
        let left = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![source(&format!("||{left}.example^"))],
            )
            .unwrap();
        let right = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![source(&format!("||{right}.example^"))],
            )
            .unwrap();
        prop_assert_ne!(left.digest(), right.digest());
    }
}

#[cfg(feature = "runtime-exact")]
#[test]
fn rules_inside_a_false_if_block_are_not_compiled() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![source(
                "!#if env_firefox\n||firefox-only.example^\n!#else\n||elsewhere.example^\n!#endif",
            )],
        )
        .unwrap();
    let action = |url: &str| {
        rules
            .evaluate(request(url, "https://page.example/"))
            .unwrap()
            .action()
    };
    assert_ne!(
        action("https://firefox-only.example/a.js"),
        NetworkAction::Block
    );
    assert_eq!(
        action("https://elsewhere.example/a.js"),
        NetworkAction::Block
    );
    assert_eq!(rules.report().sources()[0].ignored_lines(), 4);
}

#[cfg(feature = "runtime-exact")]
#[test]
fn the_built_in_rules_let_ad_recovery_sites_load_only_adsense_s_loader() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![
                source("||pagead2.googlesyndication.com^"),
                crate::builtin::source(),
            ],
        )
        .unwrap();
    let action = |url: &str, page: &str| rules.evaluate(request(url, page)).unwrap().action();
    let loader = "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js";
    for page in [
        "https://www.allrecipes.com/recipe/1/",
        "https://simplyrecipes.com/",
        "https://m.people.com/a/",
    ] {
        assert_ne!(action(loader, page), NetworkAction::Block, "{page}");
    }
    for page in ["https://news.example/", "https://notallrecipes.com/"] {
        assert_eq!(action(loader, page), NetworkAction::Block, "{page}");
    }
    assert_eq!(
        action(
            "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.jsx",
            "https://www.allrecipes.com/recipe/1/"
        ),
        NetworkAction::Block
    );
    assert_eq!(
        action(
            "https://pagead2.googlesyndication.com/pagead/managed/js/adsense/m1/show_ads_impl.js",
            "https://www.allrecipes.com/recipe/1/"
        ),
        NetworkAction::Block
    );
}

#[cfg(feature = "runtime-exact")]
#[test]
fn the_built_in_rules_let_two_ad_recovery_sites_fetch_criteo_s_probe() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::Runtime,
            vec![
                source("||ads.example^\n||criteo.net^$third-party"),
                crate::builtin::source(),
            ],
        )
        .unwrap();
    let action = |url: &str, page: &str, resource_type| {
        rules
            .evaluate(NetworkRequest::new(
                url,
                page,
                resource_type,
                RequestMethod::Get,
            ))
            .unwrap()
            .action()
    };
    let probe = "https://static.criteo.net/js/ld/publishertag.prebid.js";
    for page in [
        "https://www.mirror.co.uk/",
        "https://www.tomsguide.com/news/",
    ] {
        assert_ne!(
            action(probe, page, ResourceType::XmlHttpRequest),
            NetworkAction::Block,
            "{page}"
        );
        assert_eq!(
            action(probe, page, ResourceType::Script),
            NetworkAction::Block,
            "{page}"
        );
        assert_eq!(
            action(
                "https://static.criteo.net/js/ld/publishertag.js",
                page,
                ResourceType::XmlHttpRequest
            ),
            NetworkAction::Block,
            "{page}"
        );
    }
    assert_eq!(
        action(
            probe,
            "https://www.allrecipes.com/",
            ResourceType::XmlHttpRequest
        ),
        NetworkAction::Block
    );
}

#[cfg(feature = "webkit")]
fn pruned(text: String) -> &'static crate::webkit_prune::PrunedRules {
    Box::leak(Box::new(crate::webkit_prune::PrunedRules::parse(&text)))
}

#[cfg(feature = "webkit")]
fn webkit_with(input: &str, entries: &[String]) -> CompiledRules {
    Compiler::default()
        .with_webkit_pruned(pruned(entries.join("\n")))
        .compile(CompileTarget::WebKit, vec![source(input)])
        .unwrap()
}

/// The digest entry for the last rule `input` emits.
#[cfg(feature = "webkit")]
fn entry_for(input: &str) -> String {
    let compiled = webkit_with(input, &[]);
    let emitted: Vec<Box<serde_json::value::RawValue>> =
        serde_json::from_str(compiled.webkit().unwrap().json()).unwrap();
    crate::webkit_rule_digest(emitted.last().unwrap().get().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(feature = "webkit")]
const PRUNABLE: &str = "||tracker.example/collect/*/beacon.gif$image";

#[cfg(feature = "webkit")]
#[test]
fn a_listed_wildcard_block_is_pruned_and_counted_apart_from_unsupported_rules() {
    let input = format!("||kept.example^$script\n{PRUNABLE}\n");
    let compiled = webkit_with(&input, &[entry_for(PRUNABLE)]);
    let coverage = compiled.report().webkit().unwrap();
    assert_eq!(coverage.accepted_input_rules(), 2);
    assert_eq!(coverage.converted_input_rules(), 1);
    assert_eq!(coverage.pruned_input_rules(), 1);
    assert_eq!(coverage.omitted_input_rules(), 0);
    assert!(coverage.dropped().is_empty());
    let json = compiled.webkit().unwrap().json();
    assert!(json.contains("kept") && !json.contains("tracker"));
}

#[cfg(feature = "webkit")]
#[test]
fn the_built_in_rules_compile_for_webkit_after_the_lists() {
    let rules = Compiler::default()
        .compile(
            CompileTarget::WebKit,
            vec![
                source("||pagead2.googlesyndication.com^\n||criteo.net^$third-party"),
                crate::builtin::source(),
            ],
        )
        .unwrap();
    let coverage = rules.report().webkit().unwrap();
    assert_eq!(coverage.omitted_input_rules(), 0);
    let json = rules.webkit().unwrap().json();
    let exception = json.find("ignore-previous-rules").unwrap();
    assert!(json.find("\"block\"").unwrap() < exception);
    let emitted: Vec<serde_json::Value> = serde_json::from_str(json).unwrap();
    let loader = "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js";
    let probe = "https://static.criteo.net/js/ld/publishertag.prebid.js";
    let (mut loader_sites, mut probe_sites) = (Vec::new(), Vec::new());
    // A trailing `^` emits a pair of rules, so each exception is judged by
    // what its rules match together.
    let (mut loader_filters, mut probe_filters) = (Vec::new(), Vec::new());
    for rule in emitted
        .iter()
        .filter(|rule| rule["action"]["type"] == "ignore-previous-rules")
    {
        let pattern = rule["trigger"]["url-filter"].as_str().unwrap();
        let filter = regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .unwrap();
        let sites = rule["trigger"]["if-domain"]
            .as_array()
            .unwrap()
            .iter()
            .map(|domain| domain.as_str().unwrap());
        if pattern.contains("adsbygoogle") {
            loader_filters.push(filter);
            loader_sites.extend(sites);
        } else {
            // The probe's fetch, never the tag as a script.
            let types = rule["trigger"]["resource-type"].as_array().unwrap();
            assert_eq!(types, &["fetch"], "{types:?}");
            probe_filters.push(filter);
            probe_sites.extend(sites);
        }
    }
    let any =
        |filters: &[regex::Regex], url: &str| filters.iter().any(|filter| filter.is_match(url));
    assert!(any(&loader_filters, loader));
    assert!(any(&loader_filters, &format!("{loader}?client=ca-pub-1")));
    assert!(!any(&loader_filters, &format!("{loader}x")));
    assert!(any(&probe_filters, probe));
    assert!(!any(&probe_filters, &format!("{probe}x")));
    loader_sites.sort_unstable();
    loader_sites.dedup();
    probe_sites.sort_unstable();
    probe_sites.dedup();
    // A leading `*` makes WebKit apply the exception on every subdomain too.
    assert_eq!(
        loader_sites,
        [
            "*allrecipes.com",
            "*bild.de",
            "*mirror.co.uk",
            "*people.com",
            "*simplyrecipes.com",
            "*tomsguide.com"
        ]
    );
    assert_eq!(probe_sites, ["*mirror.co.uk", "*tomsguide.com"]);
}

#[test]
fn the_built_in_rules_are_exceptions_only() {
    let rules = include_str!("zephium_rules.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('!'));
    for rule in rules {
        assert!(rule.starts_with("@@"), "{rule}");
    }
}

#[cfg(feature = "webkit")]
#[test]
fn only_kept_rules_count_against_the_webkit_rule_limit() {
    let limits = CompileLimits::new(CompileLimitValues {
        max_sources: 1,
        max_source_bytes: 1_024,
        max_total_source_bytes: 1_024,
        max_line_bytes: 1_024,
        max_rules: 8,
        max_physical_lines: 8,
        max_webkit_rules: 1,
        max_webkit_json_bytes: 64 * 1024,
        max_request_url_bytes: 4_096,
        max_source_url_bytes: 4_096,
    })
    .unwrap();
    let input = format!("||kept.example^$script\n{PRUNABLE}\n");
    let compiled = Compiler::new(limits)
        .with_webkit_pruned(pruned(entry_for(PRUNABLE)))
        .compile(CompileTarget::WebKit, vec![source(&input)])
        .unwrap();
    assert_eq!(compiled.report().webkit().unwrap().pruned_input_rules(), 1);
}

#[cfg(feature = "webkit")]
#[test]
fn an_edited_rule_is_not_pruned() {
    // Case changes emit the same rule, since url-filters are case-insensitive.
    let edited = PRUNABLE.replace("beacon", "beacom");
    let compiled = webkit_with(&edited, &[entry_for(PRUNABLE)]);
    let coverage = compiled.report().webkit().unwrap();
    assert_eq!(coverage.pruned_input_rules(), 0);
    assert_eq!(coverage.converted_input_rules(), 1);
}

#[cfg(feature = "webkit")]
#[test]
fn an_entry_that_matches_no_rule_changes_nothing() {
    let input = format!("||kept.example^$script\n{PRUNABLE}\n");
    let without = webkit_with(&input, &[]);
    let stray = webkit_with(&input, &[entry_for("||elsewhere.example/a/*/b.gif$image")]);
    assert_eq!(
        without.webkit().unwrap().json(),
        stray.webkit().unwrap().json()
    );
    assert_eq!(stray.report().webkit().unwrap().pruned_input_rules(), 0);
}

#[cfg(feature = "webkit")]
#[test]
fn listed_exceptions_and_rules_without_a_wildcard_are_never_pruned() {
    let exception = "@@||tracker.example/collect/*/beacon.gif$image";
    let plain = "||plain.example/beacon.gif$image";
    let input = format!("{PRUNABLE}\n{exception}\n{plain}\n");
    let entries = [
        entry_for(&format!("{PRUNABLE}\n{exception}")),
        entry_for(plain),
    ];
    let compiled = webkit_with(&input, &entries);
    let coverage = compiled.report().webkit().unwrap();
    assert_eq!(coverage.pruned_input_rules(), 0);
    assert_eq!(coverage.converted_input_rules(), 3);
}

#[cfg(feature = "webkit")]
#[test]
fn release_tooling_can_compile_every_rule() {
    let input = format!("||kept.example^$script\n{PRUNABLE}\n");
    let compiled = Compiler::default()
        .with_webkit_pruned(pruned(entry_for(PRUNABLE)))
        .without_webkit_pruning()
        .compile(CompileTarget::WebKit, vec![source(&input)])
        .unwrap();
    assert_eq!(compiled.report().webkit().unwrap().pruned_input_rules(), 0);
    assert!(compiled.webkit().unwrap().json().contains("tracker"));
}

#[cfg(all(feature = "webkit", feature = "runtime"))]
#[test]
fn the_runtime_target_ignores_pruned_rules() {
    let input = format!("||kept.example^$script\n{PRUNABLE}\n");
    let compile = |entries: &[String]| {
        Compiler::default()
            .with_webkit_pruned(pruned(entries.join("\n")))
            .compile(CompileTarget::Runtime, vec![source(&input)])
            .unwrap()
    };
    let (with, without) = (compile(&[entry_for(PRUNABLE)]), compile(&[]));
    assert_eq!(with.digest(), without.digest());
    assert_eq!(with.report(), without.report());
}
