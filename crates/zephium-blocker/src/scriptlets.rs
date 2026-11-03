//! Scriptlets: functions from our own library, run in a page's main world at
//! document start with arguments from `##+js(...)` rules. adblock-rust matches
//! the rules to hosts, applies `#@#+js` exceptions and refuses `trusted-*`
//! scriptlets to lists without the trusted permission; this module supplies the
//! library as its resources and builds one program for every frame: a table
//! from hashed hostnames to the calls for that host, and the library, which
//! runs only when the frame's host has an entry. The library is reviewed code
//! shipped in the app and never fetched.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use adblock::lists::{FilterSet, ParseOptions, RuleTypes};
use adblock::resources::{MimeType, PermissionMask, Resource, ResourceType};
use adblock::Engine;
use base64::Engine as _;

/// The permission `trusted-*` scriptlets need and only uBO's lists grant.
const TRUSTED: PermissionMask = PermissionMask::from_bits(1);
/// Ends every library function, so a host's calls are what follows the last.
const END: &str = "/* zephium scriptlet end */";
/// How adblock-rust closes each call it emits.
const CALL_END: &str = "} catch ( e ) { }";
const MAX_RULES: usize = 20_000;
const MAX_RULE_BYTES: usize = 8 * 1024;
const MAX_PROGRAM_BYTES: usize = 4 * 1024 * 1024;

struct Scriptlet {
    name: &'static str,
    aliases: &'static [&'static str],
    trusted: bool,
    dependencies: &'static [&'static str],
    source: &'static str,
}

/// Helpers named here are dependencies, not rule names.
const HELPERS: &[Scriptlet] = &[Scriptlet {
    name: "zephium-constant.fn",
    aliases: &[],
    trusted: false,
    dependencies: &[],
    source: include_str!("scriptlets/constant.js"),
}];

const SCRIPTLETS: &[Scriptlet] = &[Scriptlet {
    name: "set-constant.js",
    aliases: &["set.js"],
    trusted: false,
    dependencies: &["zephium-constant.fn"],
    source: include_str!("scriptlets/set-constant.js"),
}];

/// A source file's function, without its leading comment lines.
fn function_text(source: &str) -> String {
    let body: Vec<&str> = source
        .lines()
        .skip_while(|line| line.starts_with("//"))
        .collect();
    format!("{}\n{END}", body.join("\n").trim_end())
}

fn resources() -> Vec<Resource> {
    HELPERS
        .iter()
        .chain(SCRIPTLETS)
        .map(|scriptlet| Resource {
            name: scriptlet.name.to_owned(),
            aliases: scriptlet
                .aliases
                .iter()
                .map(|alias| (*alias).to_owned())
                .collect(),
            // Helpers are dependencies only; scriptlets are what rules call.
            kind: ResourceType::Mime(if scriptlet.name.ends_with(".fn") {
                MimeType::FnJavascript
            } else {
                MimeType::ApplicationJavascript
            }),
            content: base64::engine::general_purpose::STANDARD
                .encode(function_text(scriptlet.source)),
            dependencies: scriptlet
                .dependencies
                .iter()
                .map(|dependency| (*dependency).to_owned())
                .collect(),
            permission: if scriptlet.trusted {
                TRUSTED
            } else {
                PermissionMask::default()
            },
        })
        .collect()
}

/// Whether the library has a scriptlet by this rule name.
fn known(name: &str) -> bool {
    let name = name.strip_suffix(".js").unwrap_or(name);
    SCRIPTLETS.iter().any(|scriptlet| {
        std::iter::once(scriptlet.name)
            .chain(scriptlet.aliases.iter().copied())
            .any(|candidate| candidate.strip_suffix(".js") == Some(name))
    })
}

/// One `##+js` rule as admitted, with its list's trust.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScriptletRule {
    pub(crate) line: String,
    pub(crate) trusted: bool,
}

/// What a line is, for admission and counting.
pub(crate) enum Kind {
    /// Not scriptlet syntax.
    None,
    /// A rule to keep, and whether its scriptlet is one the library lacks.
    Rule { unknown: bool },
    /// Scriptlet syntax we do not run: too long, no hostname, or a hostname
    /// form other than names and single-label entities.
    Unsupported,
}

pub(crate) fn classify(line: &str) -> Kind {
    let Some((domains, body)) = line
        .split_once("##+js(")
        .or_else(|| line.split_once("#@#+js("))
    else {
        return Kind::None;
    };
    if line.len() > MAX_RULE_BYTES || !body.ends_with(')') {
        return Kind::Unsupported;
    }
    let exception = line.contains("#@#+js(");
    let keys = domain_keys(domains);
    if keys.is_none() || (!exception && domains.is_empty()) {
        return Kind::Unsupported;
    }
    let name = body[..body.len() - 1]
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    Kind::Rule {
        unknown: !exception && !known(name),
    }
}

/// The hostnames and entities a rule names, negated ones included, or `None`
/// when one is a form the table cannot key.
fn domain_keys(domains: &str) -> Option<Vec<String>> {
    domains
        .split(',')
        .filter(|domain| !domain.is_empty())
        .map(|domain| {
            let domain = domain
                .strip_prefix('~')
                .unwrap_or(domain)
                .to_ascii_lowercase();
            let name = domain.strip_suffix(".*").unwrap_or(&domain);
            let multi_label_entity = domain.ends_with(".*") && name.contains('.');
            let valid = !name.is_empty()
                && !multi_label_entity
                && name.split('.').all(|label| {
                    !label.is_empty()
                        && label
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                });
            valid.then_some(domain)
        })
        .collect()
}

/// Two 32-bit FNV-1a hashes of a hostname key, combined into 53 bits so the
/// page can compare them as one exact number.
fn key_hash(key: &str) -> u64 {
    let fnv = |offset: u32| {
        key.bytes().fold(offset, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
        })
    };
    (u64::from(fnv(2_166_136_261)) << 21) | u64::from(fnv(0x8422_2325) >> 11)
}

/// The same hash, in the page.
const PAGE_HASH: &str = "const fnv = (key, offset) => { let hash = offset; for (let i = 0; i < key.length; i++) { hash = Math.imul(hash ^ key.charCodeAt(i), 16777619); } return hash >>> 0; };\nconst hash = (key) => fnv(key, 2166136261) * 2097152 + (fnv(key, 0x84222325) >>> 11);";

/// The program every frame runs, built from the admitted rules.
pub(crate) struct Program {
    pub(crate) script: Arc<str>,
    pub(crate) hosts: usize,
    pub(crate) calls: usize,
}

pub(crate) fn build(rules: &[ScriptletRule]) -> Result<Option<Program>, crate::CosmeticError> {
    if rules.len() > MAX_RULES {
        return Err(crate::CosmeticError::ResourceLimit);
    }
    let mut keys = BTreeSet::new();
    let (mut trusted, mut untrusted) = (Vec::new(), Vec::new());
    for rule in rules {
        let Some((domains, _)) = rule
            .line
            .split_once("##+js(")
            .or_else(|| rule.line.split_once("#@#+js("))
        else {
            return Err(crate::CosmeticError::InvalidPolicy);
        };
        keys.extend(domain_keys(domains).ok_or(crate::CosmeticError::InvalidPolicy)?);
        if rule.trusted {
            &mut trusted
        } else {
            &mut untrusted
        }
        .push(rule.line.as_str());
    }
    if keys.is_empty() {
        return Ok(None);
    }
    let mut filters = FilterSet::new(false);
    for (lines, permissions) in [(trusted, TRUSTED), (untrusted, PermissionMask::default())] {
        filters.add_filter_list(
            lines.join("\n"),
            ParseOptions {
                rule_types: RuleTypes::CosmeticOnly,
                permissions,
                ..ParseOptions::default()
            },
        );
    }
    let mut engine = Engine::new_with_filter_set(filters);
    engine.use_resources(resources());
    // A key's entry is what adblock-rust injects on that host; an entity is
    // asked about under a suffix no rule names.
    let mut calls: BTreeMap<String, usize> = BTreeMap::new();
    let mut table = Vec::new();
    for key in &keys {
        let host = match key.strip_suffix(".*") {
            Some(name) => format!("{name}.invalid"),
            None => key.clone(),
        };
        let script = engine
            .url_cosmetic_resources(&format!("https://{host}/"))
            .injected_script;
        // adblock-rust gathers a host's calls in a hash map; a fixed order
        // keeps the program the same for the same rules.
        let mut blocks: Vec<&str> = script
            .rsplit(END)
            .next()
            .unwrap_or_default()
            .split_inclusive(CALL_END)
            .map(str::trim)
            .filter(|block| !block.is_empty())
            .collect();
        if blocks.is_empty() {
            continue;
        }
        blocks.sort_unstable();
        let invocations = blocks.join("\n");
        let next = calls.len();
        let index = *calls.entry(invocations).or_insert(next);
        table.push((key_hash(key), index));
    }
    if table.is_empty() {
        return Ok(None);
    }
    let mut ordered: Vec<(&String, &usize)> = calls.iter().collect();
    ordered.sort_unstable_by_key(|(_, index)| **index);
    let library: Vec<String> = HELPERS
        .iter()
        .chain(SCRIPTLETS)
        .map(|scriptlet| function_text(scriptlet.source))
        .collect();
    let entries: Vec<String> = table
        .iter()
        .map(|(hash, index)| format!("[{hash},{index}]"))
        .collect();
    let runs: Vec<String> = ordered
        .iter()
        .map(|(invocations, _)| format!("function () {{\n{invocations}\n}}"))
        .collect();
    let script = format!(
        "(() => {{\n{PAGE_HASH}\nconst table = new Map([{}]);\nconst host = location.hostname.replace(/\\.$/, '');\nif (host === '') {{ return; }}\nconst labels = host.split('.');\nlet entry;\nfor (let i = 0; i < labels.length && entry === undefined; i++) {{ entry = table.get(hash(labels.slice(i).join('.'))); }}\nfor (let i = 0; i < labels.length - 1 && entry === undefined; i++) {{ entry = table.get(hash(labels[i] + '.*')); }}\nif (entry === undefined) {{ return; }}\n(function () {{\n{}\nconst runs = [{}];\nruns[entry]();\n}})();\n}})();\n",
        entries.join(","),
        library.join("\n"),
        runs.join(",\n"),
    );
    if script.len() > MAX_PROGRAM_BYTES {
        return Err(crate::CosmeticError::ResourceLimit);
    }
    Ok(Some(Program {
        script: script.into(),
        hosts: table.len(),
        calls: ordered.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(line: &str, trusted: bool) -> ScriptletRule {
        ScriptletRule {
            line: line.to_owned(),
            trusted,
        }
    }

    #[test]
    fn page_and_compiler_hash_hostnames_alike() {
        // Values the page's own hash function returns for these keys.
        for (key, page) in [
            ("youtube.com", 2_476_409_116_368_181),
            ("youtube.*", 4_577_303_577_750_198),
            ("m.youtube.com", 3_337_323_086_219_822),
        ] {
            assert_eq!(key_hash(key), page, "{key}");
            assert!(key_hash(key) < 1 << 53);
        }
    }

    #[test]
    fn a_host_runs_its_calls_and_exceptions_and_trust_stay_adblock_s() {
        let program = build(&[
            rule(
                "youtube.com##+js(set, ytInitialPlayerResponse.adPlacements, undefined)",
                false,
            ),
            rule(
                "m.youtube.com#@#+js(set, ytInitialPlayerResponse.adPlacements, undefined)",
                false,
            ),
            rule("example.com##+js(set, a.b, true)", false),
        ])
        .unwrap()
        .unwrap();
        // youtube.com and example.com have entries; the excepted m.youtube.com has none.
        assert_eq!(program.hosts, 2);
        assert_eq!(program.calls, 2);
        assert!(program.script.contains("function zephiumSetConstant("));
        assert!(program
            .script
            .contains("ytInitialPlayerResponse.adPlacements"));
        assert_eq!(
            program
                .script
                .matches("function zephiumSetConstant(")
                .count(),
            1
        );
    }

    #[test]
    fn the_same_rules_build_the_same_program() {
        let rules: Vec<ScriptletRule> = (0..40)
            .map(|n| {
                rule(
                    &format!("a.com,b{}.com##+js(set, v{n}, true)", n % 3),
                    false,
                )
            })
            .collect();
        let first = build(&rules).unwrap().unwrap();
        for _ in 0..8 {
            let again = build(&rules).unwrap().unwrap();
            assert_eq!(again.script, first.script);
            assert_eq!(again.calls, first.calls);
        }
    }

    #[test]
    fn entities_are_keyed_by_their_name() {
        let program = build(&[rule("youtube.*##+js(set, a, true)", false)])
            .unwrap()
            .unwrap();
        assert_eq!(program.hosts, 1);
        assert!(program
            .script
            .contains(&format!("[{},0]", key_hash("youtube.*"))));
    }

    #[test]
    fn unknown_and_unsupported_rules_are_told_apart() {
        assert!(matches!(
            classify("a.com##+js(set, a, 1)"),
            Kind::Rule { unknown: false }
        ));
        assert!(matches!(
            classify("a.com##+js(no-such-scriptlet, a)"),
            Kind::Rule { unknown: true }
        ));
        assert!(matches!(
            classify("a.com#@#+js()"),
            Kind::Rule { unknown: false }
        ));
        assert!(matches!(classify("##+js(set, a, 1)"), Kind::Unsupported));
        assert!(matches!(
            classify("/^a\\.com$/##+js(set, a, 1)"),
            Kind::Unsupported
        ));
        assert!(matches!(
            classify("a.b.*##+js(set, a, 1)"),
            Kind::Unsupported
        ));
        assert!(matches!(classify("a.com##.ad"), Kind::None));
    }

    #[test]
    fn rules_whose_hosts_get_no_calls_build_no_program() {
        assert!(build(&[rule("a.com##+js(no-such-scriptlet, x)", false)])
            .unwrap()
            .is_none());
    }
}
