#![no_main]
#![forbid(unsafe_code)]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use zephium_webext_dnr::{
    evaluate, parse_rules_json, Action, CompiledRuleset, Condition, Extension, Header, Request,
    ResourceType, Rule, RulesetKind, Stage,
};

const MAX_INPUT_BYTES: usize = 256 * 1024;

fn baseline() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let mut rules = Vec::new();
        for source in [
            include_bytes!("../corpus/rule_json/actions.json").as_slice(),
            include_bytes!("../corpus/rule_json/conditions.json").as_slice(),
        ] {
            let parsed =
                parse_rules_json(source, RulesetKind::Session).expect("synthetic baseline parses");
            assert!(parsed.rejected.is_empty());
            rules.extend(parsed.rules);
        }
        rules.push(Rule {
            id: 1,
            priority: 1,
            action: Action::Block,
            condition: Condition {
                regex_filter: Some("^[ab]*a[ab]{20}$".to_owned()),
                ..Condition::default()
            },
        });
        for (index, rule) in rules.iter_mut().enumerate() {
            rule.id = index as u32 + 1;
        }
        CompiledRuleset::compile("fuzz", RulesetKind::Session, rules)
            .expect("synthetic baseline compiles")
            .to_bytes()
    })
}

fn exercise(bytes: &[u8]) {
    if let Ok(index) = CompiledRuleset::load(bytes) {
        let encoded = index.to_bytes();
        let again = CompiledRuleset::load(&encoded).expect("loaded index round trip");
        assert_eq!(encoded, again.to_bytes());
        let rulesets = [&index];
        let extensions = [Extension {
            id: "fuzz",
            install_order: 1,
            rulesets: &rulesets,
        }];
        let headers = [Header {
            name: "content-type",
            value: "text/html",
        }];
        for (url, host) in [
            ("https://example.test/needle", "example.test"),
            (
                "http://sub.example.test/aaa/aaa?key=value",
                "sub.example.test",
            ),
            ("https://clean.test/", "clean.test"),
        ] {
            let mut request = Request::new(url, host, ResourceType::Script);
            request.initiator_host = Some("source.test");
            request.top_host = Some("top.test");
            request.response_headers = &headers;
            let _ = evaluate(&request, &extensions);
            request.stage = Stage::ResponseHeaders;
            let _ = evaluate(&request, &extensions);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    exercise(&data[..data.len().min(MAX_INPUT_BYTES)]);
    let mut mutation = baseline().to_vec();
    let mut offset_bytes = [0; 4];
    for (destination, source) in offset_bytes.iter_mut().zip(data) {
        *destination = *source;
    }
    let position = u32::from_le_bytes(offset_bytes) as usize;
    let start = position % mutation.len();
    for (offset, &byte) in data
        .get(4..)
        .unwrap_or_default()
        .iter()
        .take(256)
        .enumerate()
    {
        let index = (start + offset) % mutation.len();
        mutation[index] = byte;
    }
    // Reseal half of mutations to exercise inner lengths, enums and postings.
    if data.first().is_some_and(|byte| byte & 1 == 0) {
        let checksum = u64::from(crc32fast::hash(&mutation[96..]));
        mutation[56..64].copy_from_slice(&checksum.to_le_bytes());
    }
    exercise(&mutation);
});
