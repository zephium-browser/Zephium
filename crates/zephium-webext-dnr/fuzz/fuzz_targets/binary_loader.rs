#![no_main]
#![forbid(unsafe_code)]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use zephium_webext_dnr::{
    evaluate, parse_rules_json, CompiledRuleset, Extension, Request, ResourceType, RulesetKind,
};

const MAX_INPUT_BYTES: usize = 256 * 1024;

fn baseline() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let rules = parse_rules_json(
            include_bytes!("../corpus/rule_json/actions.json"),
            RulesetKind::Static,
        )
        .expect("synthetic baseline parses")
        .rules;
        CompiledRuleset::compile("fuzz", RulesetKind::Static, rules)
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
        let request = Request::new(
            "https://example.test/needle",
            "example.test",
            ResourceType::Script,
        );
        let _ = evaluate(&request, &extensions);
    }
}

fuzz_target!(|data: &[u8]| {
    exercise(&data[..data.len().min(MAX_INPUT_BYTES)]);
    let mut mutation = baseline().to_vec();
    let position = usize::from(data.first().copied().unwrap_or(0))
        | usize::from(data.get(1).copied().unwrap_or(0)) << 8;
    let start = position % mutation.len();
    for (offset, &byte) in data
        .get(2..)
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
        let checksum = mutation[48..]
            .iter()
            .fold(0xcbf29ce484222325u64, |hash, &byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
            });
        mutation[40..48].copy_from_slice(&checksum.to_le_bytes());
    }
    exercise(&mutation);
});
