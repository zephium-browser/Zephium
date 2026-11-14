#![cfg(feature = "compile")]

use zephium_webext_dnr::{
    evaluate, evaluate_candidates, parse_rules_json, Action, Candidate, CompiledRuleset, Condition,
    Extension, Header, PreparedRule, Request, ResourceType, Rule, RulesetKind, Stage,
};

#[test]
fn empty_indexes_roundtrip_and_produce_no_candidates() {
    for kind in [
        RulesetKind::Static,
        RulesetKind::Dynamic,
        RulesetKind::Session,
    ] {
        let owned = CompiledRuleset::compile("empty", kind, Vec::new()).unwrap();
        let bytes = owned.to_bytes();
        let loaded = CompiledRuleset::load(&bytes).unwrap();
        assert_eq!(bytes, loaded.to_bytes());
        assert_eq!(loaded.rule_count(), 0);
        let request = Request::new(
            "https://example.test/",
            "example.test",
            ResourceType::Script,
        );
        assert_eq!(loaded.candidates(&request).count(), 0);
        let rulesets = [&loaded];
        let extensions = [Extension {
            id: "empty",
            install_order: 1,
            rulesets: &rulesets,
        }];
        assert!(evaluate(&request, &extensions).matched_rules.is_empty());
    }
}

#[test]
fn accepted_empty_domain_entries_preserve_reference_semantics() {
    let rule = Rule {
        id: 1,
        priority: 1,
        action: Action::Block,
        condition: Condition {
            request_domains: Some(vec![String::new()]),
            ..Condition::default()
        },
    };
    let reference = PreparedRule::new(rule.clone()).unwrap();
    let compiled = CompiledRuleset::compile("domains", RulesetKind::Static, vec![rule]).unwrap();
    let bytes = compiled.to_bytes();
    let loaded = CompiledRuleset::load(&bytes).unwrap();
    let rulesets = [&loaded];
    let extensions = [Extension {
        id: "test",
        install_order: 1,
        rulesets: &rulesets,
    }];
    for (url, host) in [
        ("file:///resource", ""),
        ("https://example.test/", "example.test"),
    ] {
        let request = Request::new(url, host, ResourceType::Other);
        let expected = evaluate_candidates(
            &request,
            std::iter::once(Candidate {
                extension_id: "test",
                install_order: 1,
                ruleset_id: "domains",
                rule: &reference,
            }),
        );
        assert_eq!(evaluate(&request, &extensions), expected);
    }
}

#[test]
fn borrowed_automata_and_lazy_fallback_match_original_rules() {
    let source = br#"[
        {"id":1,"action":{"type":"block"},"condition":{
            "regexFilter":"^https://example[.]test/[ab]*a[ab]{20}$",
            "requestDomains":["example.test"]}},
        {"id":2,"priority":2,"action":{"type":"redirect","redirect":{
            "regexSubstitution":"https://target.test/\\1"}},"condition":{
            "regexFilter":"^https://example[.]test/Capture/([a-z]+)$",
            "isUrlFilterCaseSensitive":true}},
        {"id":3,"action":{"type":"block"},"condition":{
            "regexFilter":"\\Qhttps://example.test/literal?\\E"}},
        {"id":4,"priority":3,"action":{"type":"allow"},"condition":{
            "urlFilter":"headers","responseHeaders":[{
                "header":"content-type","values":["text/htm?","*javascript*"],
                "excludedValues":["*private*"]}]}},
        {"id":5,"action":{"type":"modifyHeaders","responseHeaders":[{
            "header":"x-policy","operation":"set","value":"bounded"}]},
            "condition":{"urlFilter":"response-edit"}}
    ]"#;
    let parsed = parse_rules_json(source, RulesetKind::Static).unwrap();
    assert!(parsed.rejected.is_empty(), "{:?}", parsed.rejected);
    let reference: Vec<_> = parsed
        .rules
        .iter()
        .cloned()
        .map(|rule| PreparedRule::new(rule).unwrap())
        .collect();
    let owned = CompiledRuleset::compile("renderer", RulesetKind::Static, parsed.rules).unwrap();
    let bytes = owned.to_bytes();
    let headers = [Header {
        name: "Content-Type",
        value: "text/html",
    }];
    for padding in 0..8 {
        let mut storage = vec![0; padding];
        storage.extend_from_slice(&bytes);
        let loaded = CompiledRuleset::load(&storage[padding..]).unwrap();
        let rulesets = [&loaded];
        let extensions = [Extension {
            id: "test",
            install_order: 1,
            rulesets: &rulesets,
        }];
        for path in [
            "abbbbbbbbbbbbbbbbbbbb",
            "bbbbbbbbbbbbbbbbbbbbb",
            "Capture/value",
            "capture/value",
            "literal?value=1",
            "headers",
            "response-edit",
        ] {
            let url = format!("https://example.test/{path}");
            for stage in [Stage::BeforeRequest, Stage::ResponseHeaders] {
                let mut request = Request::new(&url, "example.test", ResourceType::Script);
                request.stage = stage;
                request.response_headers = &headers;
                let expected = evaluate_candidates(
                    &request,
                    reference.iter().map(|rule| Candidate {
                        extension_id: "test",
                        install_order: 1,
                        ruleset_id: "renderer",
                        rule,
                    }),
                );
                assert_eq!(
                    evaluate(&request, &extensions),
                    expected,
                    "{path}, {stage:?}"
                );
            }
        }
    }
}
