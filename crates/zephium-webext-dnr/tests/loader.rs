use std::sync::OnceLock;

use zephium_webext_dnr::{
    evaluate, Action, CompiledRuleset, Condition, DecisionAction, Extension, HeaderCondition,
    HeaderModification, HeaderOperation, Redirect, Request, ResourceType, Rule, RulesetKind,
    MAX_NUMBER_OF_REGEX_RULES,
};

const HEADER: usize = 96;
const ROW: usize = 144;
const SLOT: usize = 32;

fn fixture() -> Vec<u8> {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES
        .get_or_init(|| {
            let conditions = [
                Condition {
                    url_filter: Some("||allowed.test^".into()),
                    request_domains: Some(vec!["allowed.test".into(), "other.test".into()]),
                    excluded_request_domains: Some(vec!["excluded.test".into()]),
                    ..Condition::default()
                },
                Condition {
                    regex_filter: Some("^https://regex[.]test/[a-z]+$".into()),
                    ..Condition::default()
                },
                Condition {
                    url_filter: Some("redirect".into()),
                    ..Condition::default()
                },
                Condition {
                    url_filter: Some("headers".into()),
                    ..Condition::default()
                },
                Condition {
                    response_headers: Some(vec![HeaderCondition {
                        header: "content-type".into(),
                        values: Some(vec!["text/*".into()]),
                        excluded_values: None,
                    }]),
                    ..Condition::default()
                },
            ];
            let rules = conditions
                .into_iter()
                .enumerate()
                .map(|(index, condition)| Rule {
                    id: index as u32 + 1,
                    priority: 1,
                    action: match index {
                        2 => Action::Redirect {
                            redirect: Box::new(Redirect {
                                url: Some("https://x.y/".into()),
                                ..Redirect::default()
                            }),
                        },
                        3 => Action::ModifyHeaders {
                            request_headers: vec![],
                            response_headers: vec![HeaderModification {
                                header: "x-tag".into(),
                                operation: HeaderOperation::Set,
                                value: Some("safe".into()),
                            }],
                        },
                        _ => Action::Block,
                    },
                    condition,
                })
                .collect();
            let compiled = CompiledRuleset::compile("loader", RulesetKind::Static, rules).unwrap();
            assert_eq!(compiled.regex_fallback_count(), 0);
            let bytes = compiled.to_bytes();
            assert!(CompiledRuleset::load(&bytes).is_ok());
            bytes
        })
        .clone()
}

fn get(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn put(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn reseal(bytes: &mut [u8]) {
    let crc = u64::from(crc32fast::hash(&bytes[HEADER..]));
    bytes[56..64].copy_from_slice(&crc.to_le_bytes());
}

fn rejected(bytes: &[u8], reason: &str) {
    match std::panic::catch_unwind(|| CompiledRuleset::load(bytes)) {
        Ok(Err(_)) => {}
        Ok(Ok(_)) => panic!("loader accepted {reason}"),
        Err(_) => panic!("loader panicked for {reason}"),
    }
}

fn bucket_entries(bytes: &[u8]) -> Vec<usize> {
    let table = get(bytes, 28) as usize;
    (0..get(bytes, 32) as usize)
        .map(|slot| table + slot * SLOT)
        .collect()
}

fn prefix_directory(bytes: &[u8]) -> (usize, usize) {
    let start = get(bytes, 72) as usize + get(bytes, 76) as usize;
    let count = get(bytes, 88) as usize;
    assert_eq!(start + (count + 1) * 4, get(bytes, 28) as usize);
    (start, count)
}

fn many_buckets() -> Vec<u8> {
    CompiledRuleset::compile(
        "prefixes",
        RulesetKind::Static,
        vec![Rule {
            id: 1,
            priority: 1,
            action: Action::Block,
            condition: Condition {
                request_domains: Some((0..129).map(|i| format!("domain-{i}.test")).collect()),
                ..Condition::default()
            },
        }],
    )
    .unwrap()
    .to_bytes()
}

fn replace_dfa_with_lazy(bytes: &mut Vec<u8>) {
    assert_eq!(get(bytes, 40), 1);
    let directory = get(bytes, 36) as usize;
    let start = get(bytes, directory) as usize;
    let length = get(bytes, directory + 4) as usize;
    let end = start + length;
    let mut pointers = vec![12, 28, 36, 44, 72];
    for index in 0..get(bytes, 16) as usize {
        for field in [44, 60, 68, 76, 84, 92, 100, 108, 116] {
            pointers.push(HEADER + index * ROW + field);
        }
    }
    pointers.extend(bucket_entries(bytes).into_iter().map(|slot| slot + 20));
    for field in pointers {
        let value = get(bytes, field);
        if value != u32::MAX && value as usize >= end {
            put(bytes, field, value - length as u32);
        }
    }
    put(bytes, directory, 0);
    put(bytes, directory + 4, 0);
    drop(bytes.drain(start..end));
    reseal(bytes);
}

#[test]
fn declared_counts_and_integer_overflows_are_rejected_before_allocation() {
    let original = fixture();
    for (at, value) in [
        (16, u32::MAX),
        (28, u32::MAX),
        (32, 0),
        (32, 3),
        (32, 1 << 31),
        (36, u32::MAX),
        (40, u32::MAX),
        (40, MAX_NUMBER_OF_REGEX_RULES as u32 + 1),
        (44, u32::MAX),
        (48, u32::MAX),
        (88, 0),
        (88, 3),
        (88, 1 << 31),
        (92, 1),
    ] {
        let mut bytes = original.clone();
        put(&mut bytes, at, value);
        reseal(&mut bytes);
        rejected(&bytes, &format!("header {at}={value}"));
    }
}

#[test]
fn semantic_row_bounds_and_duplicate_identifiers_are_rejected() {
    let original = fixture();
    for (at, value) in [
        (HEADER, 0),
        (HEADER, i32::MAX as u32 + 1),
        (HEADER + 4, i32::MAX as u32 + 1),
        (HEADER + 8, 6),
        (HEADER + 12, 8),
        (HEADER + 32, 0x8000),
        (HEADER + 36, 0x40000),
        (HEADER + 40, 3),
        (HEADER + ROW, 1),
        (HEADER + 124, u32::MAX),
        (HEADER + 128, u32::MAX),
        (HEADER + 132, 0),
        (HEADER + 136, u32::MAX),
        (HEADER + 140, u32::MAX),
    ] {
        let mut bytes = original.clone();
        put(&mut bytes, at, value);
        reseal(&mut bytes);
        rejected(&bytes, &format!("row {at}={value}"));
    }
}

#[test]
fn domain_and_posting_ranges_refuse_aliases_to_fixed_tables_and_overflows() {
    let original = fixture();
    let occupied = bucket_entries(&original)[0];
    for (at, value) in [
        (HEADER + 60, HEADER as u32),
        (HEADER + 64, u32::MAX),
        (HEADER + 68, u32::MAX),
        (HEADER + 72, u32::MAX),
        (occupied + 20, HEADER as u32),
        (occupied + 24, u32::MAX),
    ] {
        let mut bytes = original.clone();
        put(&mut bytes, at, value);
        reseal(&mut bytes);
        rejected(&bytes, &format!("span {at}={value}"));
    }
}

#[test]
fn empty_positive_domain_scope_and_illegal_static_tabs_are_rejected() {
    let mut bytes = fixture();
    put(&mut bytes, HEADER + 64, 0);
    reseal(&mut bytes);
    rejected(&bytes, "empty required domain scope");

    let compiled = CompiledRuleset::compile(
        "session",
        RulesetKind::Session,
        vec![Rule {
            id: 1,
            priority: 1,
            action: Action::Block,
            condition: Condition {
                tab_ids: Some(vec![7]),
                ..Condition::default()
            },
        }],
    )
    .unwrap();
    let mut bytes = compiled.to_bytes();
    put(&mut bytes, 52, 0);
    rejected(&bytes, "session tab condition in static ruleset");
}

#[test]
fn malformed_action_options_strings_and_header_semantics_are_rejected() {
    let original = fixture();
    let redirect = get(&original, HEADER + 2 * ROW + 44) as usize;
    let header_record = get(&original, HEADER + 3 * ROW + 44) as usize;
    for (at, value) in [(redirect, 255), (redirect + 1, 2)] {
        let mut bytes = original.clone();
        bytes[at] = value;
        reseal(&mut bytes);
        rejected(&bytes, "invalid action tag or option discriminator");
    }
    let mut bytes = original.clone();
    put(&mut bytes, redirect + 2, u32::MAX);
    reseal(&mut bytes);
    rejected(&bytes, "oversized action string");

    let mut bytes = original.clone();
    assert_eq!(&bytes[redirect + 6..redirect + 18], b"https://x.y/");
    bytes[redirect + 6..redirect + 18].copy_from_slice(b"javascript:a");
    reseal(&mut bytes);
    rejected(&bytes, "javascript redirect");

    let safe = original[header_record..]
        .windows(4)
        .position(|text| text == b"safe")
        .unwrap()
        + header_record;
    let mut bytes = original.clone();
    bytes[safe..safe + 4].copy_from_slice(b"a\r\nb");
    reseal(&mut bytes);
    rejected(&bytes, "header value with CRLF");

    let mut bytes = original;
    put(&mut bytes, header_record + 1, u32::MAX);
    reseal(&mut bytes);
    rejected(&bytes, "oversized header vector");
}

#[test]
fn dfa_directory_framing_and_serialized_metadata_are_checked() {
    let original = fixture();
    let directory = get(&original, 36) as usize;
    let dfa = get(&original, directory) as usize;
    let length = get(&original, directory + 4);
    assert!(length > 40);
    for (at, value) in [
        (directory, 0),
        (directory, u32::MAX),
        (directory + 4, u32::MAX),
        (directory + 4, length - 1),
        (directory + 8, u32::MAX),
        (HEADER + ROW + 52, u32::MAX),
        (dfa, u32::MAX),
        (dfa + 32, u32::MAX),
        (dfa + 36, u32::MAX),
        (dfa + 48, u32::MAX),
        (dfa + 52, u32::MAX),
    ] {
        let mut bytes = original.clone();
        put(&mut bytes, at, value);
        reseal(&mut bytes);
        rejected(&bytes, &format!("DFA {at}={value}"));
    }
}

#[test]
fn regex_source_is_validated_even_when_a_serialized_dfa_exists() {
    let original = fixture();
    let pool = get(&original, 44) as usize;
    let source = pool + get(&original, HEADER + ROW + 24) as usize;
    for prefix in [b"\\i", b"(?"] {
        let mut bytes = original.clone();
        bytes[source..source + 2].copy_from_slice(prefix);
        reseal(&mut bytes);
        rejected(&bytes, "invalid RE2 source beside a valid DFA");
    }
}

#[test]
fn posting_coverage_cannot_omit_a_scoped_rule_or_point_at_another_rule() {
    let original = fixture();
    let slot = bucket_entries(&original)
        .into_iter()
        .find(|&slot| get(&original, slot + 8) == 1)
        .unwrap();
    let posting = get(&original, slot + 20) as usize;
    for (at, value) in [(slot + 24, 0), (posting, 1)] {
        let mut bytes = original.clone();
        put(&mut bytes, at, value);
        reseal(&mut bytes);
        rejected(&bytes, "inconsistent rule posting coverage");
    }
}

#[test]
fn string_pool_utf8_and_character_boundaries_are_checked() {
    let original = fixture();
    let pool = get(&original, 44) as usize;
    let mut bytes = original.clone();
    bytes[pool] = 0xff;
    reseal(&mut bytes);
    rejected(&bytes, "invalid pool UTF-8");

    let mut bytes = original;
    bytes[pool..pool + 2].copy_from_slice("é".as_bytes());
    put(&mut bytes, 64, 1);
    reseal(&mut bytes);
    rejected(&bytes, "string starting inside a UTF-8 character");
}

#[test]
fn aliased_row_data_and_duplicate_hash_entries_are_rejected() {
    let original = fixture();
    let first_record = get(&original, HEADER + 44);
    let mut bytes = original.clone();
    put(&mut bytes, HEADER + ROW + 44, first_record);
    reseal(&mut bytes);
    rejected(&bytes, "aliased action record");

    let mut bytes = original.clone();
    let domains = get(&original, HEADER + 60);
    put(&mut bytes, HEADER + 68, domains);
    reseal(&mut bytes);
    rejected(&bytes, "aliased domain data");

    let (prefixes, count) = prefix_directory(&original);
    let first = (0..count)
        .find_map(|prefix| {
            let start = get(&original, prefixes + prefix * 4) as usize;
            let end = get(&original, prefixes + (prefix + 1) * 4) as usize;
            (end - start >= 2).then_some(start)
        })
        .unwrap();
    let entries = bucket_entries(&original);
    let mut bytes = original;
    bytes.copy_within(entries[first]..entries[first] + SLOT, entries[first + 1]);
    reseal(&mut bytes);
    rejected(&bytes, "duplicate hash entry");
}

#[test]
fn prefix_directory_ranges_and_sentinels_are_validated() {
    let original = many_buckets();
    assert!(CompiledRuleset::load(&original).is_ok());
    let (prefixes, count) = prefix_directory(&original);
    let buckets = get(&original, 32);
    assert!(count > 2);
    for (at, value) in [
        (prefixes, 1),
        (prefixes + count * 4, buckets - 1),
        (prefixes + 4, u32::MAX),
        (prefixes + 4, buckets + 1),
    ] {
        let mut bytes = original.clone();
        put(&mut bytes, at, value);
        reseal(&mut bytes);
        rejected(&bytes, "invalid prefix range or sentinel");
    }
    let mut bytes = original;
    put(&mut bytes, prefixes + 4, 2);
    put(&mut bytes, prefixes + 8, 1);
    reseal(&mut bytes);
    rejected(&bytes, "descending prefix offsets");
}

#[test]
fn compact_buckets_require_prefix_membership_strict_order_and_occupied_tags() {
    let original = fixture();
    let (prefixes, count) = prefix_directory(&original);
    assert_eq!(count, 2);
    let boundary = get(&original, prefixes + 4);
    let buckets = get(&original, 32);
    let mut bytes = original.clone();
    put(
        &mut bytes,
        prefixes + 4,
        if boundary < buckets {
            boundary + 1
        } else {
            boundary - 1
        },
    );
    reseal(&mut bytes);
    rejected(&bytes, "entry moved into another hash prefix");

    let first = (0..count)
        .find_map(|prefix| {
            let start = get(&original, prefixes + prefix * 4) as usize;
            let end = get(&original, prefixes + (prefix + 1) * 4) as usize;
            (end - start >= 2).then_some(start)
        })
        .unwrap();
    let entries = bucket_entries(&original);
    let mut bytes = original.clone();
    let saved: [u8; SLOT] = bytes[entries[first]..entries[first] + SLOT]
        .try_into()
        .unwrap();
    bytes.copy_within(
        entries[first + 1]..entries[first + 1] + SLOT,
        entries[first],
    );
    bytes[entries[first + 1]..entries[first + 1] + SLOT].copy_from_slice(&saved);
    reseal(&mut bytes);
    rejected(&bytes, "entries out of strict hash order");

    for (field, value) in [(8, 0), (8, 6), (28, 1)] {
        let mut bytes = original.clone();
        put(&mut bytes, entries[0] + field, value);
        reseal(&mut bytes);
        rejected(&bytes, "empty, invalid or reserved compact bucket tag");
    }
}

#[test]
fn prefix_lookup_work_has_a_hard_range_length_bound() {
    let mut bytes = many_buckets();
    let (prefixes, count) = prefix_directory(&bytes);
    assert_eq!(get(&bytes, 32), 129);
    for prefix in 1..=count {
        put(&mut bytes, prefixes + prefix * 4, 129);
    }
    reseal(&mut bytes);
    rejected(
        &bytes,
        "129-entry prefix exceeds the 128-entry lookup bound",
    );
}

#[test]
fn lazy_fallback_rejects_invalid_source_and_settles_repeated_matches() {
    let mut original = fixture();
    replace_dfa_with_lazy(&mut original);
    let loaded = CompiledRuleset::load(&original).unwrap();
    assert_eq!(loaded.regex_fallback_count(), 1);
    let sets = [&loaded];
    let extensions = [Extension {
        id: "test",
        install_order: 1,
        rulesets: &sets,
    }];
    for _ in 0..3 {
        for (path, expected) in [
            ("abc", DecisionAction::Block),
            ("123", DecisionAction::Allow),
        ] {
            let url = format!("https://regex.test/{path}");
            let request = Request::new(&url, "regex.test", ResourceType::Script);
            assert_eq!(evaluate(&request, &extensions).action, expected);
        }
    }
    let pool = get(&original, 44) as usize;
    let source = pool + get(&original, HEADER + ROW + 24) as usize;
    let mut malformed = original;
    malformed[source..source + 2].copy_from_slice(b"\\i");
    reseal(&mut malformed);
    rejected(&malformed, "invalid lazy fallback syntax");
}
