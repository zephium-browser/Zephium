#![no_main]
#![forbid(unsafe_code)]

use libfuzzer_sys::fuzz_target;
use zephium_webext_dnr::{parse_rules_json, CompiledRuleset, RulesetKind};

const MAX_INPUT_BYTES: usize = 128 * 1024;

fuzz_target!(|data: &[u8]| {
    let data = &data[..data.len().min(MAX_INPUT_BYTES)];
    for kind in [
        RulesetKind::Static,
        RulesetKind::Dynamic,
        RulesetKind::Session,
    ] {
        let first = parse_rules_json(data, kind);
        let second = parse_rules_json(data, kind);
        match (first, second) {
            (Ok(first), Ok(second)) => {
                assert_eq!(first, second);
                if first.rules.len() <= 512 {
                    if let Ok(index) = CompiledRuleset::compile("fuzz", kind, first.rules) {
                        let bytes = index.to_bytes();
                        let loaded = CompiledRuleset::load(&bytes)
                            .expect("a successfully compiled index must reload");
                        assert_eq!(bytes, loaded.to_bytes());
                    }
                }
            }
            (Err(first), Err(second)) => assert_eq!(first.to_string(), second.to_string()),
            _ => panic!("parser returned different results for identical input"),
        }
    }
});
