#![cfg(feature = "compile")]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;

use zephium_webext_dnr::{parse_rules_json, CompiledRuleset, RulesetKind, MAX_RULESET_JSON_BYTES};

#[test]
fn optional_ubol_corpus_compiles_and_reloads() {
    let Some(directory) = std::env::var_os("ZEPHIUM_DNR_CORPUS") else {
        eprintln!("SKIP uBOL corpus: set ZEPHIUM_DNR_CORPUS to chromium/rulesets");
        return;
    };
    let root = fs::canonicalize(directory).expect("canonical corpus directory");
    let mut files = Vec::new();
    for category in ["main", "regex", "strictblock"] {
        let directory = root.join(category);
        if !directory.is_dir() {
            continue;
        }
        for entry in fs::read_dir(directory).expect("read corpus category") {
            let entry = entry.expect("read corpus entry");
            if entry.file_type().expect("corpus entry type").is_file()
                && entry.path().extension().is_some_and(|ext| ext == "json")
            {
                assert!(files.len() < 10_000, "corpus file count exceeds limit");
                files.push(entry.path());
            }
        }
    }
    files.sort();
    assert!(!files.is_empty(), "no DNR corpus arrays found");
    let mut accepted = 0;
    let mut rejected = 0;
    let mut compiled_bytes = 0;
    let mut reasons = BTreeMap::<String, usize>::new();
    for path in &files {
        let mut bytes = Vec::new();
        File::open(path)
            .expect("open corpus JSON read-only")
            .take(MAX_RULESET_JSON_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .expect("read bounded corpus JSON");
        assert!(bytes.len() <= MAX_RULESET_JSON_BYTES, "{}", path.display());
        let parsed = parse_rules_json(&bytes, RulesetKind::Static)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        accepted += parsed.rules.len();
        rejected += parsed.rejected.len();
        for rejection in parsed.rejected {
            *reasons.entry(rejection.reason).or_default() += 1;
        }
        let id = path
            .strip_prefix(&root)
            .expect("relative corpus path")
            .to_string_lossy()
            .replace('\\', "/");
        let index = CompiledRuleset::compile(&id, RulesetKind::Static, parsed.rules)
            .unwrap_or_else(|error| panic!("compile {}: {error}", path.display()));
        let sealed = index.to_bytes();
        let loaded = CompiledRuleset::load(&sealed)
            .unwrap_or_else(|error| panic!("load {}: {error}", path.display()));
        assert_eq!(sealed, loaded.to_bytes(), "{}", path.display());
        compiled_bytes += sealed.len();
    }
    eprintln!(
        "uBOL corpus: {} rulesets, {accepted} accepted, {rejected} rejected, \
         {compiled_bytes} compiled bytes; rejection reasons: {reasons:?}",
        files.len()
    );
}
