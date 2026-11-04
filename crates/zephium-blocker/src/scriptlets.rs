//! Scriptlets: functions from our own library, run in a page's main world at
//! document start with arguments from `##+js(...)` rules. adblock-rust matches
//! the rules to hosts, applies `#@#+js` exceptions and refuses `trusted-*`
//! scriptlets to lists without the trusted permission; this module supplies the
//! library as its resources and builds one program for every frame: a table
//! from hashed hostnames to the calls for that host, and the library, which
//! runs only when the frame's host has an entry. The library is reviewed code
//! shipped in the app and never fetched.
//!
//! The owner approved replacing response text with text from uBO's lists on
//! YouTube's hosts only: those rules are `trusted-*`, so only uBO's lists can
//! run them, and a rule naming any other host is not admitted.

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
/// Scriptlets that replace response text, and the only sites, each with its
/// subdomains, they run on.
const RESPONSE_TEXT: &[&str] = &[
    "trusted-replace-fetch-response",
    "trusted-rpfr",
    "trusted-replace-xhr-response",
];
const RESPONSE_TEXT_SITES: &[&str] = &["youtube.com", "youtube-nocookie.com"];

struct Scriptlet {
    name: &'static str,
    aliases: &'static [&'static str],
    trusted: bool,
    dependencies: &'static [&'static str],
    source: &'static str,
}

/// Helpers named here are dependencies, not rule names.
const HELPERS: &[Scriptlet] = &[
    Scriptlet {
        name: "zephium-constant.fn",
        aliases: &[],
        trusted: false,
        dependencies: &[],
        source: include_str!("scriptlets/constant.js"),
    },
    Scriptlet {
        name: "zephium-pattern.fn",
        aliases: &[],
        trusted: false,
        dependencies: &[],
        source: include_str!("scriptlets/pattern.js"),
    },
    Scriptlet {
        name: "zephium-json-pruner.fn",
        aliases: &[],
        trusted: false,
        dependencies: &[],
        source: include_str!("scriptlets/json-pruner.js"),
    },
    Scriptlet {
        name: "zephium-timer-booster.fn",
        aliases: &[],
        trusted: false,
        dependencies: &["zephium-pattern.fn"],
        source: include_str!("scriptlets/timer-booster.js"),
    },
    Scriptlet {
        name: "zephium-request-matcher.fn",
        aliases: &[],
        trusted: false,
        dependencies: &["zephium-pattern.fn"],
        source: include_str!("scriptlets/request-matcher.js"),
    },
    Scriptlet {
        name: "zephium-text-replacer.fn",
        aliases: &[],
        trusted: false,
        dependencies: &["zephium-pattern.fn"],
        source: include_str!("scriptlets/text-replacer.js"),
    },
];

const SCRIPTLETS: &[Scriptlet] = &[
    Scriptlet {
        name: "set-constant.js",
        aliases: &["set.js"],
        trusted: false,
        dependencies: &["zephium-constant.fn"],
        source: include_str!("scriptlets/set-constant.js"),
    },
    Scriptlet {
        name: "json-prune.js",
        aliases: &[],
        trusted: false,
        dependencies: &["zephium-pattern.fn", "zephium-json-pruner.fn"],
        source: include_str!("scriptlets/json-prune.js"),
    },
    Scriptlet {
        name: "json-prune-fetch-response.js",
        aliases: &[],
        trusted: false,
        dependencies: &[
            "zephium-pattern.fn",
            "zephium-json-pruner.fn",
            "zephium-request-matcher.fn",
        ],
        source: include_str!("scriptlets/json-prune-fetch-response.js"),
    },
    Scriptlet {
        name: "json-prune-xhr-response.js",
        aliases: &[],
        trusted: false,
        dependencies: &[
            "zephium-pattern.fn",
            "zephium-json-pruner.fn",
            "zephium-request-matcher.fn",
        ],
        source: include_str!("scriptlets/json-prune-xhr-response.js"),
    },
    Scriptlet {
        name: "trusted-replace-fetch-response.js",
        aliases: &["trusted-rpfr.js"],
        trusted: true,
        dependencies: &[
            "zephium-pattern.fn",
            "zephium-request-matcher.fn",
            "zephium-text-replacer.fn",
        ],
        source: include_str!("scriptlets/trusted-replace-fetch-response.js"),
    },
    Scriptlet {
        name: "trusted-replace-xhr-response.js",
        aliases: &[],
        trusted: true,
        dependencies: &[
            "zephium-pattern.fn",
            "zephium-request-matcher.fn",
            "zephium-text-replacer.fn",
        ],
        source: include_str!("scriptlets/trusted-replace-xhr-response.js"),
    },
    Scriptlet {
        name: "trusted-prevent-dom-bypass.js",
        aliases: &[],
        trusted: true,
        dependencies: &[],
        source: include_str!("scriptlets/trusted-prevent-dom-bypass.js"),
    },
    Scriptlet {
        name: "no-xhr-if.js",
        aliases: &["prevent-xhr.js"],
        trusted: false,
        dependencies: &["zephium-pattern.fn"],
        source: include_str!("scriptlets/no-xhr-if.js"),
    },
    Scriptlet {
        name: "nano-setTimeout-booster.js",
        aliases: &["nano-stb.js"],
        trusted: false,
        dependencies: &["zephium-pattern.fn", "zephium-timer-booster.fn"],
        source: include_str!("scriptlets/nano-settimeout-booster.js"),
    },
    Scriptlet {
        name: "nano-setInterval-booster.js",
        aliases: &["nano-sib.js"],
        trusted: false,
        dependencies: &["zephium-pattern.fn", "zephium-timer-booster.fn"],
        source: include_str!("scriptlets/nano-setinterval-booster.js"),
    },
];

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
    if !exception && RESPONSE_TEXT.contains(&name.strip_suffix(".js").unwrap_or(name)) {
        let mut named = domains
            .split(',')
            .filter(|domain| !domain.is_empty() && !domain.starts_with('~'))
            .map(str::to_ascii_lowercase)
            .peekable();
        let on_sites = named.peek().is_some()
            && named.all(|domain| {
                RESPONSE_TEXT_SITES.iter().any(|site| {
                    domain == *site
                        || domain
                            .strip_suffix(site)
                            .is_some_and(|sub| sub.ends_with('.'))
                })
            });
        if !on_sites {
            return Kind::Unsupported;
        }
    }
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
pub(crate) fn key_hash(key: &str) -> u64 {
    let fnv = |offset: u32| {
        key.bytes().fold(offset, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
        })
    };
    (u64::from(fnv(2_166_136_261)) << 21) | u64::from(fnv(0x8422_2325) >> 11)
}

/// Whether a page on `key` would reach another entry with calls once past
/// `key`'s own: a parent domain, or an entity named by one of its labels.
fn shadows_calls(key: &str, with_calls: &BTreeSet<&str>) -> bool {
    let labels: Vec<&str> = key.split('.').collect();
    if key.ends_with(".*") {
        return with_calls
            .iter()
            .any(|other| other.ends_with(".*") && *other != key);
    }
    (1..labels.len()).any(|i| with_calls.contains(labels[i..].join(".").as_str()))
        || entity_keys(&labels)
            .iter()
            .any(|entity| with_calls.contains(entity.as_str()))
}

/// Second-level labels taken as part of a public suffix, as in `co.uk`.
const SUFFIX_SECOND_LABELS: &[&str] = &[
    "ac", "biz", "co", "com", "edu", "go", "gob", "gouv", "gov", "info", "ltd", "mil", "ne", "net",
    "nic", "nom", "or", "org", "plc", "sch",
];

/// The entity keys a host's page looks up, most specific first: as in
/// adblock-rust, an entity is the label just before the public suffix,
/// approximated here as the last label or a known second level under it,
/// since the page has no public suffix list.
fn entity_keys(labels: &[&str]) -> Vec<String> {
    (0..labels.len().saturating_sub(1))
        .filter(|&i| {
            let rest = &labels[i + 1..];
            rest.len() == 1 || (rest.len() == 2 && SUFFIX_SECOND_LABELS.contains(&rest[0]))
        })
        .map(|i| format!("{}.*", labels[i]))
        .collect()
}

/// The same hash, in the page.
const PAGE_HASH: &str = "const fnv = (key, offset) => { let hash = offset; for (let i = 0; i < key.length; i++) { hash = Math.imul(hash ^ key.charCodeAt(i), 16777619); } return hash >>> 0; };\nconst hash = (key) => fnv(key, 2166136261) * 2097152 + (fnv(key, 0x84222325) >>> 11);";

/// The program every frame runs, built from the admitted rules.
pub(crate) struct Program {
    pub(crate) script: Arc<str>,
    pub(crate) hosts: usize,
    pub(crate) calls: usize,
    #[cfg(test)]
    table: Vec<(u64, usize)>,
    #[cfg(test)]
    runs: Vec<String>,
}

pub(crate) fn build(rules: &[ScriptletRule]) -> Result<Option<Program>, crate::CosmeticError> {
    if rules.len() > MAX_RULES {
        return Err(crate::CosmeticError::ResourceLimit);
    }
    let mut keys = BTreeSet::new();
    let (mut trusted, mut untrusted) = (Vec::new(), Vec::new());
    for rule in rules {
        // A stored policy is admitted again, so it cannot run what compiling
        // it would have left out.
        if !matches!(classify(&rule.line), Kind::Rule { .. }) {
            return Err(crate::CosmeticError::InvalidPolicy);
        }
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
    let mut hosts = 0;
    // Each key's call set, by its index in `calls`; None when it has none.
    let mut found: Vec<(&String, Option<usize>)> = Vec::with_capacity(keys.len());
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
        blocks.sort_unstable();
        let index = (!blocks.is_empty()).then(|| {
            let next = calls.len();
            *calls.entry(blocks.join("\n")).or_insert(next)
        });
        found.push((key, index));
    }
    let with_calls: BTreeSet<&str> = found
        .iter()
        .filter(|(_, index)| index.is_some())
        .map(|(key, _)| key.as_str())
        .collect();
    if with_calls.is_empty() {
        return Ok(None);
    }
    for (key, index) in found {
        let index = match index {
            Some(index) => {
                hosts += 1;
                index
            }
            // A host whose calls are all excepted keeps an empty entry only
            // where the page would otherwise go on to a parent's calls.
            None if shadows_calls(key, &with_calls) => {
                let next = calls.len();
                *calls.entry(String::new()).or_insert(next)
            }
            None => continue,
        };
        table.push((key_hash(key), index));
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
    let second = serde_json::to_string(SUFFIX_SECOND_LABELS)
        .map_err(|_| crate::CosmeticError::InvalidPolicy)?;
    let script = format!(
        "(() => {{\n{PAGE_HASH}\nconst table = new Map([{}]);\nconst host = location.hostname.replace(/\\.$/, '');\nif (host === '') {{ return; }}\nconst labels = host.split('.');\nlet entry;\nfor (let i = 0; i < labels.length && entry === undefined; i++) {{ entry = table.get(hash(labels.slice(i).join('.'))); }}\nconst second = new Set({second});\nconst suffix = (rest) => rest.length === 1 || (rest.length === 2 && second.has(rest[0]));\nfor (let i = 0; i < labels.length - 1 && entry === undefined; i++) {{ if (suffix(labels.slice(i + 1))) {{ entry = table.get(hash(labels[i] + '.*')); }} }}\nif (entry === undefined) {{ return; }}\n(function () {{\n{}\nconst runs = [{}];\nruns[entry]();\n}})();\n}})();\n",
        entries.join(","),
        library.join("\n"),
        runs.join(",\n"),
    );
    if script.len() > MAX_PROGRAM_BYTES {
        return Err(crate::CosmeticError::ResourceLimit);
    }
    Ok(Some(Program {
        script: script.into(),
        hosts,
        calls: ordered
            .iter()
            .filter(|(invocations, _)| !invocations.is_empty())
            .count(),
        #[cfg(test)]
        table,
        #[cfg(test)]
        runs: ordered
            .iter()
            .map(|(invocations, _)| (*invocations).clone())
            .collect(),
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
    fn a_response_rule_reaches_its_scriptlet_with_its_pattern_intact() {
        let program = build(&[rule(
            r"www.youtube.com##+js(json-prune-fetch-response, adPlacements adSlots playerResponse.adPlacements, , propsToMatch, /\/player(\?|$)/)",
            false,
        )])
        .unwrap()
        .unwrap();
        for helper in [
            "zephiumPattern(",
            "zephiumJsonPruner(",
            "zephiumRequestMatcher(",
        ] {
            assert_eq!(
                program
                    .script
                    .matches(&format!("function {helper}"))
                    .count(),
                1,
                "{helper}"
            );
        }
        assert!(program.script.contains(r#"zephiumJsonPruneFetchResponse("adPlacements adSlots playerResponse.adPlacements", "", "propsToMatch", "/\\/player(\\?|$)/")"#));
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
    fn response_text_is_replaced_only_on_youtube_s_hosts() {
        for line in [
            "www.youtube.com##+js(trusted-rpfr, '\"adSlots\"', '\"no_ads\"', player?)",
            "youtube.com,~m.youtube.com##+js(trusted-replace-fetch-response.js, *, , x)",
            "www.youtube-nocookie.com##+js(trusted-replace-xhr-response, a, b)",
            "example.com#@#+js(trusted-rpfr, a, b)",
        ] {
            assert!(
                matches!(classify(line), Kind::Rule { unknown: false }),
                "{line}"
            );
        }
        for line in [
            "example.com##+js(trusted-rpfr, a, b)",
            "youtube.com,example.com##+js(trusted-replace-fetch-response, a, b)",
            "youtube.*##+js(trusted-replace-xhr-response, a, b)",
            "notyoutube.com##+js(trusted-rpfr, a, b)",
            "~example.com##+js(trusted-rpfr, a, b)",
        ] {
            assert!(matches!(classify(line), Kind::Unsupported), "{line}");
        }
    }

    #[test]
    fn response_text_rules_run_only_from_trusted_lists_with_their_arguments_intact() {
        let line = r#"www.youtube.com##+js(trusted-replace-xhr-response, /"adPlacements.*?("adSlots"|"adBreakHeartbeatParams")/gms, $1, /\/player(?:\?.+)?$/)"#;
        assert!(build(&[rule(line, false)]).unwrap().is_none());
        let program = build(&[
            rule(line, true),
            rule(
                "www.youtube.com##+js(trusted-rpfr, '\"adPlacements\"', '\"no_ads\"', player?)",
                true,
            ),
        ])
        .unwrap()
        .unwrap();
        assert!(program.script.contains(r#"zephiumTrustedReplaceXhrResponse("/\"adPlacements.*?(\"adSlots\"|\"adBreakHeartbeatParams\")/gms", "$1", "/\\/player(?:\\?.+)?$/")"#));
        assert!(program.script.contains(
            r#"zephiumTrustedReplaceFetchResponse("\"adPlacements\"", "\"no_ads\"", "player?")"#
        ));
        assert_eq!(
            program
                .script
                .matches("function zephiumTextReplacer(")
                .count(),
            1
        );
    }

    #[test]
    fn youtube_s_bypass_and_xhr_rules_reach_their_scriptlets() {
        let bypass =
            "www.youtube.com##+js(trusted-prevent-dom-bypass, Node.prototype.appendChild, fetch)";
        let xhr = r"www.youtube.com##+js(no-xhr-if, /\/api\/stats\/atr\?.+?&fexp=v1%[-%0-9C]{300\,}&/ method:POST)";
        let untrusted = build(&[rule(bypass, false), rule(xhr, false)])
            .unwrap()
            .unwrap();
        assert!(!untrusted
            .script
            .contains("zephiumTrustedPreventDomBypass(\""));
        assert!(untrusted.script.contains(
            r#"zephiumNoXhrIf("/\\/api\\/stats\\/atr\\?.+?&fexp=v1%[-%0-9C]{300,}&/ method:POST")"#
        ));
        let trusted = build(&[rule(bypass, true)]).unwrap().unwrap();
        assert!(trusted
            .script
            .contains(r#"zephiumTrustedPreventDomBypass("Node.prototype.appendChild", "fetch")"#));
    }

    #[test]
    fn rules_whose_hosts_get_no_calls_build_no_program() {
        assert!(build(&[rule("a.com##+js(no-such-scriptlet, x)", false)])
            .unwrap()
            .is_none());
    }

    /// The calls a page on `host` runs, found the way the page finds them.
    fn calls_for<'a>(program: &'a Program, host: &str) -> Option<&'a str> {
        let labels: Vec<&str> = host.split('.').collect();
        let entry = (0..labels.len())
            .map(|i| labels[i..].join("."))
            .chain(entity_keys(&labels))
            .find_map(|key| {
                program
                    .table
                    .iter()
                    .find(|(hash, _)| *hash == key_hash(&key))
                    .map(|(_, index)| *index)
            })?;
        Some(program.runs[entry].as_str())
    }

    #[test]
    fn an_excepted_subdomain_runs_none_of_its_parents_excepted_calls() {
        let program = build(&[
            rule("youtube.com##+js(set, a.b, true)", false),
            rule("youtube.com##+js(set, c.d, true)", false),
            rule("m.youtube.com#@#+js(set, a.b, true)", false),
            rule("music.youtube.com#@#+js()", false),
        ])
        .unwrap()
        .unwrap();
        let parent = calls_for(&program, "www.youtube.com").unwrap();
        assert!(parent.contains("\"a.b\"") && parent.contains("\"c.d\""));
        let mobile = calls_for(&program, "m.youtube.com").unwrap();
        assert!(!mobile.contains("\"a.b\"") && mobile.contains("\"c.d\""));
        assert_eq!(calls_for(&program, "music.youtube.com"), Some(""));
        assert_eq!(calls_for(&program, "example.com"), None);
    }

    #[test]
    fn a_stored_rule_admission_would_refuse_is_invalid() {
        for line in [
            "##+js(set, a, 1)",
            "a.b.*##+js(set, a, 1)",
            "example.com##+js(trusted-rpfr, a, b)",
            "a.com##.ad",
            &format!("a.com##+js(set, a, {})", "x".repeat(9 * 1024)),
        ] {
            assert_eq!(
                build(&[rule(line, false)]).err(),
                Some(crate::CosmeticError::InvalidPolicy),
                "{line}"
            );
        }
    }

    #[test]
    fn an_empty_entry_is_kept_only_where_it_shadows_calls() {
        let program = build(&[
            rule("youtube.com##+js(set, a.b, true)", false),
            rule("example.com#@#+js()", false),
            rule("m.youtube.com#@#+js()", false),
        ])
        .unwrap()
        .unwrap();
        let has_entry = |key: &str| program.table.iter().any(|(hash, _)| *hash == key_hash(key));
        assert!(has_entry("youtube.com") && has_entry("m.youtube.com"));
        assert!(!has_entry("example.com"));
    }

    #[test]
    fn entities_are_the_label_before_the_public_suffix() {
        let program = build(&[
            rule("youtube.*##+js(set, a.b, true)", false),
            rule("example.*##+js(set, c.d, true)", false),
            rule("example.com#@#+js()", false),
        ])
        .unwrap()
        .unwrap();
        assert!(calls_for(&program, "www.youtube.co.uk")
            .unwrap()
            .contains("\"a.b\""));
        assert!(calls_for(&program, "youtube.de")
            .unwrap()
            .contains("\"a.b\""));
        // youtube is not this host's entity: example is.
        assert!(!calls_for(&program, "youtube.example.org")
            .unwrap()
            .contains("\"a.b\""));
        // The excepted domain stops its subdomains before the entity's calls.
        assert_eq!(calls_for(&program, "m.example.com"), Some(""));
        assert!(calls_for(&program, "example.net")
            .unwrap()
            .contains("\"c.d\""));
    }
}
