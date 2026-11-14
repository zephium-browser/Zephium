use std::collections::{BTreeMap, HashMap};
use std::fmt;

use aho_corasick::AhoCorasick;
use regex::bytes::{RegexSet, RegexSetBuilder};

use crate::matcher::{evaluate_candidates, Candidate, Decision, PreparedRule, Request};
use crate::model::{Rule, RulesetKind};
use crate::wire::{Reader, Wire};

pub const INDEX_FORMAT_VERSION: u32 = 1;
pub const MAX_INDEX_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_INDEX_RULES: usize = 1_000_000;
const MAGIC: &[u8; 8] = b"ZEPHDNR\0";
const HEADER: usize = 48;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexError(pub String);
impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for IndexError {}
impl From<String> for IndexError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Validated immutable rules and acceleration tables. No platform or profile state.
pub struct CompiledRuleset {
    ruleset_id: String,
    kind: RulesetKind,
    rules: Vec<PreparedRule>,
    domains: HashMap<String, Vec<usize>>,
    initiators: HashMap<String, Vec<usize>>,
    tops: HashMap<String, Vec<usize>>,
    tokens: AhoCorasick,
    token_ids: Vec<Vec<usize>>,
    fallback: Vec<usize>,
    regex_ids: Vec<usize>,
    regex_set: RegexSet,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
    Domain(String),
    Initiator(String),
    Top(String),
    Token(String),
    Regex,
    Fallback,
}

impl CompiledRuleset {
    pub fn compile(
        ruleset_id: &str,
        kind: RulesetKind,
        rules: Vec<Rule>,
    ) -> Result<Self, IndexError> {
        if ruleset_id.is_empty() || ruleset_id.len() > 1024 || rules.len() > MAX_INDEX_RULES {
            return Err(IndexError("invalid ruleset identity or rule count".into()));
        }
        let rejected = crate::parse::validate_rules(&rules, kind);
        if !rejected.is_empty() {
            return Err(IndexError(format!("{rejected:?}")));
        }
        let mut encoded_bound = HEADER + 4 + ruleset_id.len() + 1 + rules.len() * 8 + 4;
        for rule in &rules {
            let mut encoded = Vec::new();
            rule.write(&mut encoded);
            encoded_bound = encoded_bound
                .checked_add(encoded.len())
                .ok_or_else(|| IndexError("index size overflow".into()))?;
            if encoded_bound > MAX_INDEX_BYTES {
                return Err(IndexError("index exceeds byte limit".into()));
            }
        }
        let buckets = bucket_rules(&rules);
        for (bucket, ids) in &buckets {
            let key_bytes = match bucket {
                Bucket::Domain(key)
                | Bucket::Initiator(key)
                | Bucket::Top(key)
                | Bucket::Token(key) => 4 + key.len(),
                Bucket::Regex | Bucket::Fallback => 0,
            };
            encoded_bound = encoded_bound
                .checked_add(1 + key_bytes + 4 + ids.len() * 4)
                .ok_or_else(|| IndexError("index size overflow".into()))?;
            if encoded_bound > MAX_INDEX_BYTES {
                return Err(IndexError("index exceeds byte limit".into()));
            }
        }
        let prepared = rules
            .into_iter()
            .map(PreparedRule::new)
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_parts(ruleset_id.to_owned(), kind, prepared, buckets)
    }

    fn from_parts(
        ruleset_id: String,
        kind: RulesetKind,
        rules: Vec<PreparedRule>,
        buckets: BTreeMap<Bucket, Vec<usize>>,
    ) -> Result<Self, IndexError> {
        let mut domains = HashMap::new();
        let mut initiators = HashMap::new();
        let mut tops = HashMap::new();
        let mut tokens = Vec::new();
        let mut token_ids: Vec<Vec<usize>> = Vec::new();
        let mut fallback = Vec::new();
        let mut regex_ids = Vec::new();
        for (bucket, ids) in buckets {
            match bucket {
                Bucket::Domain(domain) => {
                    domains.insert(domain, ids);
                }
                Bucket::Initiator(domain) => {
                    initiators.insert(domain, ids);
                }
                Bucket::Top(domain) => {
                    tops.insert(domain, ids);
                }
                Bucket::Token(token) => {
                    let group = tokens.len() % 4096;
                    if group == token_ids.len() {
                        token_ids.push(Vec::new());
                    }
                    token_ids[group].extend(ids);
                    tokens.push(token);
                }
                Bucket::Fallback => fallback = ids,
                Bucket::Regex => regex_ids = ids,
            }
        }
        let tokens = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(tokens)
            .map_err(|error| IndexError(error.to_string()))?;
        let patterns: Vec<_> = regex_ids
            .iter()
            .map(|&id| {
                let condition = &rules[id].rule.condition;
                let pattern =
                    crate::parse::normalize_regex(condition.regex_filter.as_deref().unwrap_or(""))?;
                Ok(if condition.is_url_filter_case_sensitive {
                    pattern
                } else {
                    format!("(?i:{pattern})")
                })
            })
            .collect::<Result<Vec<String>, IndexError>>()?;
        let regex_set = RegexSetBuilder::new(patterns)
            .unicode(false)
            .size_limit(16 * 1024 * 1024)
            .build()
            .map_err(|e| IndexError(e.to_string()))?;
        Ok(Self {
            ruleset_id,
            kind,
            rules,
            domains,
            initiators,
            tops,
            tokens,
            token_ids,
            fallback,
            regex_ids,
            regex_set,
        })
    }

    pub fn ruleset_id(&self) -> &str {
        &self.ruleset_id
    }
    pub fn kind(&self) -> RulesetKind {
        self.kind
    }
    pub fn rules(&self) -> &[PreparedRule] {
        &self.rules
    }

    /// A streaming prefilter; the matcher checks every candidate's full condition.
    pub fn candidates<'index: 'request, 'request>(
        &'index self,
        request: &'request Request<'_>,
    ) -> impl Iterator<Item = &'index PreparedRule> + 'request {
        let domain_ids = domain_candidates(&self.domains, request.host);
        let initiator_ids =
            domain_candidates(&self.initiators, request.initiator_host.unwrap_or(""));
        let top_host = if request.resource_type == crate::model::ResourceType::MainFrame {
            Some(request.host)
        } else {
            request.top_host.or(request.initiator_host)
        };
        let top_ids = domain_candidates(&self.tops, top_host.unwrap_or(""));
        let url = request.url.as_bytes();
        let mut seen = [0u64; 64];
        let token_ids = self
            .tokens
            .find_overlapping_iter(url)
            .filter_map(move |matched| {
                let group = matched.pattern().as_usize() % 4096;
                let word = group / 64;
                let bit = 1u64 << (group % 64);
                if seen[word] & bit != 0 {
                    return None;
                }
                seen[word] |= bit;
                Some(&self.token_ids[group])
            })
            .flat_map(|ids| ids.iter().copied());
        // RegexSet::matches allocates a bitset. Avoid it entirely for the common miss.
        let regex_matches = self
            .regex_set
            .is_match(url)
            .then(|| self.regex_set.matches(url));
        let regex_ids = regex_matches
            .into_iter()
            .flat_map(|matches| matches.into_iter())
            .map(|id| self.regex_ids[id]);
        domain_ids
            .chain(initiator_ids)
            .chain(top_ids)
            .chain(token_ids)
            .chain(self.fallback.iter().copied())
            .chain(regex_ids)
            .map(|id| &self.rules[id])
    }

    /// The checksum detects corruption, not malicious replacement. Authenticate the
    /// file and seal its backing memory at the owning process boundary.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0; HEADER];
        self.ruleset_id.write(&mut bytes);
        self.kind.write(&mut bytes);
        let directory = bytes.len();
        bytes.resize(directory + self.rules.len() * 8, 0);
        let records = bytes.len();
        for (i, rule) in self.rules.iter().enumerate() {
            let start = bytes.len();
            rule.rule.write(&mut bytes);
            let end = bytes.len();
            put_u32(&mut bytes, directory + i * 8, start as u32);
            put_u32(&mut bytes, directory + i * 8 + 4, (end - start) as u32);
        }
        let postings = bytes.len();
        let models: Vec<_> = self.rules.iter().map(|r| r.rule.clone()).collect();
        let buckets = bucket_rules(&models);
        (buckets.len() as u32).write(&mut bytes);
        for (bucket, ids) in buckets {
            match bucket {
                Bucket::Domain(domain) => {
                    0u8.write(&mut bytes);
                    domain.write(&mut bytes);
                }
                Bucket::Initiator(domain) => {
                    4u8.write(&mut bytes);
                    domain.write(&mut bytes);
                }
                Bucket::Top(domain) => {
                    5u8.write(&mut bytes);
                    domain.write(&mut bytes);
                }
                Bucket::Token(token) => {
                    1u8.write(&mut bytes);
                    token.write(&mut bytes);
                }
                Bucket::Regex => 2u8.write(&mut bytes),
                Bucket::Fallback => 3u8.write(&mut bytes),
            }
            let ids: Vec<u32> = ids.into_iter().map(|i| i as u32).collect();
            ids.write(&mut bytes);
        }
        let total = bytes.len();
        bytes[..8].copy_from_slice(MAGIC);
        for (offset, value) in [
            (8, INDEX_FORMAT_VERSION),
            (12, total as u32),
            (16, self.rules.len() as u32),
            (20, HEADER as u32),
            (24, directory as u32),
            (28, records as u32),
            (32, postings as u32),
        ] {
            put_u32(&mut bytes, offset, value);
        }
        let checksum = checksum(&bytes[HEADER..]);
        bytes[40..48].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    pub fn load(bytes: &[u8]) -> Result<Self, IndexError> {
        let bad = || IndexError("invalid or corrupt DNR index".into());
        if bytes.len() < HEADER
            || bytes.len() > MAX_INDEX_BYTES
            || bytes.get(..8) != Some(MAGIC.as_slice())
        {
            return Err(bad());
        }
        let mut head = Reader::new(&bytes[8..]);
        let version = u32::read(&mut head)?;
        let total = u32::read(&mut head)? as usize;
        let count = u32::read(&mut head)? as usize;
        let metadata = u32::read(&mut head)? as usize;
        let directory = u32::read(&mut head)? as usize;
        let records = u32::read(&mut head)? as usize;
        let postings = u32::read(&mut head)? as usize;
        let reserved = u32::read(&mut head)?;
        let seal = u64::read(&mut head)?;
        if version != INDEX_FORMAT_VERSION
            || total != bytes.len()
            || count > MAX_INDEX_RULES
            || metadata != HEADER
            || directory < metadata
            || records
                != directory
                    .checked_add(count.checked_mul(8).ok_or_else(bad)?)
                    .ok_or_else(bad)?
            || postings < records
            || postings > total
            || reserved != 0
            || checksum(&bytes[HEADER..]) != seal
        {
            return Err(bad());
        }
        let mut meta = Reader::new(bytes.get(metadata..directory).ok_or_else(bad)?);
        let ruleset_id = String::read(&mut meta)?;
        let kind = RulesetKind::read(&mut meta)?;
        meta.finish()?;
        if ruleset_id.is_empty() || ruleset_id.len() > 1024 {
            return Err(bad());
        }
        let mut table = Reader::new(bytes.get(directory..records).ok_or_else(bad)?);
        let mut next = records;
        let mut models = Vec::with_capacity(count);
        for _ in 0..count {
            let start = u32::read(&mut table)? as usize;
            let length = u32::read(&mut table)? as usize;
            let end = start.checked_add(length).ok_or_else(bad)?;
            if start != next || end > postings {
                return Err(bad());
            }
            let mut record = Reader::new(bytes.get(start..end).ok_or_else(bad)?);
            models.push(Rule::read(&mut record)?);
            record.finish()?;
            next = end;
        }
        if next != postings {
            return Err(bad());
        }
        let rejected = crate::parse::validate_rules(&models, kind);
        if !rejected.is_empty() {
            return Err(IndexError(format!("{rejected:?}")));
        }
        let mut reader = Reader::new(&bytes[postings..]);
        let buckets_count = u32::read(&mut reader)? as usize;
        if buckets_count > bytes.len() / 5 {
            return Err(bad());
        }
        let mut buckets = BTreeMap::new();
        for _ in 0..buckets_count {
            let bucket = match u8::read(&mut reader)? {
                0 => Bucket::Domain(String::read(&mut reader)?),
                1 => Bucket::Token(String::read(&mut reader)?),
                2 => Bucket::Regex,
                3 => Bucket::Fallback,
                4 => Bucket::Initiator(String::read(&mut reader)?),
                5 => Bucket::Top(String::read(&mut reader)?),
                _ => return Err(bad()),
            };
            let ids = Vec::<u32>::read(&mut reader)?;
            if ids.iter().any(|&id| id as usize >= count) {
                return Err(bad());
            }
            if buckets
                .insert(bucket, ids.into_iter().map(|id| id as usize).collect())
                .is_some()
            {
                return Err(bad());
            }
        }
        reader.finish()?;
        if buckets != bucket_rules(&models) {
            return Err(bad());
        }
        let rules = models
            .into_iter()
            .map(PreparedRule::new)
            .collect::<Result<_, _>>()?;
        Self::from_parts(ruleset_id, kind, rules, buckets)
    }
}

fn domain_candidates<'a>(
    domains: &'a HashMap<String, Vec<usize>>,
    host: &'a str,
) -> impl Iterator<Item = usize> + 'a {
    std::iter::successors(Some(host), |s| s.find('.').map(|pos| &s[pos + 1..]))
        .filter_map(|domain| {
            domains.get(domain).or_else(|| {
                domain
                    .bytes()
                    .any(|byte| byte.is_ascii_uppercase())
                    .then(|| domains.get(&domain.to_ascii_lowercase()))
                    .flatten()
            })
        })
        .flat_map(|ids| ids.iter().copied())
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
fn mandatory_tokens(filter: &str) -> impl Iterator<Item = &str> {
    filter
        .split(['*', '^', '|'])
        .filter(|part| part.len() >= 3)
        .map(|part| &part[..part.len().min(32)])
}

fn domain_key(rule: &Rule) -> Option<String> {
    let filter = rule.condition.url_filter.as_deref()?.strip_prefix("||")?;
    let end = filter.find(['^', '/', ':', '|'])?;
    let host = &filter[..end];
    if host.is_empty()
        || host.contains('*')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn bucket_rules(rules: &[Rule]) -> BTreeMap<Bucket, Vec<usize>> {
    let mut frequency = HashMap::<String, usize>::new();
    for rule in rules {
        if let Some(filter) = &rule.condition.url_filter {
            for key in mandatory_tokens(filter) {
                *frequency.entry(key.to_ascii_lowercase()).or_default() += 1;
            }
        }
    }
    let mut buckets: BTreeMap<Bucket, Vec<usize>> = BTreeMap::new();
    for (id, rule) in rules.iter().enumerate() {
        let condition = &rule.condition;
        let domain_scope = condition
            .request_domains
            .as_ref()
            .map(|domains| (0, domains))
            .or_else(|| {
                condition
                    .initiator_domains
                    .as_ref()
                    .or(condition.domains.as_ref())
                    .map(|domains| (1, domains))
            })
            .or_else(|| condition.top_domains.as_ref().map(|domains| (2, domains)));
        if let Some((scope, domains)) = domain_scope {
            let domains: std::collections::HashSet<_> = domains
                .iter()
                .map(|domain| domain.to_ascii_lowercase())
                .collect();
            for domain in &domains {
                let redundant = std::iter::successors(
                    domain.split_once('.').map(|(_, suffix)| suffix),
                    |suffix| suffix.split_once('.').map(|(_, suffix)| suffix),
                )
                .any(|suffix| domains.contains(suffix));
                if !redundant {
                    let key = match scope {
                        0 => Bucket::Domain(domain.clone()),
                        1 => Bucket::Initiator(domain.clone()),
                        _ => Bucket::Top(domain.clone()),
                    };
                    buckets.entry(key).or_default().push(id);
                }
            }
            continue;
        }
        let bucket = if rule.condition.regex_filter.is_some() {
            Bucket::Regex
        } else if let Some(domain) = domain_key(rule) {
            Bucket::Domain(domain)
        } else if let Some(key) = rule.condition.url_filter.as_deref().and_then(|filter| {
            mandatory_tokens(filter).min_by_key(|key| {
                (
                    frequency[&key.to_ascii_lowercase()],
                    std::cmp::Reverse(key.len()),
                    *key,
                )
            })
        }) {
            Bucket::Token(key.to_ascii_lowercase())
        } else {
            Bucket::Fallback
        };
        buckets.entry(bucket).or_default().push(id);
    }
    buckets
}

/// Supply extensions in any order; install_order increases with installation time.
pub struct Extension<'a> {
    pub id: &'a str,
    pub install_order: u64,
    pub rulesets: &'a [&'a CompiledRuleset],
}

pub fn evaluate<'a>(request: &Request<'_>, extensions: &'a [Extension<'a>]) -> Decision<'a> {
    evaluate_candidates(
        request,
        extensions.iter().flat_map(|extension| {
            extension.rulesets.iter().flat_map(move |ruleset| {
                ruleset.candidates(request).map(move |rule| Candidate {
                    extension_id: extension.id,
                    install_order: extension.install_order,
                    ruleset_id: ruleset.ruleset_id(),
                    rule,
                })
            })
        }),
    )
}

/// Linear reference path used to check acceleration tables against every rule.
pub fn evaluate_linear<'a>(request: &Request<'_>, extensions: &'a [Extension<'a>]) -> Decision<'a> {
    evaluate_candidates(
        request,
        extensions.iter().flat_map(|extension| {
            extension.rulesets.iter().flat_map(move |ruleset| {
                ruleset.rules().iter().map(move |rule| Candidate {
                    extension_id: extension.id,
                    install_order: extension.install_order,
                    ruleset_id: ruleset.ruleset_id(),
                    rule,
                })
            })
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Condition, ResourceType};

    fn fixture() -> CompiledRuleset {
        let filters = [
            "||example.test^",
            "track*pixel",
            "x",
            "|https://start.test/",
            "end|",
        ];
        let rules = filters
            .into_iter()
            .enumerate()
            .map(|(id, filter)| Rule {
                id: id as u32 + 1,
                priority: 1,
                action: Action::Block,
                condition: Condition {
                    url_filter: Some(filter.into()),
                    ..Condition::default()
                },
            })
            .collect();
        CompiledRuleset::compile("fixture", RulesetKind::Static, rules).unwrap()
    }

    #[test]
    fn deterministic_roundtrip_and_every_truncation() {
        let index = fixture();
        let bytes = index.to_bytes();
        let loaded = CompiledRuleset::load(&bytes).unwrap();
        assert_eq!(bytes, loaded.to_bytes());
        for end in 0..bytes.len() {
            assert!(CompiledRuleset::load(&bytes[..end]).is_err(), "{end}");
        }
        for url in [
            "https://example.test/a",
            "https://sub.example.test/a",
            "https://no.test/track-pixel",
            "https://no.test/end",
        ] {
            let host = url.split('/').nth(2).unwrap();
            let request = Request::new(url, host, ResourceType::Script);
            let rulesets = [&loaded];
            let extensions = [Extension {
                id: "ext",
                install_order: 1,
                rulesets: &rulesets,
            }];
            assert_eq!(
                evaluate(&request, &extensions),
                evaluate_linear(&request, &extensions)
            );
        }
    }

    fn reseal(bytes: &mut [u8]) {
        let seal = checksum(&bytes[HEADER..]);
        bytes[40..48].copy_from_slice(&seal.to_le_bytes());
    }

    #[test]
    fn hostile_offsets_lengths_tags_and_postings_are_rejected() {
        let original = fixture().to_bytes();
        for position in [8, 12, 16, 20, 24, 28, 32, 36] {
            let mut changed = original.clone();
            put_u32(&mut changed, position, u32::MAX);
            assert!(CompiledRuleset::load(&changed).is_err());
        }
        let directory = u32::from_le_bytes(original[24..28].try_into().unwrap()) as usize;
        let records = u32::from_le_bytes(original[28..32].try_into().unwrap()) as usize;
        let postings = u32::from_le_bytes(original[32..36].try_into().unwrap()) as usize;
        for position in [HEADER, directory, directory + 4, records, postings] {
            let mut changed = original.clone();
            put_u32(&mut changed, position, u32::MAX);
            reseal(&mut changed);
            assert!(CompiledRuleset::load(&changed).is_err(), "{position}");
        }
        let mut changed = original.clone();
        changed[records + 8] = 255;
        reseal(&mut changed);
        assert!(CompiledRuleset::load(&changed).is_err());
        let mut changed = original;
        *changed.last_mut().unwrap() = 255;
        reseal(&mut changed);
        assert!(CompiledRuleset::load(&changed).is_err());
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        let mut seed = 0xfedc_ba98_7654_3210u64;
        for length in 0..1024 {
            let bytes: Vec<_> = (0..length)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed as u8
                })
                .collect();
            assert!(CompiledRuleset::load(&bytes).is_err());
        }
    }
}

/// Check aggregate limits before publishing an extension's enabled indexes.
/// The owner supplies its static allocation; Chrome's 30,000 is a guarantee,
/// not a ceiling. Disabled declared rulesets are checked by the manifest owner.
pub fn validate_extension_rulesets(
    rulesets: &[&CompiledRuleset],
    static_rule_allocation: usize,
) -> Result<(), IndexError> {
    use crate::model::{MAX_NUMBER_OF_ENABLED_STATIC_RULESETS, MAX_NUMBER_OF_REGEX_RULES};
    let mut identifiers = std::collections::HashSet::new();
    let mut counts = [0usize; 3];
    let mut regex_counts = [0usize; 3];
    let mut static_rules = 0usize;
    for ruleset in rulesets {
        if !identifiers.insert(ruleset.ruleset_id()) {
            return Err(IndexError("duplicate ruleset id".into()));
        }
        let kind = match ruleset.kind {
            RulesetKind::Static => 0,
            RulesetKind::Dynamic => 1,
            RulesetKind::Session => 2,
        };
        counts[kind] += 1;
        regex_counts[kind] += ruleset
            .rules
            .iter()
            .filter(|rule| rule.rule.condition.regex_filter.is_some())
            .count();
        if kind == 0 {
            static_rules += ruleset.rules.len();
        }
    }
    if counts[0] > MAX_NUMBER_OF_ENABLED_STATIC_RULESETS || counts[1] > 1 || counts[2] > 1 {
        return Err(IndexError("enabled ruleset limit exceeded".into()));
    }
    if regex_counts
        .iter()
        .any(|&count| count > MAX_NUMBER_OF_REGEX_RULES)
    {
        return Err(IndexError("aggregate regex rule limit exceeded".into()));
    }
    if static_rules > static_rule_allocation {
        return Err(IndexError("static rule allocation exceeded".into()));
    }
    Ok(())
}

#[cfg(test)]
mod acceleration_tests {
    use super::*;
    use crate::{Action, Condition, ResourceType};

    #[test]
    fn normalized_re2_patterns_are_identical_in_regex_prefilter_and_full_match() {
        let patterns = [
            r"\Qhttps://example.test/\E",
            r"\C+",
            r"\141bc",
            r"\p{ASCII}+",
        ];
        let rules = patterns
            .iter()
            .enumerate()
            .map(|(id, pattern)| Rule {
                id: id as u32 + 1,
                priority: 1,
                action: Action::Block,
                condition: Condition {
                    regex_filter: Some((*pattern).into()),
                    ..Condition::default()
                },
            })
            .collect();
        let compiled = CompiledRuleset::compile("regex", RulesetKind::Static, rules).unwrap();
        let loaded = CompiledRuleset::load(&compiled.to_bytes()).unwrap();
        let rulesets = [&loaded];
        let extensions = [Extension {
            id: "extension",
            install_order: 1,
            rulesets: &rulesets,
        }];
        for url in [
            "https://example.test/a",
            "https://other.test/abc",
            "https://other.test/ABC",
        ] {
            let request = Request::new(url, url.split('/').nth(2).unwrap(), ResourceType::Script);
            assert_eq!(
                evaluate(&request, &extensions),
                evaluate_linear(&request, &extensions)
            );
        }
    }

    #[test]
    fn token_group_collisions_and_repeated_occurrences_preserve_decisions() {
        let rules = (0..4100)
            .map(|id| Rule {
                id: id + 1,
                priority: 1,
                action: Action::Block,
                condition: Condition {
                    url_filter: Some(format!("token-{id:05}")),
                    ..Condition::default()
                },
            })
            .collect();
        let compiled = CompiledRuleset::compile("tokens", RulesetKind::Static, rules).unwrap();
        let rulesets = [&compiled];
        let extensions = [Extension {
            id: "extension",
            install_order: 1,
            rulesets: &rulesets,
        }];
        for id in [0, 1, 4095, 4096, 4099] {
            let url = format!("https://example.test/token-{id:05}/token-{id:05}");
            let request = Request::new(&url, "example.test", ResourceType::Script);
            let indexed = evaluate(&request, &extensions);
            assert_eq!(indexed, evaluate_linear(&request, &extensions));
            assert_eq!(indexed.matched_rules.len(), 1);
            assert_eq!(indexed.matched_rules[0].rule_id, id + 1);
        }
    }
}
