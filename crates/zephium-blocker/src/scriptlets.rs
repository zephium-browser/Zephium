//! Scriptlets: functions from our own library, run in a page's main world at
//! document start with arguments from `##+js(...)` rules. adblock-rust matches
//! the rules to hosts, applies `#@#+js` exceptions and refuses `trusted-*`
//! scriptlets to lists without the trusted permission; this module supplies the
//! library as its resources and builds one program for every frame: a table
//! from hashed hostnames to the calls for that host, and the library, which
//! runs only when the frame's host has an entry. The library is reviewed code
//! shipped in the app and never fetched.
//!
//! The owner approved replacing response text, and editing the objects the
//! player sends, with values from uBO's lists on YouTube's hosts only: those
//! rules are `trusted-*`, so only uBO's lists can run them, and a rule naming
//! any other host is not admitted.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use adblock::lists::{FilterSet, ParseOptions, RuleTypes};
use adblock::resources::{MimeType, PermissionMask, Resource, ResourceType};
use adblock::Engine;
use base64::Engine as _;

mod object_query;

/// The permission `trusted-*` scriptlets need and only uBO's lists grant.
const TRUSTED: PermissionMask = PermissionMask::from_bits(1);
/// The permission Zephium's own scriptlets need and only its built-in rules
/// grant; those rules never hold [`TRUSTED`].
const ZEPHIUM: PermissionMask = PermissionMask::from_bits(2);
/// The scriptlet that marks YouTube's client for uBO's player-request edits.
const PLAYER_MARKER: &str = "zephium-player-marker";
/// Ends every library function, so a host's calls are what follows the last.
const END: &str = "/* zephium scriptlet end */";
/// How adblock-rust closes each call it emits.
const CALL_END: &str = "} catch ( e ) { }";
const MAX_RULES: usize = 20_000;
const MAX_RULE_BYTES: usize = 8 * 1024;
const MAX_PROGRAM_BYTES: usize = 4 * 1024 * 1024;
/// Scriptlets that replace response text or edit what the page sends, and the
/// only sites, each with its subdomains, they run on.
const YOUTUBE_ONLY: &[&str] = &[
    "trusted-replace-fetch-response",
    "trusted-rpfr",
    "trusted-replace-xhr-response",
    "trusted-edit-inbound-object",
    PLAYER_MARKER,
];
const YOUTUBE_SITES: &[&str] = &["youtube.com", "youtube-nocookie.com"];
const EDIT_INBOUND_OBJECT: &str = "trusted-edit-inbound-object";

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
    Scriptlet {
        name: "zephium-object-editor.fn",
        aliases: &[],
        trusted: false,
        dependencies: &[],
        source: include_str!("scriptlets/object-editor.js"),
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
        name: "trusted-edit-inbound-object.js",
        aliases: &[],
        trusted: true,
        dependencies: &["zephium-object-editor.fn"],
        source: include_str!("scriptlets/trusted-edit-inbound-object.js"),
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

/// Zephium's own scriptlets, which only its built-in rules can call.
const ZEPHIUM_SCRIPTLETS: &[Scriptlet] = &[Scriptlet {
    name: "zephium-player-marker.js",
    aliases: &[],
    trusted: false,
    dependencies: &[],
    source: include_str!("scriptlets/zephium-player-marker.js"),
}];

/// Every library function, with the permission a rule needs to call it.
fn library() -> impl Iterator<Item = (&'static Scriptlet, PermissionMask)> {
    HELPERS
        .iter()
        .chain(SCRIPTLETS)
        .map(|scriptlet| {
            let permission = if scriptlet.trusted {
                TRUSTED
            } else {
                PermissionMask::default()
            };
            (scriptlet, permission)
        })
        .chain(
            ZEPHIUM_SCRIPTLETS
                .iter()
                .map(|scriptlet| (scriptlet, ZEPHIUM)),
        )
}

fn resources() -> Vec<Resource> {
    library()
        .map(|(scriptlet, permission)| Resource {
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
            permission,
        })
        .collect()
}

/// Whether the library has a scriptlet by this rule name.
fn known(name: &str) -> bool {
    let name = name.strip_suffix(".js").unwrap_or(name);
    SCRIPTLETS
        .iter()
        .chain(ZEPHIUM_SCRIPTLETS)
        .any(|scriptlet| {
            std::iter::once(scriptlet.name)
                .chain(scriptlet.aliases.iter().copied())
                .any(|candidate| candidate.strip_suffix(".js") == Some(name))
        })
}

/// Which scriptlets a list's rules may call beyond the plain library.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum ScriptletTrust {
    /// The plain library only.
    #[default]
    Plain,
    /// uBlock Origin's own lists: `trusted-*` scriptlets too.
    Trusted,
    /// Zephium's built-in rules: Zephium's scriptlets too, never `trusted-*`.
    Zephium,
}

impl ScriptletTrust {
    const fn permissions(self) -> PermissionMask {
        match self {
            Self::Plain => PermissionMask::from_bits(0),
            Self::Trusted => TRUSTED,
            Self::Zephium => ZEPHIUM,
        }
    }
}

/// One `##+js` rule as admitted, with its list's trust.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScriptletRule {
    pub(crate) line: String,
    pub(crate) trust: ScriptletTrust,
}

/// What a line is, for admission and counting.
pub(crate) enum Kind {
    /// Not scriptlet syntax.
    None,
    /// A rule to keep, whether its scriptlet is one the library lacks, and
    /// whether it runs with a stand-in for what it names, such as an empty
    /// body for a resource the app does not ship.
    Rule { unknown: bool, substituted: bool },
    /// Scriptlet syntax we do not run: too long, no hostname, or a hostname
    /// form other than names and single-label entities.
    Unsupported,
}

/// A scriptlet line's hostnames and body, and whether it is an exception.
/// Whichever marker comes first decides, as adblock-rust reads it; a marker
/// inside an argument is text. Admission and building both split here.
fn split(line: &str) -> Option<(bool, &str, &str)> {
    let (exception, at, marker) = match (line.find("##+js("), line.find("#@#+js(")) {
        (Some(rule), Some(except)) if except < rule => (true, except, "#@#+js("),
        (Some(rule), _) => (false, rule, "##+js("),
        (None, Some(except)) => (true, except, "#@#+js("),
        (None, None) => return None,
    };
    Some((exception, &line[..at], &line[at + marker.len()..]))
}

pub(crate) fn classify(line: &str) -> Kind {
    let Some((exception, domains, body)) = split(line) else {
        return Kind::None;
    };
    if line.len() > MAX_RULE_BYTES || !body.ends_with(')') {
        return Kind::Unsupported;
    }
    let keys = domain_keys(domains);
    if keys.is_none() || (!exception && domains.is_empty()) {
        return Kind::Unsupported;
    }
    let arguments = arguments(&body[..body.len() - 1]);
    let name = arguments.first().copied().unwrap_or_default();
    if !exception && YOUTUBE_ONLY.contains(&name.strip_suffix(".js").unwrap_or(name)) {
        let mut named = domains
            .split(',')
            .filter(|domain| !domain.is_empty() && !domain.starts_with('~'))
            .map(str::to_ascii_lowercase)
            .peekable();
        let on_sites = named.peek().is_some()
            && named.all(|domain| {
                YOUTUBE_SITES.iter().any(|site| {
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
    let bare = name.strip_suffix(".js").unwrap_or(name);
    if !exception && bare == EDIT_INBOUND_OBJECT && !edits_an_object(&body[..body.len() - 1]) {
        return Kind::Unsupported;
    }
    Kind::Rule {
        unknown: !exception && !known(name),
        substituted: !exception
            && matches!(bare, "no-xhr-if" | "prevent-xhr")
            && arguments
                .get(2)
                .is_some_and(|directive| directive.starts_with("war:")),
    }
}

/// Keeps in Zephium's marker rules only markers uBO's admitted edit rules
/// both key an edit on and strip from comment requests, on every host the
/// marker rule names, and drops a marker rule left with none, so a marker uBO
/// has renamed or scoped elsewhere is never sent. An exception that turns off
/// object edits, or every scriptlet, on one of those hosts leaves it none.
/// Returns how many markers were dropped.
pub(crate) fn keep_edited_markers(rules: &mut Vec<ScriptletRule>) -> usize {
    let mut edits = Vec::new();
    let mut exceptions = Vec::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.trust != ScriptletTrust::Zephium)
    {
        let Some((exception, domains, body)) = split(&rule.line) else {
            continue;
        };
        if exception {
            let name = arguments(body.strip_suffix(')').unwrap_or(body))
                .first()
                .copied()
                .unwrap_or_default();
            if name.is_empty() || name.strip_suffix(".js").unwrap_or(name) == EDIT_INBOUND_OBJECT {
                exceptions.push(domains.to_owned());
            }
        } else if rule.trust == ScriptletTrust::Trusted {
            if let Some(query) = rule_arguments(&rule.line, EDIT_INBOUND_OBJECT)
                .and_then(|arguments| arguments.into_iter().nth(3))
            {
                edits.push((domains.to_owned(), query));
            }
        }
    }
    let mut dropped = 0;
    rules.retain_mut(|rule| {
        if rule.trust != ScriptletTrust::Zephium {
            return true;
        }
        let Some(mut arguments) = rule_arguments(&rule.line, PLAYER_MARKER) else {
            return true;
        };
        let hosts: Vec<&str> = split(&rule.line)
            .map(|(_, domains, _)| domains)
            .unwrap_or_default()
            .split(',')
            .filter(|host| !host.is_empty() && !host.starts_with('~'))
            .collect();
        let (mut keyed, mut stripped) = (BTreeSet::new(), BTreeSet::new());
        let excepted = exceptions
            .iter()
            .any(|domains| hosts.iter().any(|host| applies(domains, host)));
        if !excepted && !hosts.is_empty() {
            for (domains, query) in &edits {
                if hosts.iter().all(|host| applies(domains, host)) {
                    user_agent_markers(query, &mut keyed, &mut stripped);
                }
            }
        }
        let Some(markers) = arguments.get(1) else {
            return true;
        };
        let (kept, lost): (Vec<&str>, Vec<&str>) = markers
            .split_whitespace()
            .partition(|marker| keyed.contains(*marker) && stripped.contains(*marker));
        dropped += lost.len();
        if kept.is_empty() {
            return false;
        }
        let kept = kept.join(" ");
        arguments[1] = kept;
        let Some((_, domains, _)) = split(&rule.line) else {
            return false;
        };
        let body: Vec<String> = arguments
            .iter()
            .map(|argument| argument.replace(',', "\\,"))
            .collect();
        rule.line = format!("{domains}##+js({})", body.join(", "));
        true
    });
    dropped
}

/// Whether a rule scoped to `domains` runs on `host`: a generic rule runs
/// everywhere, a named one on that host and its subdomains, unless a negated
/// entry names the host or one of its parents.
fn applies(domains: &str, host: &str) -> bool {
    let within = |name: &str| {
        let name = name.to_ascii_lowercase();
        host == name
            || host
                .strip_suffix(name.as_str())
                .is_some_and(|sub| sub.ends_with('.'))
    };
    let entries: Vec<&str> = domains
        .split(',')
        .filter(|entry| !entry.is_empty())
        .collect();
    let named: Vec<&str> = entries
        .iter()
        .filter(|entry| !entry.starts_with('~'))
        .copied()
        .collect();
    let excluded = entries
        .iter()
        .filter_map(|entry| entry.strip_prefix('~'))
        .any(within);
    !excluded && (named.is_empty() || named.into_iter().any(within))
}

/// A non-exception rule's arguments as the page receives them, when it calls
/// `scriptlet`.
fn rule_arguments(line: &str, scriptlet: &str) -> Option<Vec<String>> {
    let (exception, _, body) = split(line)?;
    let arguments = page_arguments(body.strip_suffix(')')?)?;
    let name = arguments.first()?;
    (!exception && name.strip_suffix(".js").unwrap_or(name) == scriptlet).then_some(arguments)
}

/// The markers an edit query keys on in the client's user agent, and those it
/// strips from it.
fn user_agent_markers(query: &str, keyed: &mut BTreeSet<String>, stripped: &mut BTreeSet<String>) {
    // Hyphens stay inside a word, so a renamed `lactmilli-v2` is not read as
    // `lactmilli`.
    let words = |text: &str| -> Vec<String> {
        text.split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect()
    };
    for (marker, end) in [("userAgent*=\"", '"'), ("userAgent=/", '/')] {
        for (at, _) in query.match_indices(marker) {
            let rest = &query[at + marker.len()..];
            if let Some(length) = rest.find(end) {
                keyed.extend(words(&rest[..length]));
            }
        }
    }
    if let Some((_, rest)) = query.split_once("userAgent=repl(") {
        stripped.extend(words(rest));
    }
}

/// Whether an edit-inbound-object rule names a method chain, an argument
/// position and a query the page reads, exactly as the page receives them.
fn edits_an_object(body: &str) -> bool {
    let Some(arguments) = page_arguments(body) else {
        return false;
    };
    let [_, chain, position, query] = arguments.as_slice() else {
        return false;
    };
    let identifier = |name: &str| {
        name.bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
    };
    // A method has far fewer arguments than this; a longer position would not
    // be a finite integer in the page and would install no edit.
    let digits = position.strip_prefix('-').unwrap_or(position);
    chain.split('.').all(identifier)
        && !digits.is_empty()
        && digits.len() <= 4
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && object_query::accepts(query)
}

/// A rule's arguments as adblock-rust hands them to the page: split at
/// commas no backslash escapes, trimmed, with `\,` read as a comma and every
/// other backslash kept. A quoted argument is not read here; a quoted
/// scriptlet name is unquoted, as the library reads it.
fn page_arguments(body: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in body.chars() {
        if escaped {
            if character != ',' {
                current.push('\\');
            }
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ',' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    parts.push(current);
    parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            let part = part.trim();
            if index == 0 {
                return Some(part.trim_matches(['"', '\'', '`']).to_owned());
            }
            (!part.starts_with(['"', '\'', '`'])).then(|| part.to_owned())
        })
        .collect()
}

/// A rule's comma-separated arguments, trimmed and unquoted; `\,` is a comma
/// inside one.
fn arguments(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut escaped) = (0, false);
    for (index, character) in body.char_indices() {
        match character {
            '\\' => escaped = !escaped,
            ',' if !escaped => {
                parts.push(&body[start..index]);
                start = index + 1;
            }
            _ => escaped = false,
        }
    }
    parts.push(&body[start..]);
    parts
        .into_iter()
        .map(|part| part.trim().trim_matches(|c| matches!(c, '"' | '\'' | '`')))
        .collect()
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
    let mut lists: BTreeMap<ScriptletTrust, Vec<&str>> = BTreeMap::new();
    for rule in rules {
        // A stored policy is admitted again, so it cannot run what compiling
        // it would have left out.
        if !matches!(classify(&rule.line), Kind::Rule { .. }) {
            return Err(crate::CosmeticError::InvalidPolicy);
        }
        let Some((_, domains, _)) = split(&rule.line) else {
            return Err(crate::CosmeticError::InvalidPolicy);
        };
        keys.extend(domain_keys(domains).ok_or(crate::CosmeticError::InvalidPolicy)?);
        lists
            .entry(rule.trust)
            .or_default()
            .push(rule.line.as_str());
    }
    if keys.is_empty() {
        return Ok(None);
    }
    let mut filters = FilterSet::new(false);
    for (trust, lines) in lists {
        filters.add_filter_list(
            lines.join("\n"),
            ParseOptions {
                rule_types: RuleTypes::CosmeticOnly,
                permissions: trust.permissions(),
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
    let library: Vec<String> = library()
        .map(|(scriptlet, _)| function_text(scriptlet.source))
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
            trust: if trusted {
                ScriptletTrust::Trusted
            } else {
                ScriptletTrust::Plain
            },
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
    fn a_backtick_quoted_name_is_read_as_the_library_reads_it() {
        assert!(matches!(
            classify("example.com##+js(`trusted-rpfr`, a, b)"),
            Kind::Unsupported
        ));
        assert!(matches!(
            classify("www.youtube.com##+js(`trusted-rpfr`, a, b)"),
            Kind::Rule { unknown: false, .. }
        ));
        assert!(matches!(
            build(&[rule("example.com##+js(`trusted-rpfr`, a, b)", true)]),
            Err(crate::CosmeticError::InvalidPolicy)
        ));
    }

    #[test]
    fn a_backtick_quoted_object_edit_is_read_as_the_library_reads_it() {
        let edit = r#"##+js(`trusted-edit-inbound-object`, JSON.stringify, 0, [?.attestationRequest][?.context.client.userAgent*="lactmilli"]+={"params":"8AUB"})"#;
        assert!(matches!(
            classify(&format!("example.com{edit}")),
            Kind::Unsupported
        ));
        assert!(matches!(
            classify(&format!("www.youtube.com{edit}")),
            Kind::Rule { unknown: false, .. }
        ));
        assert!(matches!(
            classify(
                r#"www.youtube.com##+js(`trusted-edit-inbound-object`, JSON.stringify, 0, `[?.a]+={"b":1}`)"#
            ),
            Kind::Unsupported
        ));
        assert!(matches!(
            build(&[rule(&format!("example.com{edit}"), true)]),
            Err(crate::CosmeticError::InvalidPolicy)
        ));

        // A quoted edit keys and strips markers as the plain one does.
        let mut rules: Vec<ScriptletRule> = YOUTUBE_EDITS
            .iter()
            .map(|line| {
                rule(
                    &line.replacen(
                        "trusted-edit-inbound-object",
                        "`trusted-edit-inbound-object`",
                        1,
                    ),
                    true,
                )
            })
            .collect();
        rules.push(marker_rule(ScriptletTrust::Zephium));
        assert_eq!(keep_edited_markers(&mut rules), 0);
    }

    #[test]
    fn unknown_and_unsupported_rules_are_told_apart() {
        assert!(matches!(
            classify("a.com##+js(set, a, 1)"),
            Kind::Rule { unknown: false, .. }
        ));
        assert!(matches!(
            classify("a.com##+js(no-such-scriptlet, a)"),
            Kind::Rule { unknown: true, .. }
        ));
        assert!(matches!(
            classify("a.com#@#+js()"),
            Kind::Rule { unknown: false, .. }
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
    fn the_first_marker_decides_and_a_quoted_one_is_text() {
        // A quoted exception marker does not lift the YouTube host gate.
        assert!(matches!(
            classify(r##"example.com##+js(trusted-rpfr, x, "#@#+js(")"##),
            Kind::Unsupported
        ));
        // An exception that quotes the other marker is still an exception.
        assert!(matches!(
            classify(r###"a.com#@#+js(trusted-rpfr, x, "##+js(")"###),
            Kind::Rule {
                unknown: false,
                substituted: false
            }
        ));
    }

    #[test]
    fn every_rule_admission_keeps_also_builds() {
        let odd = [
            r###"a.com#@#+js(trusted-rpfr, x, "##+js(")"###,
            r##"www.youtube.com##+js(trusted-rpfr, x, "#@#+js(")"##,
            r##"a.com#@#+js(set, "a.com##+js(", 1)"##,
            "a.com#@#+js(set, b, '##+js(x)')",
            "a.com##+js(set, b\\, c, 1)",
        ];
        for line in odd {
            assert!(matches!(classify(line), Kind::Rule { .. }), "{line}");
            for trusted in [false, true] {
                assert!(build(&[rule(line, trusted)]).is_ok(), "{line}");
            }
        }
        // Through the cosmetic compile, beside an ordinary rule.
        let source = format!("{}\na.com##+js(set, ok, 1)", odd.join("\n"));
        let policy =
            crate::CosmeticPolicy::compile_with_trust([(source.as_str(), ScriptletTrust::Trusted)])
                .unwrap();
        assert!(policy.scriptlet_program().is_some());
    }

    #[test]
    fn a_resource_no_xhr_if_answers_with_an_empty_body_is_a_loss() {
        for (line, substituted) in [
            ("a.com##+js(no-xhr-if, ads.js, war:noop-vast3.xml)", true),
            (
                "a.com##+js(prevent-xhr.js, 'ads.js', 'war:noop.json')",
                true,
            ),
            ("a.com##+js(no-xhr-if, ads.js, emptyObj)", false),
            ("a.com##+js(no-xhr-if, a\\,war:b, true)", false),
            ("a.com#@#+js(no-xhr-if, ads.js, war:noop-vast3.xml)", false),
        ] {
            assert!(
                matches!(classify(line), Kind::Rule { unknown: false, substituted: s } if s == substituted),
                "{line}"
            );
        }
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
                matches!(classify(line), Kind::Rule { unknown: false, .. }),
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

    /// uBlock Origin's quick fixes edit YouTube's player requests with these
    /// rules, verbatim as of 2026-11-06.
    const YOUTUBE_EDITS: [&str; 7] = [
        r##"www.youtube.com##+js(trusted-edit-inbound-object, JSON.stringify, 0, [?.attestationRequest][?.context.client.userAgent*="channel"].context.client[?.clientName=="WEB"]+={"clientScreen":"CHANNEL"})"##,
        r##"www.youtube.com##+js(trusted-edit-inbound-object, JSON.stringify, 0, [?.attestationRequest][?.context.client.userAgent*="lactmilli"]+={"params":"8AUB"})"##,
        r##"www.youtube.com##+js(trusted-edit-inbound-object, JSON.stringify, 0, [?.attestationRequest][?.context.client.userAgent*="yahi"]+={"params":"YAHI"})"##,
        r##"www.youtube.com##+js(trusted-edit-inbound-object, JSON.stringify, 0, [?.attestationRequest][?.context.client.userAgent*="inline"].playbackContext.contentPlaybackContext+={"isInlinePlaybackNoAd":true})"##,
        r##"www.youtube.com##+js(trusted-edit-inbound-object, JSON.stringify, 0, [?.attestationRequest][?.context.client.userAgent=/channel|lactmilli|inline/].playbackContext.contentPlaybackContext.lactMilliseconds="${now}")"##,
        r##"www.youtube.com##+js(trusted-edit-inbound-object, JSON.stringify, 0, [?.attestationRequest][?.context.client.userAgent=/adunit|channel|lactmilli|inline|yahi|eafg|yaf4/].playbackContext.contentPlaybackContext.referer=repl({"regex":"(?:#reloadxhr)?$"\,"replacement":"#reloadxhr"}))"##,
        r##"www.youtube.com##+js(trusted-edit-inbound-object, JSON.stringify, 0, [?.createCommentParams].context.client.userAgent=repl({"regex":"/; (adunit|channel|lactmilli|inline|yahi|eafg|yaf4)\b/"\,"replacement":""}))"##,
    ];

    #[test]
    fn youtube_s_object_edits_reach_the_page_as_its_reader_reads_them() {
        let queries: serde_json::Value = serde_json::from_str(include_str!(
            "../../../scripts/blocker-scriptlets/edit-queries.json"
        ))
        .unwrap();
        for (line, query) in YOUTUBE_EDITS
            .iter()
            .zip(queries["accepted"].as_array().unwrap())
        {
            assert!(
                matches!(classify(line), Kind::Rule { unknown: false, .. }),
                "{line}"
            );
            assert!(build(&[rule(line, false)]).unwrap().is_none(), "{line}");
            let program = build(&[rule(line, true)]).unwrap().unwrap();
            let call = format!(
                "zephiumTrustedEditInboundObject(\"JSON.stringify\", \"0\", {})",
                serde_json::to_string(query).unwrap()
            );
            assert!(program.script.contains(&call), "{line}");
        }
    }

    const MARKER: &str = r"www.youtube.com##+js(zephium-player-marker, channel lactmilli yahi inline, buffer_health_seconds=0.00 s|resolution=0x0, debug_info^=SSAP\, AD, 1000, a.b=c, support.google.com/x, a.b*=t)";

    fn marker_rule(trust: ScriptletTrust) -> ScriptletRule {
        ScriptletRule {
            line: MARKER.to_owned(),
            trust,
        }
    }

    #[test]
    fn only_zephium_s_own_rules_call_its_scriptlets_and_never_trusted_ones() {
        let marked = |rules: &[ScriptletRule]| {
            build(rules)
                .unwrap()
                .is_some_and(|program| program.script.contains("zephiumPlayerMarker(\"channel"))
        };
        assert!(marked(&[marker_rule(ScriptletTrust::Zephium)]));
        assert!(!marked(&[marker_rule(ScriptletTrust::Trusted)]));
        assert!(!marked(&[marker_rule(ScriptletTrust::Plain)]));
        let edit = ScriptletRule {
            line: YOUTUBE_EDITS[1].to_owned(),
            trust: ScriptletTrust::Zephium,
        };
        assert!(build(&[edit]).unwrap().is_none());
        assert!(matches!(
            classify(&MARKER.replacen("www.youtube.com", "example.com", 1)),
            Kind::Unsupported
        ));
    }

    #[test]
    fn a_marker_no_admitted_edit_both_keys_on_and_strips_is_never_sent() {
        let trusted = |line: &str| ScriptletRule {
            line: line.to_owned(),
            trust: ScriptletTrust::Trusted,
        };
        let markers = |rules: &[ScriptletRule]| -> Option<String> {
            let marker = rules
                .iter()
                .find(|rule| rule.trust == ScriptletTrust::Zephium)?;
            rule_arguments(&marker.line, PLAYER_MARKER)?
                .into_iter()
                .nth(1)
        };
        let mut rules: Vec<ScriptletRule> =
            YOUTUBE_EDITS.iter().map(|line| trusted(line)).collect();
        rules.push(marker_rule(ScriptletTrust::Zephium));
        assert_eq!(keep_edited_markers(&mut rules), 0);
        assert_eq!(
            markers(&rules).as_deref(),
            Some("channel lactmilli yahi inline")
        );
        assert_eq!(rules.last().unwrap().line, MARKER);

        // uBO stops keying on yahi: it is dropped, and the rule still runs.
        let mut rules: Vec<ScriptletRule> = YOUTUBE_EDITS
            .iter()
            .filter(|line| !line.contains(r#"*="yahi""#))
            .map(|line| trusted(&line.replace("|yahi", "")))
            .collect();
        rules.push(marker_rule(ScriptletTrust::Zephium));
        assert_eq!(keep_edited_markers(&mut rules), 1);
        assert_eq!(markers(&rules).as_deref(), Some("channel lactmilli inline"));
        let program = build(&rules).unwrap().unwrap();
        assert!(program.script.contains(
            r#"zephiumPlayerMarker("channel lactmilli inline", "buffer_health_seconds=0.00 s|resolution=0x0", "debug_info^=SSAP, AD", "1000""#
        ));

        // Edits with no strip, or none at all, leave nothing to mark with.
        let mut rules: Vec<ScriptletRule> = YOUTUBE_EDITS[..6]
            .iter()
            .map(|line| trusted(line))
            .collect();
        rules.push(marker_rule(ScriptletTrust::Zephium));
        assert_eq!(keep_edited_markers(&mut rules), 4);
        assert_eq!(markers(&rules), None);
        let mut alone = vec![marker_rule(ScriptletTrust::Zephium)];
        assert_eq!(keep_edited_markers(&mut alone), 4);
        assert!(alone.is_empty());

        // A renamed marker is not read as the old one it contains.
        let mut rules: Vec<ScriptletRule> = YOUTUBE_EDITS
            .iter()
            .map(|line| trusted(&line.replace("lactmilli", "lactmilli-v2")))
            .collect();
        rules.push(marker_rule(ScriptletTrust::Zephium));
        assert_eq!(keep_edited_markers(&mut rules), 1);
        assert_eq!(markers(&rules).as_deref(), Some("channel yahi inline"));

        // Edits that do not run on the marker's host leave nothing to mark with.
        for hosts in ["m.youtube.com", "youtube.com,~www.youtube.com"] {
            let mut rules: Vec<ScriptletRule> = YOUTUBE_EDITS
                .iter()
                .map(|line| trusted(&line.replacen("www.youtube.com", hosts, 1)))
                .collect();
            rules.push(marker_rule(ScriptletTrust::Zephium));
            assert_eq!(keep_edited_markers(&mut rules), 4, "{hosts}");
        }
        // Edits for a parent domain run on it.
        let mut rules: Vec<ScriptletRule> = YOUTUBE_EDITS
            .iter()
            .map(|line| trusted(&line.replacen("www.youtube.com", "youtube.com", 1)))
            .collect();
        rules.push(marker_rule(ScriptletTrust::Zephium));
        assert_eq!(keep_edited_markers(&mut rules), 0);
        // An exception that turns the edits, or every scriptlet, off there
        // leaves none, from whichever list.
        for exception in [
            "www.youtube.com#@#+js(trusted-edit-inbound-object)",
            "youtube.com#@#+js()",
            "#@#+js(trusted-edit-inbound-object.js)",
        ] {
            let mut rules: Vec<ScriptletRule> =
                YOUTUBE_EDITS.iter().map(|line| trusted(line)).collect();
            rules.push(rule(exception, false));
            rules.push(marker_rule(ScriptletTrust::Zephium));
            assert_eq!(keep_edited_markers(&mut rules), 4, "{exception}");
        }
        let mut rules: Vec<ScriptletRule> =
            YOUTUBE_EDITS.iter().map(|line| trusted(line)).collect();
        rules.push(rule(
            "m.youtube.com#@#+js(trusted-edit-inbound-object)",
            false,
        ));
        rules.push(marker_rule(ScriptletTrust::Zephium));
        assert_eq!(keep_edited_markers(&mut rules), 0);

        // Only uBO's own lists count: the same edits from elsewhere do not.
        let mut untrusted: Vec<ScriptletRule> = YOUTUBE_EDITS
            .iter()
            .map(|line| ScriptletRule {
                line: (*line).to_owned(),
                trust: ScriptletTrust::Plain,
            })
            .collect();
        untrusted.push(marker_rule(ScriptletTrust::Zephium));
        assert_eq!(keep_edited_markers(&mut untrusted), 4);
    }

    #[test]
    fn an_object_edit_the_page_would_not_read_is_not_admitted() {
        let edit = |hosts: &str, arguments: &str| {
            format!("{hosts}##+js(trusted-edit-inbound-object, {arguments})")
        };
        for line in [
            edit("example.com", r#"JSON.stringify, 0, [?.a]+={"b":1}"#),
            edit("youtube.*", r#"JSON.stringify, 0, [?.a]+={"b":1}"#),
            edit("www.youtube.com", r#"JSON.stringify, 0, [?.a]"#),
            edit(
                "www.youtube.com",
                r#"JSON.stringify, 0, [?.a]+={"b":1,"c":2}"#,
            ),
            edit("www.youtube.com", r#"JSON.stringify, 0, '[?.a]+={"b":1}'"#),
            edit("www.youtube.com", r#"JSON.stringify, x, [?.a]+={"b":1}"#),
            edit("www.youtube.com", r#"JSON.stringify, , [?.a]+={"b":1}"#),
            edit(
                "www.youtube.com",
                r#"JSON.stringify, 10000, [?.a]+={"b":1}"#,
            ),
            edit("www.youtube.com", r#"JSON..stringify, 0, [?.a]+={"b":1}"#),
            edit(
                "www.youtube.com",
                r#"JSON.stringify, 0, [?.a]+={"b":1}, extra"#,
            ),
            edit(
                "www.youtube.com",
                r#"JSON.stringify, 0, v2:$.a=call([null\,"alert"])"#,
            ),
        ] {
            assert!(matches!(classify(&line), Kind::Unsupported), "{line}");
        }
        assert!(matches!(
            classify(&edit(
                "m.youtube.com,www.youtube-nocookie.com",
                r#"JSON.parse, -1, v2:$..adSlots"#
            )),
            Kind::Rule { unknown: false, .. }
        ));
        assert!(matches!(
            classify(r#"example.com#@#+js(trusted-edit-inbound-object, JSON.stringify, 0, x)"#),
            Kind::Rule { unknown: false, .. }
        ));
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
