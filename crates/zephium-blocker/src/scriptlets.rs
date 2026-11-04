//! Scriptlets: functions from our own library, run in a page's main world at
//! document start with arguments from `##+js(...)` rules. adblock-rust matches
//! the rules to hosts, applies `#@#+js` exceptions and refuses `trusted-*`
//! scriptlets to lists without the trusted permission; this module supplies the
//! library as its resources and builds one program for every frame: a table
//! from hashed hostnames to the calls for that host, and the library, which
//! runs only when the frame's host has an entry. The library is reviewed code
//! shipped in the app and never fetched.
//!
//! One owner-approved exception: the text `rpnt` writes into YouTube's own
//! scripts, and the values `trusted-edit-inbound-object` writes into its
//! requests, come from uBO's lists. They are admitted only from those lists,
//! only for YouTube's hosts, and within fixed sizes; anything else is dropped
//! and counted.

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
/// Scriptlets whose arguments are page code from the list.
const PAGE_CODE: &[&str] = &[
    "trusted-replace-node-text",
    "trusted-rpnt",
    "replace-node-text",
    "rpnt",
    "trusted-edit-inbound-object",
];
/// The only sites page code runs on, each with its subdomains.
const PAGE_CODE_SITES: &[&str] = &[
    "youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "www.youtube-nocookie.com",
];
/// The library functions that PAGE_CODE rules call.
const PAGE_CODE_FUNCTIONS: &[&str] = &["zephiumReplaceNodeText", "zephiumTrustedEditInboundObject"];
const MAX_PAGE_CODE_RULE_BYTES: usize = 16 * 1024;
const MAX_PAGE_CODE_SITE_BYTES: usize = 64 * 1024;

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
        name: "zephium-node-text.fn",
        aliases: &[],
        trusted: false,
        dependencies: &["zephium-pattern.fn"],
        source: include_str!("scriptlets/node-text.js"),
    },
    Scriptlet {
        name: "zephium-json-path.fn",
        aliases: &[],
        trusted: false,
        dependencies: &[],
        source: include_str!("scriptlets/json-path.js"),
    },
    Scriptlet {
        name: "zephium-inbound-object.fn",
        aliases: &[],
        trusted: false,
        dependencies: &["zephium-json-path.fn"],
        source: include_str!("scriptlets/inbound-object.js"),
    },
    Scriptlet {
        name: "zephium-page-code-site.fn",
        aliases: &[],
        trusted: false,
        dependencies: &[],
        source: include_str!("scriptlets/page-code-site.js"),
    },
    Scriptlet {
        name: "zephium-request-matcher.fn",
        aliases: &[],
        trusted: false,
        dependencies: &["zephium-pattern.fn"],
        source: include_str!("scriptlets/request-matcher.js"),
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
    Scriptlet {
        name: "trusted-replace-node-text.js",
        aliases: &["trusted-rpnt.js", "replace-node-text.js", "rpnt.js"],
        trusted: true,
        dependencies: &["zephium-pattern.fn", "zephium-node-text.fn"],
        source: include_str!("scriptlets/replace-node-text.js"),
    },
    Scriptlet {
        name: "remove-node-text.js",
        aliases: &["rmnt.js"],
        trusted: false,
        dependencies: &["zephium-pattern.fn", "zephium-node-text.fn"],
        source: include_str!("scriptlets/remove-node-text.js"),
    },
    Scriptlet {
        name: "edit-inbound-object.js",
        aliases: &[],
        trusted: false,
        dependencies: &["zephium-json-path.fn", "zephium-inbound-object.fn"],
        source: include_str!("scriptlets/edit-inbound-object.js"),
    },
    Scriptlet {
        name: "trusted-edit-inbound-object.js",
        aliases: &[],
        trusted: true,
        dependencies: &["zephium-json-path.fn", "zephium-inbound-object.fn"],
        source: include_str!("scriptlets/trusted-edit-inbound-object.js"),
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
    /// A rule to keep, whether its scriptlet is one the library lacks, and
    /// the bytes of page code it carries.
    Rule { unknown: bool, page_code: usize },
    /// Scriptlet syntax we do not run: too long, no hostname, or a hostname
    /// form other than names and single-label entities.
    Unsupported,
    /// Page code from a list without trust, for a site outside
    /// `PAGE_CODE_SITES`, or larger than one rule may carry.
    RefusedPageCode,
}

pub(crate) fn classify(line: &str, trusted: bool) -> Kind {
    let Some((domains, body)) = line
        .split_once("##+js(")
        .or_else(|| line.split_once("#@#+js("))
    else {
        return Kind::None;
    };
    if !body.ends_with(')') {
        return Kind::Unsupported;
    }
    let exception = line.contains("#@#+js(");
    let keys = domain_keys(domains);
    if keys.is_none() || (!exception && domains.is_empty()) {
        return Kind::Unsupported;
    }
    let arguments = &body[..body.len() - 1];
    let name = arguments
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    let name = name.strip_suffix(".js").unwrap_or(name);
    if !exception && PAGE_CODE.contains(&name) {
        let bytes = arguments.len();
        if !trusted || bytes > MAX_PAGE_CODE_RULE_BYTES || page_code_sites(domains).is_none() {
            return Kind::RefusedPageCode;
        }
        return Kind::Rule {
            unknown: false,
            page_code: bytes,
        };
    }
    if line.len() > MAX_RULE_BYTES {
        return Kind::Unsupported;
    }
    Kind::Rule {
        unknown: !exception && !known(name),
        page_code: 0,
    }
}

/// The `PAGE_CODE_SITES` entries a page-code rule falls under, when it names
/// at least one site and every one it names is one of them or below one.
/// Excluded sites only narrow it.
fn page_code_sites(domains: &str) -> Option<BTreeSet<&'static str>> {
    domain_keys(domains)?;
    let mut sites = BTreeSet::new();
    for domain in domains.split(',') {
        if domain.is_empty() || domain.starts_with('~') {
            continue;
        }
        let domain = domain.to_ascii_lowercase();
        // The first entry a host is under, so all of youtube.com shares one total.
        let site = PAGE_CODE_SITES.iter().find(|site| {
            domain == **site
                || domain
                    .strip_suffix(**site)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })?;
        sites.insert(*site);
    }
    (!sites.is_empty()).then_some(sites)
}

/// Page code admitted so far under each of `PAGE_CODE_SITES`.
#[derive(Default)]
pub(crate) struct PageCodeBudget(BTreeMap<&'static str, usize>);

impl PageCodeBudget {
    /// Takes a page-code rule's bytes under every site it names, or under
    /// none when one would pass `MAX_PAGE_CODE_SITE_BYTES`.
    pub(crate) fn admit(&mut self, line: &str, bytes: usize) -> bool {
        let Some(sites) = line
            .split_once("##+js(")
            .and_then(|(domains, _)| page_code_sites(domains))
        else {
            return false;
        };
        let fits = sites.iter().all(|site| {
            self.0.get(site).copied().unwrap_or(0).saturating_add(bytes) <= MAX_PAGE_CODE_SITE_BYTES
        });
        if fits {
            for site in sites {
                *self.0.entry(site).or_default() += bytes;
            }
        }
        fits
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
    // A stored policy is admitted again, so page code it holds keeps its limits.
    let mut budget = PageCodeBudget::default();
    for rule in rules {
        match classify(&rule.line, rule.trusted) {
            Kind::Rule { page_code: 0, .. } => {}
            Kind::Rule { page_code, .. } if budget.admit(&rule.line, page_code) => {}
            _ => return Err(crate::CosmeticError::InvalidPolicy),
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
        // Page code runs only where the page's real host, not the hash that
        // found this entry, is under one of the sites it was admitted for.
        let (page_code, mut blocks): (Vec<&str>, Vec<&str>) =
            blocks.into_iter().partition(|block| {
                PAGE_CODE_FUNCTIONS
                    .iter()
                    .any(|function| block.contains(&format!("{function}(")))
            });
        let sites: Vec<&str> = PAGE_CODE_SITES
            .iter()
            .copied()
            .filter(|site| {
                key == site
                    || key
                        .strip_suffix(site)
                        .is_some_and(|prefix| prefix.ends_with('.'))
            })
            .collect();
        let guarded;
        if !page_code.is_empty() && !sites.is_empty() {
            guarded = format!(
                "if (zephiumPageCodeSite({})) {{\n{}\n}}",
                serde_json::to_string(&sites).map_err(|_| crate::CosmeticError::InvalidPolicy)?,
                page_code.join("\n")
            );
            blocks.push(&guarded);
        }
        if blocks.is_empty() {
            continue;
        }
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
            classify("a.com##+js(set, a, 1)", false),
            Kind::Rule { unknown: false, .. }
        ));
        assert!(matches!(
            classify("a.com##+js(no-such-scriptlet, a)", false),
            Kind::Rule { unknown: true, .. }
        ));
        assert!(matches!(
            classify("a.com#@#+js()", false),
            Kind::Rule { unknown: false, .. }
        ));
        assert!(matches!(
            classify("##+js(set, a, 1)", false),
            Kind::Unsupported
        ));
        assert!(matches!(
            classify("/^a\\.com$/##+js(set, a, 1)", false),
            Kind::Unsupported
        ));
        assert!(matches!(
            classify("a.b.*##+js(set, a, 1)", false),
            Kind::Unsupported
        ));
        assert!(matches!(classify("a.com##.ad", false), Kind::None));
    }

    #[test]
    fn rules_whose_hosts_get_no_calls_build_no_program() {
        assert!(build(&[rule("a.com##+js(no-such-scriptlet, x)", false)])
            .unwrap()
            .is_none());
    }

    #[test]
    fn page_code_runs_only_from_trusted_lists_on_youtube() {
        let code = "##+js(rpnt, script, /(start\\(\\))/, prepare();$1, sedCount, 1)";
        let page_code = |line: &str, trusted: bool| match classify(line, trusted) {
            Kind::Rule { page_code, .. } => Some(page_code),
            Kind::RefusedPageCode => None,
            _ => panic!("{line}"),
        };
        for site in [
            "youtube.com",
            "www.youtube.com",
            "m.youtube.com",
            "music.youtube.com",
            "www.youtube-nocookie.com",
            "www.youtube.com,~tv.youtube.com",
        ] {
            assert!(
                page_code(&format!("{site}{code}"), true).is_some_and(|bytes| bytes > 0),
                "{site}"
            );
            assert_eq!(page_code(&format!("{site}{code}"), false), None, "{site}");
        }
        for site in [
            "example.com",
            "youtube.*",
            "notyoutube.com",
            "youtube.com.example",
            "youtube-nocookie.com",
            "www.youtube.com,example.com",
            "~example.com",
        ] {
            assert_eq!(page_code(&format!("{site}{code}"), true), None, "{site}");
        }
        for name in [
            "trusted-rpnt",
            "replace-node-text",
            "trusted-replace-node-text",
            "trusted-edit-inbound-object",
        ] {
            assert_eq!(
                page_code(&format!("example.com##+js({name}, a, b, c)"), true),
                None,
                "{name}"
            );
        }
        // Other scriptlets keep their own rules, and exceptions carry no code.
        assert_eq!(
            page_code("example.com##+js(rmnt, script, a)", true),
            Some(0)
        );
        assert_eq!(
            page_code("example.com#@#+js(rpnt, script, a, b)", false),
            Some(0)
        );
    }

    #[test]
    fn page_code_has_a_size_per_rule_and_per_site() {
        let rule = |bytes: usize| {
            format!(
                "www.youtube.com##+js(rpnt, script, a, {})",
                "x".repeat(bytes)
            )
        };
        assert!(matches!(classify(&rule(16_000), true), Kind::Rule { .. }));
        assert!(matches!(
            classify(&rule(17_000), true),
            Kind::RefusedPageCode
        ));
        let mut budget = PageCodeBudget::default();
        let line = rule(15_000);
        let Kind::Rule { page_code, .. } = classify(&line, true) else {
            panic!("page code refused");
        };
        for _ in 0..4 {
            assert!(budget.admit(&line, page_code));
        }
        // youtube.com and its subdomains share one total; nocookie has its own.
        assert!(!budget.admit(&line.replacen("www.", "m.", 1), page_code));
        assert!(budget.admit(
            &line.replacen("www.youtube.com", "www.youtube-nocookie.com", 1),
            page_code
        ));
    }

    #[test]
    fn a_stored_policy_cannot_carry_page_code_past_its_limits() {
        let line = "example.com##+js(rpnt, script, a, b)";
        assert_eq!(
            build(&[rule(line, true)]).err(),
            Some(crate::CosmeticError::InvalidPolicy)
        );
        let youtube = "www.youtube.com##+js(rpnt, script, a, b)";
        assert_eq!(
            build(&[rule(youtube, false)]).err(),
            Some(crate::CosmeticError::InvalidPolicy)
        );
        let program = build(&[rule(youtube, true)]).unwrap().unwrap();
        assert!(program.script.contains("function zephiumReplaceNodeText("));
    }

    #[test]
    fn page_code_calls_run_only_behind_the_text_host_check() {
        let program = build(&[
            rule("www.youtube.com##+js(rpnt, script, a, b)", true),
            rule("www.youtube.com##+js(set, a.b, true)", false),
        ])
        .unwrap()
        .unwrap();
        let script = program.script.as_ref();
        let guard = script
            .find("if (zephiumPageCodeSite([\"youtube.com\"])) {\n")
            .expect("guard");
        let page_code = script.find("zephiumReplaceNodeText(\"script\"").unwrap();
        let plain = script.find("zephiumSetConstant(\"a.b\"").unwrap();
        assert!(plain < guard && guard < page_code);
        assert_eq!(script.matches("zephiumReplaceNodeText(\"").count(), 1);
    }
}
