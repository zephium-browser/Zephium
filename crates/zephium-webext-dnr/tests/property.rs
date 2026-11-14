#![cfg(feature = "compile")]

use serde_json::{json, Value};
use zephium_webext_dnr::{
    evaluate, evaluate_candidates, evaluate_linear, parse_rules_json, validate_extension_rulesets,
    Action, Candidate, CompiledRuleset, Condition, DomainType, Extension, FrameAllow, Header,
    PreparedRule, Request, RequestMethod, ResourceType, Rule, RulesetKind, Stage,
    GUARANTEED_MINIMUM_STATIC_RULES, MAX_NUMBER_OF_REGEX_RULES,
};

struct Generator(u64);

impl Generator {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0 ^ (self.0 >> 29)
    }

    fn pick(&mut self, maximum: u64) -> u64 {
        self.next() % maximum
    }
}

fn generated_rules(seed: u64, kind: RulesetKind) -> Vec<Value> {
    let mut generator = Generator(seed);
    (1..=36)
        .map(|id| {
            let domain = format!("d{}.example.test", generator.pick(8));
            let token = format!("needle{}", generator.pick(8));
            let filter = generator.pick(12);
            let mut condition = match filter {
                0 => json!({"urlFilter":format!("||{domain}^")}),
                1 => json!({"urlFilter":token}),
                2 => json!({"urlFilter":format!("|https://{domain}/path")}),
                3 => json!({"regexFilter":"^https?://[^/]+/path[0-9]+"}),
                4 => json!({"urlFilter":format!("||{domain}/*needle*")}),
                5 => json!({"urlFilter":"*"}),
                6 => json!({"urlFilter":format!("{token}|")}),
                7 => json!({"urlFilter":"x^"}),
                8 => json!({"urlFilter":"CaseToken","isUrlFilterCaseSensitive":true}),
                9 => json!({"urlFilter":"aaa*aaa"}),
                10 => json!({"urlFilter":"||example.test"}),
                _ => json!({"urlFilter":"||example.test^"}),
            };
            match generator.pick(9) {
                0 => condition["requestDomains"] = json!([domain]),
                1 => condition["requestDomains"] = json!(["example.test", "other.test"]),
                2 => condition["initiatorDomains"] = json!(["source.test"]),
                3 => condition["domains"] = json!(["source.test"]),
                4 => condition["excludedRequestDomains"] = json!(["d2.example.test"]),
                5 => condition["excludedInitiatorDomains"] = json!(["excluded.source.test"]),
                6 => condition["topDomains"] = json!(["top.test"]),
                7 => condition["excludedTopDomains"] = json!(["excluded.top.test"]),
                _ => {}
            }
            match generator.pick(5) {
                0 => condition["resourceTypes"] = json!(["script", "xmlhttprequest"]),
                1 => condition["resourceTypes"] = json!(["main_frame", "sub_frame"]),
                2 => condition["excludedResourceTypes"] = json!(["image"]),
                _ => {}
            }
            match generator.pick(5) {
                0 => condition["requestMethods"] = json!(["get", "post"]),
                1 => condition["excludedRequestMethods"] = json!(["head"]),
                _ => {}
            }
            match generator.pick(3) {
                0 => condition["domainType"] = json!("firstParty"),
                1 => condition["domainType"] = json!("thirdParty"),
                _ => {}
            }
            if kind == RulesetKind::Session {
                match generator.pick(3) {
                    0 => condition["tabIds"] = json!([-1, 7]),
                    1 => condition["excludedTabIds"] = json!([9]),
                    _ => {}
                }
            }
            match generator.pick(8) {
                0 => {
                    condition["responseHeaders"] = json!([
                        {"header":"content-type","values":["*javascript*","text/htm?"]}
                    ])
                }
                1 => {
                    condition["excludedResponseHeaders"] = json!([
                        {"header":"x-policy","values":["deny*"]}
                    ])
                }
                _ => {}
            }
            let action = match generator.pick(9) {
                0 => json!({"type":"allow"}),
                1 => {
                    condition["resourceTypes"] = json!(["main_frame", "sub_frame"]);
                    condition
                        .as_object_mut()
                        .expect("condition object")
                        .remove("excludedResourceTypes");
                    json!({"type":"allowAllRequests"})
                }
                2 => json!({"type":"upgradeScheme"}),
                3 => json!({"type":"redirect","redirect":{"url":"https://target.test/"}}),
                4 => json!({"type":"redirect","redirect":{"transform":{
                    "queryTransform":{"removeParams":["tracking"],
                        "addOrReplaceParams":[{"key":"source","value":"test"}]}
                }}}),
                5 => json!({"type":"modifyHeaders","requestHeaders":[
                    {"header":"x-test","operation":"set","value":"value"}
                ]}),
                6 => json!({"type":"modifyHeaders","requestHeaders":[
                {"header":"cookie","operation":"append","value":"a=b"}
            ],"responseHeaders":[{"header":"set-cookie","operation":"remove"}]}),
                7 if filter == 3 => json!({"type":"redirect","redirect":{
                    "regexSubstitution":"https://regex-target.test/"
                }}),
                _ => json!({"type":"block"}),
            };
            if action.get("requestHeaders").is_some() {
                let object = condition.as_object_mut().expect("condition object");
                object.remove("responseHeaders");
                object.remove("excludedResponseHeaders");
            }
            json!({"id":id,"priority":1+generator.pick(5),"action":action,"condition":condition})
        })
        .collect()
}

fn compile(seed: u64, kind: RulesetKind, id: &str) -> Vec<u8> {
    let source =
        serde_json::to_vec(&generated_rules(seed, kind)).expect("serialize generated rules");
    let parsed = parse_rules_json(&source, kind).expect("parse generated rules");
    assert!(
        parsed.rejected.is_empty(),
        "seed {seed}: {:?}",
        parsed.rejected
    );
    let compiled =
        CompiledRuleset::compile(id, kind, parsed.rules).expect("compile generated rules");
    let bytes = compiled.to_bytes();
    let loaded = CompiledRuleset::load(&bytes).expect("load generated rules");
    assert_eq!(bytes, loaded.to_bytes(), "binary round trip, seed {seed}");
    bytes
}

#[test]
fn generated_compiled_decisions_equal_linear_reference() {
    let headers = [
        Header {
            name: "Content-Type",
            value: "application/javascript",
        },
        Header {
            name: "x-policy",
            value: "deny-tracking",
        },
    ];
    let allow = [FrameAllow {
        extension_id: "older",
        priority: 3,
    }];
    for seed in 1..=24 {
        let static_bytes = compile(seed, RulesetKind::Static, "static");
        let dynamic_bytes = compile(seed ^ 0xabcd, RulesetKind::Dynamic, "_dynamic");
        let session_bytes = compile(seed ^ 0xef01, RulesetKind::Session, "_session");
        let static_set = CompiledRuleset::load(&static_bytes).expect("borrow static index");
        let dynamic_set = CompiledRuleset::load(&dynamic_bytes).expect("borrow dynamic index");
        let session_set = CompiledRuleset::load(&session_bytes).expect("borrow session index");
        let older = [&static_set, &session_set];
        let newer = [&dynamic_set];
        let extensions = [
            Extension {
                id: "older",
                install_order: 1,
                rulesets: &older,
            },
            Extension {
                id: "newer",
                install_order: 2,
                rulesets: &newer,
            },
        ];
        let reference_sets: Vec<_> = [
            (seed, RulesetKind::Static, "static", "older", 1),
            (seed ^ 0xef01, RulesetKind::Session, "_session", "older", 1),
            (seed ^ 0xabcd, RulesetKind::Dynamic, "_dynamic", "newer", 2),
        ]
        .into_iter()
        .map(
            |(rule_seed, kind, ruleset_id, extension_id, install_order)| {
                let source = serde_json::to_vec(&generated_rules(rule_seed, kind)).unwrap();
                let rules = parse_rules_json(&source, kind)
                    .unwrap()
                    .rules
                    .into_iter()
                    .map(|rule| PreparedRule::new(rule).unwrap())
                    .collect::<Vec<_>>();
                (ruleset_id, extension_id, install_order, rules)
            },
        )
        .collect();
        let mut generator = Generator(seed ^ 0xfedc_ba98);
        for case in 0..192 {
            let domain = generator.pick(8);
            let host = match case % 6 {
                0 => format!("d{domain}.example.test"),
                1 => format!("sub.d{domain}.example.test"),
                2 => format!("d{domain}.example.test.evil.test"),
                3 => "example.test".to_owned(),
                4 => "elsewhere.test".to_owned(),
                _ => "notexample.test".to_owned(),
            };
            let scheme = ["https", "http", "ftp"][case % 3];
            let path = match case % 5 {
                0 => format!("path{case}?tracking=1&needle{}", generator.pick(8)),
                1 => "CaseToken/aaa-aaa".to_owned(),
                2 => "casetoken/x/".to_owned(),
                3 => format!("path/needle{}", generator.pick(8)),
                _ => "clean".to_owned(),
            };
            let url = format!("{scheme}://{host}/{path}");
            let resource = [
                ResourceType::Script,
                ResourceType::MainFrame,
                ResourceType::SubFrame,
                ResourceType::Image,
                ResourceType::Xmlhttprequest,
                ResourceType::Other,
            ][case % 6];
            let mut request = Request::new(&url, &host, resource);
            request.method = [
                RequestMethod::Get,
                RequestMethod::Post,
                RequestMethod::Head,
                RequestMethod::Other,
            ][case % 4];
            request.initiator_host = [
                None,
                Some("source.test"),
                Some("sub.source.test"),
                Some("excluded.source.test"),
            ][case % 4];
            request.top_host = [None, Some("top.test"), Some("excluded.top.test")][case % 3];
            request.domain_type = if case % 2 == 0 {
                DomainType::FirstParty
            } else {
                DomainType::ThirdParty
            };
            request.tab_id = [-1, 7, 9][case % 3];
            request.stage = if case % 2 == 0 {
                Stage::BeforeRequest
            } else {
                Stage::ResponseHeaders
            };
            request.response_headers = if case % 4 < 2 { &headers } else { &[] };
            request.frame_allow = if case % 7 == 0 { &allow } else { &[] };
            let reference = evaluate_candidates(
                &request,
                reference_sets.iter().flat_map(
                    |(ruleset_id, extension_id, install_order, rules)| {
                        rules.iter().map(move |rule| Candidate {
                            extension_id,
                            install_order: *install_order,
                            ruleset_id,
                            rule,
                        })
                    },
                ),
            );
            assert_eq!(
                evaluate(&request, &extensions),
                reference,
                "serialized/original model difference, seed {seed}, case {case}"
            );
            assert_eq!(
                evaluate(&request, &extensions),
                evaluate_linear(&request, &extensions),
                "compiled/linear difference, seed {seed}, case {case}, URL {url}"
            );
        }
    }
}

#[test]
fn domain_and_repeated_gram_prefilters_never_drop_or_duplicate_decisions() {
    let source = br#"[
        {"id":1,"action":{"type":"block"},"condition":{"urlFilter":"||example.test"}},
        {"id":2,"priority":2,"action":{"type":"allow"},"condition":{"urlFilter":"aaa*aaa"}},
        {"id":3,"action":{"type":"modifyHeaders","requestHeaders":[
            {"header":"cookie","operation":"append","value":"a=b"}
        ]},"condition":{"urlFilter":"aaa"}},
        {"id":4,"action":{"type":"block"},"condition":{"requestDomains":["a.example.test","b.other.test"]}}
    ]"#;
    let rules = parse_rules_json(source, RulesetKind::Static)
        .expect("parse fixture")
        .rules;
    let compiled =
        CompiledRuleset::compile("fixture", RulesetKind::Static, rules).expect("compile fixture");
    let sets = [&compiled];
    let extensions = [Extension {
        id: "test",
        install_order: 1,
        rulesets: &sets,
    }];
    for (host, url) in [
        ("sub.example.test", "https://sub.example.test/aaaaaa"),
        ("example.test.evil.test", "https://example.test.evil.test/"),
        ("a.example.test", "https://a.example.test/"),
        ("b.other.test", "https://b.other.test/"),
        ("clean.test", "https://clean.test/aaaaaa/aaa"),
    ] {
        let request = Request::new(url, host, ResourceType::Script);
        assert_eq!(
            evaluate(&request, &extensions),
            evaluate_linear(&request, &extensions),
            "{url}"
        );
    }
}

#[test]
fn many_domain_postings_reload_and_match_case_insensitively() {
    let domains = [
        "example.test",
        "sub.example.test",
        "EXAMPLE.TEST",
        "other.test",
        "third.test",
        "fourth.test",
        "fifth.test",
        "sixth.test",
        "seventh.test",
    ];
    let rules = vec![Rule {
        id: 1,
        priority: 1,
        action: Action::Block,
        condition: Condition {
            request_domains: Some(domains.into_iter().map(str::to_owned).collect()),
            ..Condition::default()
        },
    }];
    let compiled =
        CompiledRuleset::compile("domains", RulesetKind::Static, rules).expect("compile domains");
    let bytes = compiled.to_bytes();
    let compiled = CompiledRuleset::load(&bytes).expect("many buckets for one rule reload");
    let sets = [&compiled];
    let extensions = [Extension {
        id: "test",
        install_order: 1,
        rulesets: &sets,
    }];
    for host in [
        "EXAMPLE.TEST",
        "SuB.ExAmPlE.TeSt",
        "sixth.test",
        "clean.test",
    ] {
        let url = format!("https://{}/", host.to_ascii_lowercase());
        let request = Request::new(&url, host, ResourceType::Script);
        assert_eq!(
            evaluate(&request, &extensions),
            evaluate_linear(&request, &extensions),
            "{host}"
        );
    }
}

#[test]
fn regex_quota_includes_rules_accelerated_by_domain() {
    let rules: Vec<_> = (1..=MAX_NUMBER_OF_REGEX_RULES)
        .map(|id| Rule {
            id: id as u32,
            priority: 1,
            action: Action::Block,
            condition: Condition {
                regex_filter: Some("needle".to_owned()),
                request_domains: Some(vec!["example.test".to_owned()]),
                ..Condition::default()
            },
        })
        .collect();
    let extra = vec![rules[0].clone()];
    let full =
        CompiledRuleset::compile("full", RulesetKind::Static, rules).expect("per-set regex quota");
    let extra =
        CompiledRuleset::compile("extra", RulesetKind::Static, extra).expect("one extra regex");
    assert!(validate_extension_rulesets(&[&full], GUARANTEED_MINIMUM_STATIC_RULES).is_ok());
    let error = validate_extension_rulesets(&[&full, &extra], GUARANTEED_MINIMUM_STATIC_RULES)
        .expect_err("aggregate regex limit includes domain-indexed rules");
    assert!(error.to_string().contains("regex"));
}
