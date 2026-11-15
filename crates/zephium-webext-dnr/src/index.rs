use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::OnceLock;

use regex::bytes::Regex;
use regex_automata::{
    dfa::{dense, sparse, Automaton},
    nfa::thompson,
    util::syntax,
    Input,
};

use crate::matcher::{
    evaluate_matching_candidates, Decision, MatchingCandidate, PreparedRule, Request, Stage,
};
use crate::model::*;
use crate::wire::{Reader, Wire};

pub const INDEX_FORMAT_VERSION: u32 = 2;
pub const MAX_INDEX_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_INDEX_RULES: usize = 1_000_000;
pub const MAX_SERIALIZED_DFA_BYTES_PER_RULESET: usize = 64 * 1024 * 1024;
pub const MAX_DFA_BUILD_BYTES: usize = 4 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"ZEPHDNR\0";
const HEADER: usize = 96;
const ROW: usize = 144;
const SLOT: usize = 32;
const TOKEN_BLOOM_BYTES: usize = 2048;
const NONE: u32 = u32::MAX;
const MAX_HASH_PROBES: usize = 128;
pub const MAX_LAZY_REGEX_RULES_PER_RULESET: usize = 64;
pub const MAX_LAZY_REGEX_RULES_PER_EXTENSION: usize = 128;

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
#[cold]
#[inline(never)]
fn bad() -> IndexError {
    IndexError("invalid or corrupt DNR index".into())
}
#[inline(always)]
fn charge_string_work(total: &mut usize, bytes: usize) -> Result<(), IndexError> {
    *total = total.checked_add(bytes).ok_or_else(bad)?;
    if *total > MAX_INDEX_BYTES {
        Err(bad())
    } else {
        Ok(())
    }
}

/// The backing bytes may be a sealed shared mapping. Loading retains only small
/// borrowed DFA descriptors; rules, strings and postings stay in that mapping.
pub struct CompiledRuleset<'a> {
    bytes: Cow<'a, [u8]>,
    programs: Vec<Program<'a>>,
    captures: Vec<OnceLock<Result<Regex, String>>>,
    pool: Option<&'a str>,
    token_count: usize,
    reference: OnceLock<Vec<PreparedRule>>,
}
enum Program<'a> {
    Borrowed(sparse::DFA<&'a [u8]>),
    Owned(sparse::DFA<Vec<u8>>),
    Lazy(OnceLock<Result<Regex, String>>),
}
impl Program<'_> {
    fn matches(&self, pattern: &str, sensitive: bool, url: &str) -> bool {
        match self {
            Self::Borrowed(dfa) => dfa
                .try_search_fwd(&Input::new(url).earliest(true))
                .ok()
                .flatten()
                .is_some(),
            Self::Owned(dfa) => dfa
                .try_search_fwd(&Input::new(url).earliest(true))
                .ok()
                .flatten()
                .is_some(),
            Self::Lazy(cache) => cache
                .get_or_init(|| crate::parse::compile_regex(pattern, sensitive))
                .as_ref()
                .is_ok_and(|regex| regex.is_match(url.as_bytes())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
    Domain(String),
    Initiator(String),
    Top(String),
    Token(String),
    Fallback,
}
impl Bucket {
    fn parts(&self) -> (u32, &str) {
        match self {
            Self::Domain(s) => (1, s),
            Self::Initiator(s) => (2, s),
            Self::Top(s) => (3, s),
            Self::Token(s) => (4, s),
            Self::Fallback => (5, ""),
        }
    }
}

struct Builder {
    bytes: Vec<u8>,
    strings: Vec<u8>,
    interned: HashMap<String, (u32, u32)>,
    limit: usize,
}
impl Builder {
    fn budget_error() -> IndexError {
        IndexError("index exceeds byte limit".into())
    }
    fn add(total: &mut usize, bytes: usize, limit: usize) -> Result<(), IndexError> {
        *total = total
            .checked_add(bytes)
            .filter(|&n| n <= limit)
            .ok_or_else(Self::budget_error)?;
        Ok(())
    }
    fn preflight(
        ruleset_id: &str,
        rules: &[Rule],
        input_limit: usize,
    ) -> Result<usize, IndexError> {
        Self::preflight_with_limits(
            ruleset_id,
            rules,
            input_limit,
            MAX_DECODED_ACTION_BYTES_PER_RULESET,
        )
    }
    fn preflight_with_limits(
        ruleset_id: &str,
        rules: &[Rule],
        input_limit: usize,
        decoded_limit: usize,
    ) -> Result<usize, IndexError> {
        let mut logical = 4usize;
        let mut decoded_bytes = 0usize;
        let mut base = HEADER;
        let mut bucket_references = 0usize;
        Self::add(
            &mut base,
            rules
                .len()
                .checked_mul(ROW)
                .ok_or_else(Self::budget_error)?,
            MAX_INDEX_BYTES,
        )?;
        Self::add(&mut base, TOKEN_BLOOM_BYTES, MAX_INDEX_BYTES)?;
        Self::add(&mut base, ruleset_id.len(), MAX_INDEX_BYTES)?;
        for rule in rules {
            for size in [
                rule.action.heap_len(),
                rule.condition.response_headers.heap_len(),
                rule.condition.excluded_response_headers.heap_len(),
            ] {
                decoded_bytes = decoded_bytes
                    .checked_add(size.ok_or_else(Self::budget_error)?)
                    .filter(|&n| n <= decoded_limit)
                    .ok_or_else(|| IndexError("decoded action data exceeds byte limit".into()))?;
            }
            Self::add(
                &mut logical,
                rule.encoded_len().ok_or_else(Self::budget_error)?,
                input_limit,
            )?;
            let c = &rule.condition;
            let scoped_domains = c
                .request_domains
                .as_ref()
                .or(c.initiator_domains.as_ref())
                .or(c.domains.as_ref())
                .or(c.top_domains.as_ref());
            Self::add(
                &mut bucket_references,
                scoped_domains.map_or(1, Vec::len),
                MAX_INDEX_BYTES / 8,
            )?;
            for size in [
                rule.action.encoded_len(),
                c.response_headers.encoded_len(),
                c.excluded_response_headers.encoded_len(),
            ] {
                Self::add(
                    &mut base,
                    size.ok_or_else(Self::budget_error)?,
                    MAX_INDEX_BYTES,
                )?;
            }
            for filter in [&c.url_filter, &c.regex_filter].into_iter().flatten() {
                Self::add(&mut base, filter.len(), MAX_INDEX_BYTES)?;
            }
            if c.regex_filter.is_some() {
                Self::add(&mut base, 12, MAX_INDEX_BYTES)?;
            }
            for domains in [
                c.request_domains.as_deref(),
                c.excluded_request_domains.as_deref(),
                c.initiator_domains.as_deref().or(c.domains.as_deref()),
                c.excluded_initiator_domains
                    .as_deref()
                    .or(c.excluded_domains.as_deref()),
                c.top_domains.as_deref(),
                c.excluded_top_domains.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                Self::add(
                    &mut base,
                    domains
                        .len()
                        .checked_mul(16)
                        .ok_or_else(Self::budget_error)?,
                    MAX_INDEX_BYTES,
                )?;
                for domain in domains {
                    Self::add(&mut base, domain.len(), MAX_INDEX_BYTES)?;
                }
            }
            for tabs in [&c.tab_ids, &c.excluded_tab_ids].into_iter().flatten() {
                Self::add(
                    &mut base,
                    tabs.len().checked_mul(4).ok_or_else(Self::budget_error)?,
                    MAX_INDEX_BYTES,
                )?;
            }
        }
        let prefixes = bucket_references
            .div_ceil(4)
            .max(1)
            .checked_next_power_of_two()
            .ok_or_else(Self::budget_error)?;
        let mut projected = base;
        Self::add(
            &mut projected,
            bucket_references
                .checked_mul(SLOT)
                .ok_or_else(Self::budget_error)?,
            MAX_INDEX_BYTES,
        )?;
        Self::add(
            &mut projected,
            prefixes
                .checked_add(1)
                .and_then(|count| count.checked_mul(4))
                .ok_or_else(Self::budget_error)?,
            MAX_INDEX_BYTES,
        )?;
        Self::add(
            &mut projected,
            bucket_references
                .checked_mul(8)
                .ok_or_else(Self::budget_error)?,
            MAX_INDEX_BYTES,
        )?;
        Ok(base)
    }
    fn ensure(&self, additional: usize) -> Result<(), IndexError> {
        let mut total = self.bytes.len();
        Self::add(&mut total, self.strings.len(), self.limit)?;
        Self::add(&mut total, additional, self.limit)
    }
    fn reserve_vec(bytes: &mut Vec<u8>, additional: usize, limit: usize) -> Result<(), IndexError> {
        let needed = bytes
            .len()
            .checked_add(additional)
            .filter(|&n| n <= limit)
            .ok_or_else(Self::budget_error)?;
        if needed > bytes.capacity() {
            let target = needed.max(bytes.capacity().saturating_mul(2)).min(limit);
            bytes
                .try_reserve_exact(target - bytes.len())
                .map_err(|_| Self::budget_error())?;
        }
        Ok(())
    }
    fn reserve(&mut self, additional: usize) -> Result<(), IndexError> {
        self.ensure(additional)?;
        Self::reserve_vec(&mut self.bytes, additional, self.limit)
    }
    fn extend(&mut self, bytes: &[u8]) -> Result<(), IndexError> {
        self.reserve(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn write<T: Wire>(&mut self, value: &T) -> Result<(), IndexError> {
        self.reserve(value.encoded_len().ok_or_else(Self::budget_error)?)?;
        value.write(&mut self.bytes);
        Ok(())
    }
    fn resize(&mut self, length: usize) -> Result<(), IndexError> {
        self.reserve(
            length
                .checked_sub(self.bytes.len())
                .ok_or_else(Self::budget_error)?,
        )?;
        self.bytes.resize(length, 0);
        Ok(())
    }
    fn string(&mut self, value: &str) -> Result<(u32, u32), IndexError> {
        if let Some(&entry) = self.interned.get(value) {
            return Ok(entry);
        }
        self.ensure(value.len())?;
        Self::reserve_vec(&mut self.strings, value.len(), self.limit)?;
        let entry = (self.strings.len() as u32, value.len() as u32);
        self.strings.extend_from_slice(value.as_bytes());
        self.interned.insert(value.to_owned(), entry);
        Ok(entry)
    }
    fn string_at(&mut self, at: usize, value: &str) -> Result<(), IndexError> {
        let (off, len) = self.string(value)?;
        put(&mut self.bytes, at, off);
        put(&mut self.bytes, at + 4, len);
        Ok(())
    }
    fn domains(&mut self, at: usize, domains: Option<&[String]>) -> Result<(), IndexError> {
        let Some(domains) = domains else {
            put(&mut self.bytes, at, NONE);
            return Ok(());
        };
        self.ensure(
            domains
                .len()
                .checked_mul(16)
                .ok_or_else(Self::budget_error)?,
        )?;
        let mut values: Vec<_> = domains
            .iter()
            .map(|s| (hash_key(0, s.as_bytes()), s.to_ascii_lowercase()))
            .collect();
        values.sort_unstable();
        values.dedup();
        let start = self.bytes.len() as u32;
        put(&mut self.bytes, at, start);
        put(&mut self.bytes, at + 4, values.len() as u32);
        for (hash, value) in values {
            let (off, len) = self.string(&value)?;
            self.extend(&hash.to_le_bytes())?;
            self.extend(&off.to_le_bytes())?;
            self.extend(&len.to_le_bytes())?;
        }
        Ok(())
    }
    fn tabs(&mut self, at: usize, tabs: Option<&[i32]>) -> Result<(), IndexError> {
        let Some(tabs) = tabs else {
            put(&mut self.bytes, at, NONE);
            return Ok(());
        };
        self.ensure(tabs.len().checked_mul(4).ok_or_else(Self::budget_error)?)?;
        let mut tabs = tabs.to_vec();
        tabs.sort_unstable();
        tabs.dedup();
        let start = self.bytes.len() as u32;
        put(&mut self.bytes, at, start);
        put(&mut self.bytes, at + 4, tabs.len() as u32);
        for tab in tabs {
            self.extend(&tab.to_le_bytes())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod compiler_budget_tests {
    use super::*;

    fn builder(limit: usize) -> Builder {
        Builder {
            bytes: vec![0; 8],
            strings: Vec::new(),
            interned: HashMap::new(),
            limit,
        }
    }

    #[test]
    fn growth_is_admitted_before_mutating_buffers_or_interning_strings() {
        let mut b = builder(16);
        assert_eq!(b.string("same").unwrap(), (0, 4));
        assert_eq!(b.string("same").unwrap(), (0, 4));
        assert!(b.string("excess").is_err());
        assert_eq!(b.strings, b"same");
        assert_eq!(b.interned.len(), 1);
        b.extend(&[1, 2, 3, 4]).unwrap();
        let before = b.bytes.clone();
        assert!(b.extend(&[5]).is_err());
        assert!(b.write(&String::from("overflow")).is_err());
        assert!(b.resize(17).is_err());
        assert_eq!(b.bytes, before);
        assert!(b.bytes.capacity() <= b.limit);
    }

    #[test]
    fn list_expansion_and_typed_payloads_refuse_small_budgets_before_building() {
        let mut b = builder(32);
        let domains = vec!["domain.test".to_owned(); 8];
        assert!(b.domains(0, Some(&domains)).is_err());
        assert!(b.tabs(0, Some(&[1; 8])).is_err());
        assert_eq!(b.bytes, [0; 8]);
        assert!(b.strings.is_empty());
        let rules = [Rule {
            id: 1,
            priority: 1,
            action: Action::Block,
            condition: Condition {
                url_filter: Some("a".repeat(256)),
                ..Condition::default()
            },
        }];
        assert!(Builder::preflight("typed", &rules, 128).is_err());
        assert!(Builder::preflight("typed", &rules, 1024).is_ok());
    }

    #[test]
    fn checked_wire_lengths_match_nested_rule_encodings() {
        let mut rule = Rule {
            id: 1,
            priority: 1,
            action: Action::Block,
            condition: Condition {
                request_domains: Some(vec!["example.test".into()]),
                response_headers: Some(vec![HeaderCondition {
                    header: "content-type".into(),
                    values: Some(vec!["text/*".into()]),
                    excluded_values: None,
                }]),
                ..Condition::default()
            },
        };
        let actions = [
            Action::Block,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    transform: Some(UrlTransform {
                        query_transform: Some(QueryTransform {
                            remove_params: vec!["tracking".into()],
                            add_or_replace_params: vec![QueryKeyValue {
                                key: "name".into(),
                                value: "value".into(),
                                replace_only: true,
                            }],
                        }),
                        ..UrlTransform::default()
                    }),
                    ..Redirect::default()
                }),
            },
            Action::ModifyHeaders {
                request_headers: vec![],
                response_headers: vec![HeaderModification {
                    header: "x-policy".into(),
                    operation: HeaderOperation::Set,
                    value: Some("value".into()),
                }],
            },
        ];
        for action in actions {
            rule.action = action;
            let mut encoded = Vec::new();
            rule.write(&mut encoded);
            assert_eq!(rule.encoded_len(), Some(encoded.len()));
            let mut reader = Reader::new(&encoded);
            Rule::validate(&mut reader).unwrap();
            assert_eq!(rule.heap_len(), Some(reader.decoded_bytes()));
            reader.finish().unwrap();
            let mut reader = Reader::new(&encoded);
            let decoded = Rule::read(&mut reader).unwrap();
            assert_eq!(decoded, rule);
            assert_eq!(rule.heap_len(), Some(reader.decoded_bytes()));
            reader.finish().unwrap();
        }
    }

    #[test]
    fn typed_action_heap_budget_precedes_compiler_construction() {
        let rules = [Rule {
            id: 1,
            priority: 1,
            action: Action::ModifyHeaders {
                request_headers: vec![],
                response_headers: vec![HeaderModification {
                    header: "x-policy".into(),
                    operation: HeaderOperation::Set,
                    value: Some("value".into()),
                }],
            },
            condition: Condition::default(),
        }];
        assert!(Builder::preflight_with_limits("typed", &rules, 1024, 16).is_err());
        assert!(Builder::preflight_with_limits("typed", &rules, 1024, 1024).is_ok());
    }
}

impl CompiledRuleset<'static> {
    pub fn compile(
        ruleset_id: &str,
        kind: RulesetKind,
        rules: Vec<Rule>,
    ) -> Result<Self, IndexError> {
        if ruleset_id.is_empty() || ruleset_id.len() > 1024 || rules.len() > MAX_INDEX_RULES {
            return Err(bad());
        }
        let mut base_bytes = Builder::preflight(ruleset_id, &rules, MAX_RULESET_JSON_BYTES)?;
        if rules
            .iter()
            .filter(|rule| rule.condition.regex_filter.is_some())
            .count()
            > MAX_NUMBER_OF_REGEX_RULES
        {
            return Err(IndexError("MAX_NUMBER_OF_REGEX_RULES exceeded".into()));
        }
        let rejected = crate::parse::validate_rules(&rules, kind);
        if !rejected.is_empty() {
            return Err(IndexError(format!("{rejected:?}")));
        }
        let buckets = bucket_rules(&rules);
        let slots = buckets.len();
        let prefixes = slots
            .div_ceil(4)
            .max(1)
            .checked_next_power_of_two()
            .ok_or_else(Builder::budget_error)?;
        let prefix_bytes = prefixes
            .checked_add(1)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(Builder::budget_error)?;
        let table_bytes = slots.checked_mul(SLOT).ok_or_else(Builder::budget_error)?;
        Builder::add(&mut base_bytes, table_bytes, MAX_INDEX_BYTES)?;
        Builder::add(&mut base_bytes, prefix_bytes, MAX_INDEX_BYTES)?;
        let mut ordered: Vec<_> = buckets
            .iter()
            .map(|(bucket, ids)| {
                (
                    hash_key(bucket.parts().0, bucket.parts().1.as_bytes()),
                    bucket,
                    ids,
                )
            })
            .collect();
        ordered.sort_unstable_by(|left, right| {
            let l = left.1.parts();
            let r = right.1.parts();
            (left.0 as usize & (prefixes - 1), left.0, l.0, l.1).cmp(&(
                right.0 as usize & (prefixes - 1),
                right.0,
                r.0,
                r.1,
            ))
        });
        for (bucket, ids) in &buckets {
            Builder::add(&mut base_bytes, bucket.parts().1.len(), MAX_INDEX_BYTES)?;
            Builder::add(
                &mut base_bytes,
                ids.len().checked_mul(8).ok_or_else(Builder::budget_error)?,
                MAX_INDEX_BYTES,
            )?;
        }
        let dfa_limit = MAX_SERIALIZED_DFA_BYTES_PER_RULESET.min(MAX_INDEX_BYTES - base_bytes);
        let mut b = Builder {
            bytes: vec![0; HEADER + rules.len() * ROW],
            strings: Vec::new(),
            interned: HashMap::new(),
            limit: MAX_INDEX_BYTES,
        };
        let mut programs = Vec::new();
        let mut dfa_directory = Vec::new();
        let mut dfa_bytes = 0;
        let mut nfa_bytes = 0u64;
        let mut lazy_count = 0;
        b.string_at(64, ruleset_id)?;
        for (i, rule) in rules.iter().enumerate() {
            let at = HEADER + i * ROW;
            let c = &rule.condition;
            put(&mut b.bytes, at, rule.id);
            put(&mut b.bytes, at + 4, rule.priority);
            put(&mut b.bytes, at + 8, action_tag(&rule.action));
            let flags = u32::from(c.is_url_filter_case_sensitive)
                | (u32::from(c.has_response_header_condition()) << 1)
                | (u32::from(c.request_methods.is_some()) << 2);
            put(&mut b.bytes, at + 12, flags);
            b.string_at(at + 16, c.url_filter.as_deref().unwrap_or(""))?;
            b.string_at(at + 24, c.regex_filter.as_deref().unwrap_or(""))?;
            if let Some(filter) = &c.url_filter {
                let mut offset = 0;
                let mut longest = (0, 0);
                for part in filter.split(['*', '^', '|']) {
                    if part.len() >= 3 && part.len() > longest.1 {
                        longest = (offset, part.len());
                    }
                    offset += part.len() + 1;
                }
                put(&mut b.bytes, at + 124, longest.0 as u32);
                put(&mut b.bytes, at + 128, longest.1 as u32);
            }
            let types = if let Some(types) = &c.resource_types {
                types.iter().fold(0, |m, t| m | (1 << (*t as u32)))
            } else if let Some(types) = &c.excluded_resource_types {
                let excluded = types.iter().fold(0, |m, t| m | (1 << (*t as u32)));
                if types.is_empty() {
                    0x7ffe
                } else {
                    0x7fff & !excluded
                }
            } else {
                0x7ffe
            };
            put(&mut b.bytes, at + 32, types);
            let methods = c
                .request_methods
                .as_ref()
                .map_or(0x1ff, |ms| ms.iter().fold(0, |m, t| m | (1 << (*t as u32))));
            let excluded = c
                .excluded_request_methods
                .as_ref()
                .map_or(0, |ms| ms.iter().fold(0, |m, t| m | (1 << (*t as u32))));
            put(&mut b.bytes, at + 36, methods | (excluded << 9));
            put(
                &mut b.bytes,
                at + 40,
                match c.domain_type {
                    None => 0,
                    Some(DomainType::FirstParty) => 1,
                    Some(DomainType::ThirdParty) => 2,
                },
            );
            let record_start = b.bytes.len() as u32;
            put(&mut b.bytes, at + 44, record_start);
            let before = b.bytes.len();
            b.write(&rule.action)?;
            b.write(&c.response_headers)?;
            b.write(&c.excluded_response_headers)?;
            let record_len = (b.bytes.len() - before) as u32;
            put(&mut b.bytes, at + 48, record_len);
            put(&mut b.bytes, at + 52, NONE);
            if let Some(pattern) = &c.regex_filter {
                nfa_bytes +=
                    crate::parse::regex_nfa_memory_usage(pattern, c.is_url_filter_case_sensitive)?
                        as u64;
                let normalized = crate::parse::normalize_regex(pattern)?;
                let remaining = dfa_limit - dfa_bytes;
                let serialized = if remaining == 0 {
                    None
                } else {
                    let build_limit = MAX_DFA_BUILD_BYTES.min(remaining);
                    dense::Builder::new()
                        .configure(
                            dense::Config::new()
                                .dfa_size_limit(Some(build_limit))
                                .determinize_size_limit(Some(build_limit)),
                        )
                        .syntax(
                            syntax::Config::new()
                                .unicode(false)
                                .utf8(false)
                                .case_insensitive(!c.is_url_filter_case_sensitive),
                        )
                        .thompson(
                            thompson::Config::new()
                                .utf8(false)
                                .nfa_size_limit(Some(MAX_REGEX_NFA_BYTES)),
                        )
                        .build(&normalized)
                        .ok()
                        .and_then(|dfa| dfa.to_sparse().ok())
                        .filter(|dfa| dfa.write_to_len() <= remaining)
                        .map(|dfa| {
                            let bytes = dfa.to_bytes_native_endian();
                            (dfa, bytes)
                        })
                };
                let id = programs.len() as u32;
                put(&mut b.bytes, at + 52, id);
                if let Some((dfa, bytes)) = serialized {
                    dfa_bytes += bytes.len();
                    let offset = b.bytes.len();
                    b.extend(&bytes)?;
                    dfa_directory.push((offset as u32, bytes.len() as u32, i as u32));
                    programs.push(Program::Owned(dfa));
                } else {
                    if lazy_count >= MAX_LAZY_REGEX_RULES_PER_RULESET {
                        return Err(IndexError("lazy regex budget exceeded".into()));
                    }
                    lazy_count += 1;
                    dfa_directory.push((0, 0, i as u32));
                    programs.push(Program::Lazy(OnceLock::new()));
                }
            }
            b.domains(at + 60, c.request_domains.as_deref())?;
            b.domains(at + 68, c.excluded_request_domains.as_deref())?;
            b.domains(
                at + 76,
                c.initiator_domains.as_deref().or(c.domains.as_deref()),
            )?;
            b.domains(
                at + 84,
                c.excluded_initiator_domains
                    .as_deref()
                    .or(c.excluded_domains.as_deref()),
            )?;
            b.domains(at + 92, c.top_domains.as_deref())?;
            b.domains(at + 100, c.excluded_top_domains.as_deref())?;
            b.tabs(at + 108, c.tab_ids.as_deref())?;
            b.tabs(at + 116, c.excluded_tab_ids.as_deref())?;
        }
        let bloom = b.bytes.len();
        b.resize(
            bloom
                .checked_add(TOKEN_BLOOM_BYTES)
                .ok_or_else(Builder::budget_error)?,
        )?;
        let prefix_directory = b.bytes.len();
        b.resize(
            prefix_directory
                .checked_add(prefix_bytes)
                .ok_or_else(Builder::budget_error)?,
        )?;
        let mut cursor = 0;
        for prefix in 0..prefixes {
            put(&mut b.bytes, prefix_directory + prefix * 4, cursor as u32);
            let start = cursor;
            while cursor < ordered.len() && ordered[cursor].0 as usize & (prefixes - 1) == prefix {
                cursor += 1;
            }
            if cursor - start > MAX_HASH_PROBES {
                return Err(IndexError("index hash prefix budget exceeded".into()));
            }
        }
        put(&mut b.bytes, prefix_directory + prefixes * 4, slots as u32);
        let table = b.bytes.len();
        b.resize(
            table
                .checked_add(table_bytes)
                .ok_or_else(Builder::budget_error)?,
        )?;
        for (slot, (hash, bucket, ids)) in ordered.iter().enumerate() {
            let hash = *hash;
            let (kind, key) = bucket.parts();
            if kind == 4 {
                let bit = hash as usize & (TOKEN_BLOOM_BYTES * 8 - 1);
                b.bytes[bloom + bit / 8] |= 1 << (bit % 8);
            }
            let at = table + slot * SLOT;
            let (off, len) = b.string(key)?;
            for &id in ids.iter() {
                let row = HEADER + id * ROW;
                put(&mut b.bytes, row + 132, kind);
                let c = &rules[id].condition;
                let scoped = match kind {
                    1 => c.request_domains.is_some(),
                    2 => c.initiator_domains.is_some() || c.domains.is_some(),
                    3 => c.top_domains.is_some(),
                    _ => false,
                };
                if !scoped {
                    put(&mut b.bytes, row + 136, off);
                    put(&mut b.bytes, row + 140, len);
                }
            }
            put64(&mut b.bytes, at, hash);
            put(&mut b.bytes, at + 8, kind);
            put(&mut b.bytes, at + 12, off);
            put(&mut b.bytes, at + 16, len);
            let postings_start = b.bytes.len() as u32;
            put(&mut b.bytes, at + 20, postings_start);
            put(&mut b.bytes, at + 24, ids.len() as u32);
            for &id in ids.iter() {
                b.extend(&(id as u32).to_le_bytes())?;
                let row = HEADER + id * ROW;
                let domain_index = if get(&b.bytes, row + 140) == 0 && kind != 5 {
                    let field = match kind {
                        1 => 60,
                        2 => 76,
                        3 => 92,
                        _ => return Err(bad()),
                    };
                    let start = get(&b.bytes, row + field) as usize;
                    let count = get(&b.bytes, row + field + 4) as usize;
                    let key_hash = hash_key(0, key.as_bytes());
                    let mut low = 0;
                    let mut high = count;
                    while low < high {
                        let middle = (low + high) / 2;
                        if get64(&b.bytes, start + middle * 16) < key_hash {
                            low = middle + 1;
                        } else {
                            high = middle;
                        }
                    }
                    while low < count {
                        let entry = start + low * 16;
                        let off = get(&b.bytes, entry + 8) as usize;
                        let len = get(&b.bytes, entry + 12) as usize;
                        if b.strings[off..off + len].eq_ignore_ascii_case(key.as_bytes()) {
                            break;
                        }
                        low += 1;
                    }
                    if low == count {
                        return Err(bad());
                    }
                    low as u32
                } else {
                    NONE
                };
                b.extend(&domain_index.to_le_bytes())?;
            }
        }
        let directory = b.bytes.len();
        for (off, len, rule) in dfa_directory {
            b.write(&off)?;
            b.write(&len)?;
            b.write(&rule)?;
        }
        let strings = b.bytes.len();
        b.ensure(0)?;
        Builder::reserve_vec(&mut b.bytes, b.strings.len(), MAX_INDEX_BYTES)?;
        b.bytes.extend_from_slice(&b.strings);
        if b.bytes.len() > MAX_INDEX_BYTES {
            return Err(IndexError("index exceeds byte limit".into()));
        }
        let total = b.bytes.len();
        b.bytes[..8].copy_from_slice(MAGIC);
        for (off, value) in [
            (8, INDEX_FORMAT_VERSION),
            (12, total as u32),
            (16, rules.len() as u32),
            (20, HEADER as u32),
            (24, ROW as u32),
            (28, table as u32),
            (32, slots as u32),
            (36, directory as u32),
            (40, programs.len() as u32),
            (44, strings as u32),
            (48, b.strings.len() as u32),
            (72, bloom as u32),
            (76, TOKEN_BLOOM_BYTES as u32),
            (88, prefixes as u32),
            (
                52,
                match kind {
                    RulesetKind::Static => 0,
                    RulesetKind::Dynamic => 1,
                    RulesetKind::Session => 2,
                },
            ),
        ] {
            put(&mut b.bytes, off, value);
        }
        if programs
            .iter()
            .filter(|p| matches!(p, Program::Lazy(_)))
            .count()
            > MAX_LAZY_REGEX_RULES_PER_RULESET
        {
            return Err(IndexError("lazy regex budget exceeded".into()));
        }
        put64(&mut b.bytes, 80, nfa_bytes);
        let seal = checksum(&b.bytes[HEADER..]);
        put64(&mut b.bytes, 56, seal);
        Ok(Self {
            bytes: Cow::Owned(b.bytes),
            captures: (0..programs.len()).map(|_| OnceLock::new()).collect(),
            programs,
            pool: None,
            reference: OnceLock::new(),
            token_count: buckets
                .keys()
                .filter(|key| matches!(key, Bucket::Token(_)))
                .count(),
        })
    }
}

impl<'a> CompiledRuleset<'a> {
    /// The owner authenticates compiler output and seals the backing mapping
    /// before delivering these immutable bytes to a renderer.
    pub fn load(bytes: &'a [u8]) -> Result<Self, IndexError> {
        Self::load_inner(bytes, |_| {})
    }

    /// Report validation phase boundaries for compiler-side corpus diagnostics.
    #[cfg(feature = "compile")]
    pub fn load_profiled(
        bytes: &'a [u8],
        mark: impl FnMut(&'static str),
    ) -> Result<Self, IndexError> {
        Self::load_inner(bytes, mark)
    }

    fn load_inner(bytes: &'a [u8], mut mark: impl FnMut(&'static str)) -> Result<Self, IndexError> {
        if bytes.len() < HEADER
            || bytes.len() > MAX_INDEX_BYTES
            || bytes.get(..8) != Some(MAGIC.as_slice())
        {
            return Err(bad());
        }
        let count = get(bytes, 16) as usize;
        let table = get(bytes, 28) as usize;
        let slots = get(bytes, 32) as usize;
        let directory = get(bytes, 36) as usize;
        let program_count = get(bytes, 40) as usize;
        let strings = get(bytes, 44) as usize;
        let string_len = get(bytes, 48) as usize;
        let bloom = get(bytes, 72) as usize;
        let prefixes = get(bytes, 88) as usize;
        let prefix_directory = bloom.checked_add(TOKEN_BLOOM_BYTES).ok_or_else(bad)?;
        if get(bytes, 8) != INDEX_FORMAT_VERSION
            || get(bytes, 12) as usize != bytes.len()
            || count > MAX_INDEX_RULES
            || get(bytes, 20) as usize != HEADER
            || get(bytes, 24) as usize != ROW
            || bloom < HEADER + count * ROW
            || prefixes == 0
            || !prefixes.is_power_of_two()
            || prefixes > MAX_INDEX_BYTES / 4
            || prefixes
                != slots
                    .div_ceil(4)
                    .max(1)
                    .checked_next_power_of_two()
                    .ok_or_else(bad)?
            || prefixes
                .checked_add(1)
                .and_then(|n| n.checked_mul(4))
                .and_then(|n| prefix_directory.checked_add(n))
                != Some(table)
            || get(bytes, 76) as usize != TOKEN_BLOOM_BYTES
            || slots > MAX_INDEX_BYTES / SLOT
            || table
                .checked_add(slots * SLOT)
                .is_none_or(|end| end > directory)
            || directory
                .checked_add(program_count.checked_mul(12).ok_or_else(bad)?)
                .is_none_or(|end| end != strings)
            || strings.checked_add(string_len) != Some(bytes.len())
            || get(bytes, 52) > 2
            || program_count > count
            || program_count > MAX_NUMBER_OF_REGEX_RULES
            || get64(bytes, 80) > MAX_REGEX_NFA_BYTES_PER_RULESET as u64
            || get(bytes, 92) != 0
            || checksum(&bytes[HEADER..]) != get64(bytes, 56)
        {
            return Err(bad());
        }
        mark("checksum");
        let mut previous = 0;
        for prefix in 0..=prefixes {
            let value = get(bytes, prefix_directory + prefix * 4) as usize;
            if value > slots
                || value < previous
                || (prefix == 0 && value != 0)
                || (prefix > 0 && value - previous > MAX_HASH_PROBES)
                || (prefix == prefixes && value != slots)
            {
                return Err(bad());
            }
            previous = value;
        }
        let pool = std::str::from_utf8(&bytes[strings..]).map_err(|_| bad())?;
        let checked_string = |off: u32, len: u32| -> Result<&'a str, IndexError> {
            let start = off as usize;
            let end = start.checked_add(len as usize).ok_or_else(bad)?;
            pool.get(start..end).ok_or_else(bad)
        };
        let s = Self {
            bytes: Cow::Borrowed(bytes),
            programs: Vec::new(),
            captures: Vec::new(),
            pool: Some(pool),
            token_count: 0,
            reference: OnceLock::new(),
        };
        let identity = checked_string(get(bytes, 64), get(bytes, 68))?;
        if identity.is_empty() || identity.len() > 1024 {
            return Err(bad());
        }
        mark("pool");

        let mut next = HEADER + count * ROW;
        let mut next_program = 0;
        let mut unsafe_rules = 0;
        let mut header_patterns = 0usize;
        let mut record_bytes = 0usize;
        let mut decoded_action_bytes = 0usize;
        let mut referenced_bytes = 0usize;
        for (i, frame) in bytes[HEADER..HEADER + count * ROW]
            .chunks_exact(ROW)
            .enumerate()
        {
            let row: &[u8; ROW] = frame.try_into().expect("fixed rule row");
            if get(row, 0) == 0
                || get(row, 0) > i32::MAX as u32
                || get(row, 4) == 0
                || get(row, 4) > i32::MAX as u32
                || get(row, 8) > 5
                || get(row, 12) > 7
                || get(row, 32) > 0x7fff
                || get(row, 36) > 0x3ffff
                || get(row, 40) > 2
            {
                return Err(bad());
            }
            for off in [16, 24] {
                charge_string_work(&mut referenced_bytes, get(row, off + 4) as usize)?;
                checked_string(get(row, off), get(row, off + 4))?;
            }
            let methods = get(row, 36);
            if (get(row, 12) & 4 != 0 && (methods & 0x1ff == 0 || methods >> 9 != 0))
                || (get(row, 12) & 4 == 0 && methods & 0x1ff != 0x1ff)
            {
                return Err(bad());
            }
            if get(row, 20) > 0 && get(row, 28) > 0 {
                return Err(bad());
            }
            let filter = checked_string(get(row, 16), get(row, 20))?;
            let literal_start = get(row, 124) as usize;
            let literal_len = get(row, 128) as usize;
            if literal_start
                .checked_add(literal_len)
                .is_none_or(|end| end > filter.len())
                || filter.as_bytes()[literal_start..literal_start + literal_len]
                    .iter()
                    .any(|b| b"*^|".contains(b))
            {
                return Err(bad());
            }
            if !filter.is_ascii() || filter.starts_with("||*") {
                return Err(bad());
            }
            if get(row, 28) > 0 {
                crate::parse::validate_regex_source(
                    checked_string(get(row, 24), get(row, 28))?,
                    get(row, 12) & 1 != 0,
                )
                .map_err(IndexError)?;
            }
            let routing = get(row, 132);
            let route = checked_string(get(row, 136), get(row, 140))?;
            if routing == 4 {
                charge_string_work(&mut referenced_bytes, filter.len())?;
            }
            if !(1..=5).contains(&routing)
                || (routing == 4
                    && (route.len() != 3
                        || route.as_bytes().iter().any(|b| b"*^|".contains(b))
                        || !crate::matcher::contains_literal(
                            filter.as_bytes(),
                            route.as_bytes(),
                            false,
                        )))
                || (routing == 5 && !route.is_empty())
            {
                return Err(bad());
            }
            if (1..=3).contains(&routing) {
                if route.is_empty() {
                    let off = match routing {
                        1 => 60,
                        2 => 76,
                        _ => 92,
                    };
                    if get(row, off) == NONE || get(row, off + 4) == 0 {
                        return Err(bad());
                    }
                } else if routing != 1
                    || !filter
                        .strip_prefix("||")
                        .and_then(|f| f.find(['^', '/', ':', '|']).map(|end| &f[..end]))
                        .is_some_and(|host| {
                            !host.is_empty()
                                && host
                                    .bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
                                && host.eq_ignore_ascii_case(route)
                        })
                {
                    return Err(bad());
                }
            }
            if get(row, 8) == 2 && (get(row, 32) == 0 || get(row, 32) & !3 != 0) {
                return Err(bad());
            }
            if get(row, 8) >= 4 {
                unsafe_rules += 1;
            }
            if get(row, 44) as usize != next {
                return Err(bad());
            }
            let record = s.range(get(row, 44), get(row, 48), 1, table)?;
            next += record.len();
            record_bytes = record_bytes.checked_add(record.len()).ok_or_else(bad)?;
            if record_bytes > MAX_RULESET_JSON_BYTES {
                return Err(bad());
            }
            if get(row, 8) <= 3 && get(row, 12) & 2 == 0 {
                if record != [get(row, 8) as u8, 0, 0] {
                    return Err(bad());
                }
            } else {
                let mut reader = Reader::new(record);
                if record.first().copied().map(u32::from) != Some(get(row, 8)) {
                    return Err(bad());
                }
                Action::validate(&mut reader)?;
                let response = bool::read(&mut reader)?;
                if response {
                    Vec::<HeaderCondition>::validate(&mut reader)?;
                }
                let excluded = bool::read(&mut reader)?;
                if excluded {
                    Vec::<HeaderCondition>::validate(&mut reader)?;
                }
                if (response || excluded) != (get(row, 12) & 2 != 0) {
                    return Err(bad());
                }
                decoded_action_bytes = decoded_action_bytes
                    .checked_add(reader.decoded_bytes())
                    .ok_or_else(bad)?;
                header_patterns = header_patterns
                    .checked_add(reader.header_pattern_count())
                    .ok_or_else(bad)?;
                if decoded_action_bytes > MAX_DECODED_ACTION_BYTES_PER_RULESET
                    || header_patterns > MAX_HEADER_PATTERNS_PER_RULESET
                {
                    return Err(bad());
                }
                reader.finish()?;
            }
            if get(row, 8) >= 4 || get(row, 12) & 2 != 0 {
                let rule = s.row(i).validation_rule();
                crate::parse::validate_loaded_action(&rule).map_err(IndexError)?;
            }
            if get(row, 52) != NONE && get(row, 52) as usize >= program_count {
                return Err(bad());
            }
            if (get(row, 52) == NONE) != (get(row, 28) == 0) {
                return Err(bad());
            }
            if get(row, 52) != NONE {
                let id = get(row, 52) as usize;
                if id != next_program {
                    return Err(bad());
                }
                next_program += 1;
                let program_at = directory + id * 12;
                let off = get(bytes, program_at);
                let len = get(bytes, program_at + 4);
                if len != 0 {
                    if off as usize != next {
                        return Err(bad());
                    }
                    s.range(off, len, 1, table)?;
                    next += len as usize;
                }
            }
            for off in [60, 68, 76, 84, 92, 100] {
                let start = get(row, off);
                let len = get(row, off + 4);
                if start == NONE {
                    if len != 0 {
                        return Err(bad());
                    }
                    continue;
                }
                if start as usize != next || ([60, 76, 92].contains(&off) && len == 0) {
                    return Err(bad());
                }
                let list = s.range(start, len, 16, table)?;
                next += list.len();
                let mut previous: Option<(u64, &str)> = None;
                for entry in list.chunks_exact(16) {
                    let entry: &[u8; 16] = entry.try_into().expect("fixed domain entry");
                    charge_string_work(&mut referenced_bytes, get(entry, 12) as usize)?;
                    let text = checked_string(get(entry, 8), get(entry, 12))?;
                    let hash = get64(entry, 0);
                    if hash != normalized_domain_hash(text).ok_or_else(bad)?
                        || previous.is_some_and(|old| old >= (hash, text))
                    {
                        return Err(bad());
                    }
                    previous = Some((hash, text));
                }
            }
            for off in [108, 116] {
                if get(row, off) != NONE && s.kind() != RulesetKind::Session {
                    return Err(bad());
                }
                if get(row, off) == NONE {
                    if get(row, off + 4) != 0 {
                        return Err(bad());
                    }
                } else {
                    if get(row, off) as usize != next {
                        return Err(bad());
                    }
                    let span = s.range(get(row, off), get(row, off + 4), 4, table)?;
                    next += span.len();
                    if (off == 108 && span.is_empty())
                        || span
                            .chunks_exact(4)
                            .map(|e| i32::from_le_bytes(e.try_into().expect("fixed width")))
                            .try_fold(None, |previous, value| {
                                if previous.is_some_and(|p| p >= value) {
                                    Err(())
                                } else {
                                    Ok(Some(value))
                                }
                            })
                            .is_err()
                    {
                        return Err(bad());
                    }
                }
            }
            if get(row, 108) != NONE && get(row, 116) != NONE {
                let included = get(row, 108) as usize;
                let excluded = get(row, 116) as usize;
                let mut left = 0;
                let mut right = 0;
                while left < get(row, 112) as usize && right < get(row, 120) as usize {
                    let l = i32::from_le_bytes(
                        bytes[included + left * 4..included + left * 4 + 4]
                            .try_into()
                            .expect("fixed width"),
                    );
                    let r = i32::from_le_bytes(
                        bytes[excluded + right * 4..excluded + right * 4 + 4]
                            .try_into()
                            .expect("fixed width"),
                    );
                    match l.cmp(&r) {
                        std::cmp::Ordering::Equal => return Err(bad()),
                        std::cmp::Ordering::Less => left += 1,
                        std::cmp::Ordering::Greater => right += 1,
                    }
                }
            }
        }
        mark("rows");
        if next != bloom || next_program != program_count {
            return Err(bad());
        }
        match s.kind() {
            RulesetKind::Dynamic
                if count > MAX_NUMBER_OF_DYNAMIC_RULES
                    || unsafe_rules > MAX_NUMBER_OF_UNSAFE_DYNAMIC_RULES =>
            {
                return Err(bad())
            }
            RulesetKind::Session
                if count > MAX_NUMBER_OF_SESSION_RULES
                    || unsafe_rules > MAX_NUMBER_OF_UNSAFE_SESSION_RULES =>
            {
                return Err(bad())
            }
            _ => {}
        }
        let mut token_count = 0;
        let mut expected_bloom = [0u8; TOKEN_BLOOM_BYTES];
        let mut posting_bytes = 0;
        let mut coverage: Vec<_> = (0..count).map(|i| get(bytes, HEADER + i * ROW)).collect();
        coverage.sort_unstable();
        if coverage.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(bad());
        }
        coverage.fill(0);
        mark("identifiers");
        for (slot, frame) in bytes[table..table + slots * SLOT]
            .chunks_exact(SLOT)
            .enumerate()
        {
            let bucket: &[u8; SLOT] = frame.try_into().expect("fixed bucket row");
            let kind = get(bucket, 8);
            if kind == 4 {
                token_count += 1;
                let bit = get64(bucket, 0) as usize & (TOKEN_BLOOM_BYTES * 8 - 1);
                expected_bloom[bit / 8] |= 1 << (bit % 8);
            }
            if !(1..=5).contains(&kind) || get(bucket, 28) != 0 {
                return Err(bad());
            }
            let ids = s.range(get(bucket, 20), get(bucket, 24), 8, directory)?;
            if (get(bucket, 20) as usize) < table + slots * SLOT || ids.is_empty() {
                return Err(bad());
            }
            let first_id = get(ids, 0) as usize;
            if first_id >= count {
                return Err(bad());
            }
            let first_row = s.row(first_id);
            let domain_index = get(ids, 4);
            let hash = get64(bucket, 0);
            if (1..=3).contains(&kind) && domain_index != NONE {
                let field = match kind {
                    1 => 60,
                    2 => 76,
                    _ => 92,
                };
                if first_row.field(132) != kind
                    || first_row.field(field) == NONE
                    || domain_index >= first_row.field(field + 4)
                {
                    return Err(bad());
                }
                let item = first_row.field(field) as usize + domain_index as usize * 16;
                let domain: &[u8; 16] = bytes[item..item + 16]
                    .try_into()
                    .expect("fixed domain entry");
                if hash != scope_hash(get64(domain, 0), kind)
                    || get(bucket, 12) != get(domain, 8)
                    || get(bucket, 16) != get(domain, 12)
                {
                    return Err(bad());
                }
            } else {
                charge_string_work(&mut referenced_bytes, get(bucket, 16) as usize)?;
                let key = checked_string(get(bucket, 12), get(bucket, 16))?;
                if (kind == 4 && key.len() != 3)
                    || (kind == 5 && !key.is_empty())
                    || hash != hash_key(kind, key.as_bytes())
                {
                    return Err(bad());
                }
            }
            let prefix = hash as usize & (prefixes - 1);
            let first = get(bytes, prefix_directory + prefix * 4) as usize;
            let end = get(bytes, prefix_directory + (prefix + 1) * 4) as usize;
            if slot < first || slot >= end {
                return Err(bad());
            }
            if slot > first {
                let previous_at = table + (slot - 1) * SLOT;
                let previous_hash = get64(bytes, previous_at);
                let previous_kind = get(bytes, previous_at + 8);
                if (previous_hash, previous_kind) > (hash, kind) {
                    return Err(bad());
                }
                if (previous_hash, previous_kind) == (hash, kind) {
                    charge_string_work(
                        &mut referenced_bytes,
                        get(bytes, previous_at + 16) as usize,
                    )?;
                    let previous_key =
                        checked_string(get(bytes, previous_at + 12), get(bytes, previous_at + 16))?;
                    let key = checked_string(get(bucket, 12), get(bucket, 16))?;
                    if previous_key >= key || previous_key.eq_ignore_ascii_case(key) {
                        return Err(bad());
                    }
                }
            }
            posting_bytes += ids.len();
            if posting_bytes > directory - (table + slots * SLOT) {
                return Err(bad());
            }
            let mut previous = None;
            for entry in ids.chunks_exact(8) {
                let id = get(entry, 0);
                if id as usize >= count || previous.is_some_and(|p| p >= id) {
                    return Err(bad());
                }
                let row = s.row(id as usize);
                if row.field(132) != kind {
                    return Err(bad());
                }
                if row.field(140) != 0 {
                    if row.field(136) != get(bucket, 12) || row.field(140) != get(bucket, 16) {
                        return Err(bad());
                    }
                    if get(entry, 4) != NONE {
                        return Err(bad());
                    }
                } else if kind != 5 {
                    let off = match kind {
                        1 => 60,
                        2 => 76,
                        3 => 92,
                        _ => return Err(bad()),
                    };
                    let index = get(entry, 4);
                    if index >= row.field(off + 4) {
                        return Err(bad());
                    }
                    let item = row.field(off) as usize + index as usize * 16;
                    let domain: &[u8; 16] = bytes[item..item + 16]
                        .try_into()
                        .expect("fixed domain entry");
                    if get(domain, 8) != get(bucket, 12) || get(domain, 12) != get(bucket, 16) {
                        return Err(bad());
                    }
                } else if get(entry, 4) != NONE {
                    return Err(bad());
                }
                coverage[id as usize] += 1;
                previous = Some(id);
            }
        }
        if bytes[bloom..prefix_directory] != expected_bloom
            || posting_bytes != directory - (table + slots * SLOT)
        {
            return Err(bad());
        }
        for (id, &actual) in coverage.iter().enumerate() {
            let row = s.row(id);
            let expected = if row.field(140) == 0 && row.field(132) != 5 {
                let off = match row.field(132) {
                    1 => 60,
                    2 => 76,
                    3 => 92,
                    _ => return Err(bad()),
                };
                row.field(off + 4)
            } else {
                1
            };
            if actual != expected {
                return Err(bad());
            }
        }
        mark("slots");
        let mut programs = Vec::with_capacity(program_count);
        let mut dfa_bytes = 0usize;
        for i in 0..program_count {
            let at = directory + i * 12;
            let off = get(bytes, at);
            let len = get(bytes, at + 4);
            let rule = get(bytes, at + 8) as usize;
            if rule >= count || get(bytes, HEADER + rule * ROW + 52) as usize != i {
                return Err(bad());
            }
            if len == 0 {
                if off != 0 {
                    return Err(bad());
                }
                let row = s.row(rule);
                crate::parse::validate_regex_source(
                    row.regex_filter().ok_or_else(bad)?,
                    row.sensitive(),
                )
                .map_err(IndexError)?;
                programs.push(Program::Lazy(OnceLock::new()));
            } else {
                dfa_bytes = dfa_bytes.checked_add(len as usize).ok_or_else(bad)?;
                if dfa_bytes > MAX_SERIALIZED_DFA_BYTES_PER_RULESET {
                    return Err(bad());
                }
                s.range(off, len, 1, table)?;
                let data = &bytes[off as usize..off as usize + len as usize];
                let (dfa, used) = sparse::DFA::from_bytes(data).map_err(|_| bad())?;
                if used != data.len() || dfa.pattern_len() != 1 {
                    return Err(bad());
                }
                programs.push(Program::Borrowed(dfa));
            }
        }
        if programs
            .iter()
            .filter(|p| matches!(p, Program::Lazy(_)))
            .count()
            > MAX_LAZY_REGEX_RULES_PER_RULESET
        {
            return Err(bad());
        }
        mark("programs");
        Ok(Self {
            bytes: Cow::Borrowed(bytes),
            captures: (0..programs.len()).map(|_| OnceLock::new()).collect(),
            programs,
            pool: Some(pool),
            token_count,
            reference: OnceLock::new(),
        })
    }
    #[inline(always)]
    fn range(&self, off: u32, count: u32, width: usize, upper: usize) -> Result<&[u8], IndexError> {
        let start = off as usize;
        let length = (count as usize).checked_mul(width).ok_or_else(bad)?;
        let end = start.checked_add(length).ok_or_else(bad)?;
        if start < HEADER + self.rule_count() * ROW || end > upper {
            return Err(bad());
        }
        self.bytes.get(start..end).ok_or_else(bad)
    }
    #[inline(always)]
    fn string(&self, off: u32, len: u32) -> Result<&str, IndexError> {
        let start = get(&self.bytes, 44) as usize;
        let end = (off as usize).checked_add(len as usize).ok_or_else(bad)?;
        if let Some(pool) = self.pool {
            return pool.get(off as usize..end).ok_or_else(bad);
        }
        let data = self
            .bytes
            .get(start..)
            .and_then(|p| p.get(off as usize..end))
            .ok_or_else(bad)?;
        std::str::from_utf8(data).map_err(|_| bad())
    }
    pub fn ruleset_id(&self) -> &str {
        self.string(get(&self.bytes, 64), get(&self.bytes, 68))
            .expect("validated identity")
    }
    pub fn kind(&self) -> RulesetKind {
        match get(&self.bytes, 52) {
            0 => RulesetKind::Static,
            1 => RulesetKind::Dynamic,
            _ => RulesetKind::Session,
        }
    }
    pub fn rule_count(&self) -> usize {
        get(&self.bytes, 16) as usize
    }
    pub fn regex_nfa_bytes(&self) -> usize {
        get64(&self.bytes, 80) as usize
    }
    pub fn regex_count(&self) -> usize {
        self.programs.len()
    }
    pub fn regex_fallback_count(&self) -> usize {
        self.programs
            .iter()
            .filter(|p| matches!(p, Program::Lazy(_)))
            .count()
    }
    pub fn serialized_dfa_bytes(&self) -> usize {
        let at = get(&self.bytes, 36) as usize;
        (0..self.programs.len())
            .map(|i| get(&self.bytes, at + i * 12 + 4) as usize)
            .sum()
    }
    pub fn rules(&self) -> impl ExactSizeIterator<Item = Rule> + '_ {
        (0..self.rule_count()).map(|id| self.row(id).decode())
    }
    /// The checksum detects corruption; it does not authenticate the producer.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.to_vec()
    }
    fn row(&self, id: usize) -> RuleRef<'_> {
        let start = HEADER + id * ROW;
        RuleRef {
            index: self,
            row: self.bytes[start..start + ROW]
                .try_into()
                .expect("fixed rule row"),
        }
    }
    #[inline]
    fn lookup(&self, kind: u32, key: &[u8]) -> Option<usize> {
        let hash = hash_key(kind, key);
        if kind == 4 {
            let bit = hash as usize & (TOKEN_BLOOM_BYTES * 8 - 1);
            let bloom = get(&self.bytes, 72) as usize;
            if self.bytes[bloom + bit / 8] & (1 << (bit % 8)) == 0 {
                return None;
            }
        }
        self.lookup_hashed(kind, key, hash)
    }
    #[inline]
    fn lookup_hashed(&self, kind: u32, key: &[u8], hash: u64) -> Option<usize> {
        let prefixes = get(&self.bytes, 88) as usize;
        let prefix = hash as usize & (prefixes - 1);
        let directory = get(&self.bytes, 72) as usize + TOKEN_BLOOM_BYTES;
        let first = get(&self.bytes, directory + prefix * 4) as usize;
        let end = get(&self.bytes, directory + (prefix + 1) * 4) as usize;
        let table = get(&self.bytes, 28) as usize;
        for slot in first..end {
            let at = table + slot * SLOT;
            let bucket: &[u8; SLOT] = self.bytes[at..at + SLOT]
                .try_into()
                .expect("fixed bucket row");
            let candidate_hash = get64(bucket, 0);
            if candidate_hash > hash {
                break;
            }
            if candidate_hash == hash && get(bucket, 8) == kind {
                if kind == 4 {
                    return Some(at);
                }
                let text = self.string(get(bucket, 12), get(bucket, 16)).ok()?;
                if text.as_bytes().eq_ignore_ascii_case(key) {
                    return Some(at);
                }
            }
        }
        None
    }
    pub fn candidates<'index: 'request, 'request>(
        &'index self,
        request: &'request Request<'_>,
    ) -> impl Iterator<Item = RuleRef<'index>> + 'request {
        let top = if request.resource_type == ResourceType::MainFrame {
            Some(request.host)
        } else {
            request.top_host.or(request.initiator_host)
        };
        let domains = [
            (1, request.host),
            (2, request.initiator_host.unwrap_or("")),
            (3, top.unwrap_or("")),
        ]
        .into_iter()
        .flat_map(|(kind, host)| {
            std::iter::successors(Some(host), |s| s.split_once('.').map(|(_, suffix)| suffix))
                .map(move |suffix| (kind, suffix.as_bytes()))
        });
        let tokens = request
            .url
            .as_bytes()
            .windows(3)
            .take(if self.token_count == 0 { 0 } else { usize::MAX })
            .map(|gram| (4, gram));
        let probes = domains
            .chain(tokens)
            .chain(std::iter::once((5, b"".as_slice())));
        let mut seen = [usize::MAX; 64];
        let mut filled = 0;
        probes
            .filter_map(move |(kind, key)| {
                let slot = self.lookup(kind, key)?;
                if seen[..filled].contains(&slot) {
                    return None;
                }
                if filled < seen.len() {
                    seen[filled] = slot;
                    filled += 1;
                }
                let off = get(&self.bytes, slot + 20) as usize;
                let len = get(&self.bytes, slot + 24) as usize;
                Some(self.bytes[off..off + len * 8].chunks_exact(8))
            })
            .flatten()
            .map(|entry| self.row(get(entry, 0) as usize))
    }
}

#[derive(Clone, Copy)]
pub struct RuleRef<'a> {
    index: &'a CompiledRuleset<'a>,
    row: &'a [u8; ROW],
}
impl<'a> RuleRef<'a> {
    #[inline(always)]
    fn field(self, off: usize) -> u32 {
        get(self.row, off)
    }
    #[inline(always)]
    fn text(self, off: usize) -> &'a str {
        self.index
            .string(self.field(off), self.field(off + 4))
            .expect("validated string")
    }
    fn sensitive(self) -> bool {
        self.field(12) & 1 != 0
    }
    pub fn regex_filter(self) -> Option<&'a str> {
        (self.field(28) != 0).then(|| self.text(24))
    }
    fn capture_regex(self) -> Option<&'a Regex> {
        let pattern = self.regex_filter()?;
        let id = self.field(52) as usize;
        let cache = match &self.index.programs[id] {
            Program::Lazy(cache) => cache,
            _ => &self.index.captures[id],
        };
        cache
            .get_or_init(|| match &self.index.programs[id] {
                Program::Lazy(_) => crate::parse::compile_regex(pattern, self.sensitive()),
                _ => crate::parse::compile_capture_regex(pattern, self.sensitive()),
            })
            .as_ref()
            .ok()
    }
    fn validation_rule(self) -> Rule {
        let mut rule = self.decision_rule();
        let off = self.field(44) as usize;
        let mut reader = Reader::new(&self.index.bytes[off..off + self.field(48) as usize]);
        Action::validate(&mut reader).expect("validated action");
        rule.condition.response_headers =
            Option::<Vec<HeaderCondition>>::read(&mut reader).expect("validated headers");
        rule.condition.excluded_response_headers =
            Option::<Vec<HeaderCondition>>::read(&mut reader).expect("validated headers");
        rule
    }
    fn decision_rule(self) -> Rule {
        let start = self.field(44) as usize;
        let action =
            Action::read(&mut Reader::new(&self.index.bytes[start..])).expect("validated action");
        let regex_filter=matches!(&action,Action::Redirect { redirect } if redirect.regex_substitution.is_some()).then(||self.regex_filter().unwrap_or("").to_owned());
        Rule {
            id: self.field(0),
            priority: self.field(4),
            action,
            condition: Condition {
                regex_filter,
                is_url_filter_case_sensitive: self.sensitive(),
                response_headers: (self.field(12) & 2 != 0).then(Vec::new),
                ..Condition::default()
            },
        }
    }
    pub fn decode(self) -> Rule {
        let off = self.field(44) as usize;
        let mut reader = Reader::new(&self.index.bytes[off..off + self.field(48) as usize]);
        let action = Action::read(&mut reader).expect("validated action");
        let response_headers =
            Option::<Vec<HeaderCondition>>::read(&mut reader).expect("validated headers");
        let excluded_response_headers =
            Option::<Vec<HeaderCondition>>::read(&mut reader).expect("validated headers");
        let list = |at| {
            (self.field(at) != NONE).then(|| {
                let start = self.field(at) as usize;
                let count = self.field(at + 4) as usize;
                self.index.bytes[start..start + count * 16]
                    .chunks_exact(16)
                    .map(|entry| {
                        self.index
                            .string(get(entry, 8), get(entry, 12))
                            .expect("validated domain")
                            .to_owned()
                    })
                    .collect()
            })
        };
        let tabs = |at| {
            (self.field(at) != NONE).then(|| {
                let start = self.field(at) as usize;
                let count = self.field(at + 4) as usize;
                self.index.bytes[start..start + count * 4]
                    .chunks_exact(4)
                    .map(|entry| i32::from_le_bytes(entry.try_into().expect("fixed width")))
                    .collect()
            })
        };
        let types = [
            ResourceType::MainFrame,
            ResourceType::SubFrame,
            ResourceType::Stylesheet,
            ResourceType::Script,
            ResourceType::Image,
            ResourceType::Font,
            ResourceType::Object,
            ResourceType::Xmlhttprequest,
            ResourceType::Ping,
            ResourceType::CspReport,
            ResourceType::Media,
            ResourceType::Websocket,
            ResourceType::Webtransport,
            ResourceType::Webbundle,
            ResourceType::Other,
        ];
        let methods = [
            RequestMethod::Connect,
            RequestMethod::Delete,
            RequestMethod::Get,
            RequestMethod::Head,
            RequestMethod::Options,
            RequestMethod::Patch,
            RequestMethod::Post,
            RequestMethod::Put,
            RequestMethod::Other,
        ];
        Rule {
            id: self.field(0),
            priority: self.field(4),
            action,
            condition: Condition {
                url_filter: (self.field(20) != 0).then(|| self.text(16).to_owned()),
                regex_filter: self.regex_filter().map(str::to_owned),
                is_url_filter_case_sensitive: self.sensitive(),
                request_domains: list(60),
                excluded_request_domains: list(68),
                initiator_domains: list(76),
                excluded_initiator_domains: list(84),
                top_domains: list(92),
                excluded_top_domains: list(100),
                resource_types: Some(
                    types
                        .into_iter()
                        .filter(|t| self.field(32) & (1 << (*t as u32)) != 0)
                        .collect(),
                ),
                request_methods: (self.field(12) & 4 != 0).then(|| {
                    methods
                        .into_iter()
                        .filter(|m| self.field(36) & (1 << (*m as u32)) != 0)
                        .collect()
                }),
                excluded_request_methods: (self.field(36) >> 9 != 0).then(|| {
                    methods
                        .into_iter()
                        .filter(|m| self.field(36) & (1 << (9 + *m as u32)) != 0)
                        .collect()
                }),
                domain_type: match self.field(40) {
                    0 => None,
                    1 => Some(DomainType::FirstParty),
                    _ => Some(DomainType::ThirdParty),
                },
                tab_ids: tabs(108),
                excluded_tab_ids: tabs(116),
                response_headers,
                excluded_response_headers,
                ..Condition::default()
            },
        }
    }
    fn domain_contains(self, off: usize, host: Option<&str>) -> bool {
        let Some(host) = host else {
            return false;
        };
        let start = self.field(off) as usize;
        let count = self.field(off + 4) as usize;
        let entries = &self.index.bytes[start..start + count * 16];
        std::iter::successors(Some(host), |s| s.split_once('.').map(|(_, suffix)| suffix)).any(
            |suffix| {
                let hash = hash_key(0, suffix.as_bytes());
                let mut lo = 0;
                let mut hi = count;
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if get64(domain_entry(entries, mid), 0) < hash {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                while lo < count && get64(domain_entry(entries, lo), 0) == hash {
                    let entry = domain_entry(entries, lo);
                    if self
                        .index
                        .string(get(entry, 8), get(entry, 12))
                        .is_ok_and(|s| s.eq_ignore_ascii_case(suffix))
                    {
                        return true;
                    }
                    lo += 1;
                }
                false
            },
        )
    }
    fn domain_scope(self, off: usize, host: Option<&str>) -> bool {
        (self.field(off + 8) == NONE || !self.domain_contains(off + 8, host))
            && (self.field(off) == NONE || self.domain_contains(off, host))
    }
    pub fn matches(self, request: &Request<'_>) -> bool {
        if self.field(32) & (1 << (request.resource_type as u32)) == 0 {
            return false;
        }
        let http = request.url.starts_with("http://") || request.url.starts_with("https://");
        let methods = self.field(36);
        if (self.field(12) & 4 != 0 && (!http || methods & (1 << (request.method as u32)) == 0))
            || (http && methods & (1 << (9 + request.method as u32)) != 0)
        {
            return false;
        }
        if self.field(40) != 0
            && self.field(40)
                != match request.domain_type {
                    DomainType::FirstParty => 1,
                    DomainType::ThirdParty => 2,
                }
        {
            return false;
        }
        for (off, exclude) in [(108, false), (116, true)] {
            if self.field(off) != NONE {
                let start = self.field(off) as usize;
                let count = self.field(off + 4) as usize;
                let found = self.index.bytes[start..start + count * 4]
                    .chunks_exact(4)
                    .any(|e| {
                        i32::from_le_bytes(e.try_into().expect("fixed width")) == request.tab_id
                    });
                if found == exclude {
                    return false;
                }
            }
        }
        if self.field(128) != 0 {
            let start = self.field(124) as usize;
            let length = self.field(128) as usize;
            if !crate::matcher::contains_literal(
                request.url.as_bytes(),
                &self.text(16).as_bytes()[start..start + length],
                self.sensitive(),
            ) {
                return false;
            }
        }
        let top = if request.resource_type == ResourceType::MainFrame {
            Some(request.host)
        } else {
            request.top_host.or(request.initiator_host)
        };
        if !self.domain_scope(60, Some(request.host))
            || !self.domain_scope(76, request.initiator_host)
            || !self.domain_scope(92, top)
        {
            return false;
        }
        if self.field(12) & 2 != 0 {
            if request.stage != Stage::ResponseHeaders {
                return false;
            }
            // Header conditions are uncommon and run only after a response exists.
            return PreparedRule::new(self.decode()).is_ok_and(|r| r.matches(request));
        }
        if self.field(20) != 0
            && !crate::matcher::url_filter_matches(self.text(16), request.url, self.sensitive())
        {
            return false;
        }
        self.regex_filter().is_none_or(|pattern| {
            self.index.programs[self.field(52) as usize].matches(
                pattern,
                self.sensitive(),
                request.url,
            )
        })
    }
}

fn action_tag(action: &Action) -> u32 {
    match action {
        Action::Block => 0,
        Action::Allow => 1,
        Action::AllowAllRequests => 2,
        Action::UpgradeScheme => 3,
        Action::Redirect { .. } => 4,
        Action::ModifyHeaders { .. } => 5,
    }
}
#[inline(always)]
fn domain_entry(bytes: &[u8], index: usize) -> &[u8; 16] {
    let at = index * 16;
    bytes[at..at + 16].try_into().expect("fixed domain entry")
}
#[inline(always)]
fn get(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(bytes[off..off + 4].try_into().expect("fixed width"))
}
#[inline(always)]
fn get64(bytes: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(bytes[off..off + 8].try_into().expect("fixed width"))
}
fn put(bytes: &mut [u8], off: usize, value: u32) {
    bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], off: usize, value: u64) {
    bytes[off..off + 8].copy_from_slice(&value.to_le_bytes());
}
fn checksum(bytes: &[u8]) -> u64 {
    u64::from(crc32fast::hash(bytes))
}
#[inline]
fn hash_key(kind: u32, key: &[u8]) -> u64 {
    if kind == 4 && key.len() == 3 {
        let code = u64::from(key[0].to_ascii_lowercase())
            | (u64::from(key[1].to_ascii_lowercase()) << 8)
            | (u64::from(key[2].to_ascii_lowercase()) << 16);
        // Both operations are bijective: equal token hashes imply equal trigrams.
        let mixed = code.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        return mixed ^ (mixed >> 32);
    }
    scope_hash(domain_hash(key), kind)
}
#[inline(always)]
fn normalized_ascii_word(word: u64) -> bool {
    const HIGH: u64 = 0x8080_8080_8080_8080;
    word & HIGH == 0
        && (word.wrapping_add(0x3f3f_3f3f_3f3f_3f3f)
            & !word.wrapping_add(0x2525_2525_2525_2525)
            & HIGH)
            == 0
}

#[inline(always)]
fn domain_hash_checked<const NORMALIZED: bool>(key: &[u8]) -> Option<u64> {
    let mut hash =
        0xa076_1d64_78bd_642fu64 ^ (key.len() as u64).wrapping_mul(0xe703_7ed1_a0b4_28db);
    let mut chunks = key.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(chunk.try_into().expect("fixed width"));
        if NORMALIZED && !normalized_ascii_word(word) {
            return None;
        }
        hash = (hash ^ (word | 0x2020_2020_2020_2020).wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .rotate_left(27)
            .wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    }
    let remaining = chunks.remainder();
    let tail = if remaining.is_empty() {
        0
    } else if key.len() >= 8 {
        let word = u64::from_le_bytes(key[key.len() - 8..].try_into().expect("fixed width"));
        if NORMALIZED && !normalized_ascii_word(word) {
            return None;
        }
        word | 0x2020_2020_2020_2020
    } else {
        let mut word = 0;
        for (i, &byte) in remaining.iter().enumerate() {
            if NORMALIZED && (!byte.is_ascii() || byte.is_ascii_uppercase()) {
                return None;
            }
            word |= u64::from(byte | 0x20) << (i * 8);
        }
        word
    };
    hash ^= tail.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    hash ^= hash >> 32;
    hash = hash.wrapping_mul(0xd6e8_feb8_6659_fd93);
    hash ^= hash >> 32;
    hash = hash.wrapping_mul(0xd6e8_feb8_6659_fd93);
    Some(hash ^ (hash >> 32))
}

#[inline(always)]
fn domain_hash(key: &[u8]) -> u64 {
    domain_hash_checked::<false>(key).expect("unchecked hash always succeeds")
}

#[inline(always)]
fn normalized_domain_hash(text: &str) -> Option<u64> {
    domain_hash_checked::<true>(text.as_bytes())
}
#[inline(always)]
fn scope_hash(base: u64, kind: u32) -> u64 {
    base ^ u64::from(kind)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .rotate_left(17)
}
fn grams(filter: &str) -> impl Iterator<Item = &str> {
    filter
        .split(['*', '^', '|'])
        .flat_map(|s| s.as_bytes().windows(3))
        .filter_map(|s| std::str::from_utf8(s).ok())
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
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}
fn bucket_rules(rules: &[Rule]) -> BTreeMap<Bucket, Vec<usize>> {
    // Sketch collisions affect token choice; every selected token stays mandatory.
    let mut frequency = vec![0u32; 1 << 18];
    for rule in rules {
        if let Some(filter) = &rule.condition.url_filter {
            for gram in grams(filter) {
                let slot = hash_key(4, gram.as_bytes()) as usize & (frequency.len() - 1);
                frequency[slot] = frequency[slot].saturating_add(1);
            }
        }
    }
    let mut buckets = BTreeMap::<Bucket, Vec<usize>>::new();
    for (id, rule) in rules.iter().enumerate() {
        let c = &rule.condition;
        let scope = c
            .request_domains
            .as_ref()
            .map(|d| (0, d))
            .or_else(|| {
                c.initiator_domains
                    .as_ref()
                    .or(c.domains.as_ref())
                    .map(|d| (1, d))
            })
            .or_else(|| c.top_domains.as_ref().map(|d| (2, d)));
        let token = c.url_filter.as_deref().and_then(|filter| {
            grams(filter).min_by_key(|gram| {
                let slot = hash_key(4, gram.as_bytes()) as usize & (frequency.len() - 1);
                (frequency[slot], *gram)
            })
        });
        let prefer_token = scope.is_some_and(|(scope, _)| scope != 0)
            && token.is_some_and(|gram| {
                let slot = hash_key(4, gram.as_bytes()) as usize & (frequency.len() - 1);
                frequency[slot] <= 8
            });
        if let Some((scope, domains)) = scope.filter(|_| !prefer_token) {
            let mut domains: Vec<_> = domains.iter().map(|s| s.to_ascii_lowercase()).collect();
            domains.sort();
            domains.dedup();
            for d in domains {
                let key = match scope {
                    0 => Bucket::Domain(d),
                    1 => Bucket::Initiator(d),
                    _ => Bucket::Top(d),
                };
                buckets.entry(key).or_default().push(id);
            }
            continue;
        }
        let bucket = if prefer_token {
            Bucket::Token(token.expect("selected rare token").to_ascii_lowercase())
        } else if let Some(domain) = domain_key(rule) {
            Bucket::Domain(domain)
        } else if let Some(gram) = token {
            Bucket::Token(gram.to_ascii_lowercase())
        } else {
            Bucket::Fallback
        };
        buckets.entry(bucket).or_default().push(id);
    }
    buckets
}

pub struct Extension<'a> {
    pub id: &'a str,
    pub install_order: u64,
    pub rulesets: &'a [&'a CompiledRuleset<'a>],
}

pub fn evaluate<'a>(request: &Request<'_>, extensions: &'a [Extension<'a>]) -> Decision<'a> {
    evaluate_filtered(
        request,
        extensions.iter().flat_map(|extension| {
            extension
                .rulesets
                .iter()
                .map(move |set| (extension.id, extension.install_order, *set))
        }),
        |_, _| true,
    )
}

/// Evaluate mapped indexes after the native host's permission admission. The
/// iterator avoids building a second extension/ruleset graph per request.
pub fn evaluate_filtered<'a>(
    request: &Request<'_>,
    rulesets: impl Iterator<Item = (&'a str, u64, &'a CompiledRuleset<'a>)>,
    admitted: impl Fn(&str, &Rule) -> bool,
) -> Decision<'a> {
    let admitted = &admitted;
    let matching: Vec<_> = rulesets
        .flat_map(|(id, order, set)| {
            set.candidates(request)
                .filter(|rule| rule.matches(request))
                .filter_map(move |reference| {
                    let rule = reference.decision_rule();
                    if !admitted(id, &rule) {
                        return None;
                    }
                    let regex = rule
                        .condition
                        .regex_filter
                        .as_ref()
                        .and_then(|_| reference.capture_regex());
                    Some((id, order, set.ruleset_id(), rule, regex))
                })
        })
        .collect();
    evaluate_matching_candidates(
        request,
        matching
            .iter()
            .map(|(id, order, set, rule, regex)| MatchingCandidate {
                extension_id: id,
                install_order: *order,
                ruleset_id: set,
                rule,
                regex: *regex,
            }),
    )
}
pub fn evaluate_linear<'a>(request: &Request<'_>, extensions: &'a [Extension<'a>]) -> Decision<'a> {
    evaluate_matching_candidates(
        request,
        extensions.iter().flat_map(|ext| {
            ext.rulesets.iter().flat_map(move |set| {
                set.reference
                    .get_or_init(|| {
                        set.rules()
                            .filter_map(|rule| PreparedRule::new(rule).ok())
                            .collect()
                    })
                    .iter()
                    .filter(|rule| rule.matches(request))
                    .map(move |rule| MatchingCandidate {
                        extension_id: ext.id,
                        install_order: ext.install_order,
                        ruleset_id: set.ruleset_id(),
                        rule: rule.rule(),
                        regex: rule.compiled_regex(),
                    })
            })
        }),
    )
}

pub fn validate_extension_rulesets(
    rulesets: &[&CompiledRuleset<'_>],
    static_rule_allocation: usize,
) -> Result<(), IndexError> {
    let mut identifiers = std::collections::HashSet::new();
    let mut counts = [0usize; 3];
    let mut regex_counts = [0usize; 3];
    let mut static_rules = 0;
    let mut bytes = 0usize;
    let mut nfa_bytes = 0usize;
    let mut lazy = 0usize;
    for set in rulesets {
        if !identifiers.insert(set.ruleset_id()) {
            return Err(IndexError("duplicate ruleset id".into()));
        }
        let kind = match set.kind() {
            RulesetKind::Static => 0,
            RulesetKind::Dynamic => 1,
            RulesetKind::Session => 2,
        };
        counts[kind] += 1;
        regex_counts[kind] += set.regex_count();
        nfa_bytes = nfa_bytes
            .checked_add(set.regex_nfa_bytes())
            .ok_or_else(bad)?;
        lazy += set.regex_fallback_count();
        bytes = bytes.checked_add(set.bytes.len()).ok_or_else(bad)?;
        if kind == 0 {
            static_rules += set.rule_count();
        }
    }
    if counts[0] > MAX_NUMBER_OF_ENABLED_STATIC_RULESETS || counts[1] > 1 || counts[2] > 1 {
        return Err(IndexError("enabled ruleset limit exceeded".into()));
    }
    if regex_counts.iter().any(|&n| n > MAX_NUMBER_OF_REGEX_RULES) {
        return Err(IndexError("aggregate regex rule limit exceeded".into()));
    }
    if static_rules > static_rule_allocation {
        return Err(IndexError("static rule allocation exceeded".into()));
    }
    if nfa_bytes > MAX_REGEX_NFA_BYTES_PER_EXTENSION || lazy > MAX_LAZY_REGEX_RULES_PER_EXTENSION {
        return Err(IndexError("extension regex memory budget exceeded".into()));
    }
    if bytes > MAX_INDEX_BYTES {
        return Err(IndexError("extension index memory budget exceeded".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Condition, ResourceType};

    fn fixture() -> CompiledRuleset<'static> {
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
        bytes[56..64].copy_from_slice(&seal.to_le_bytes());
    }

    #[test]
    fn hostile_offsets_lengths_tags_and_postings_are_rejected() {
        let original = fixture().to_bytes();
        for position in [
            8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 64, 68, 80, 84,
        ] {
            let mut changed = original.clone();
            put(&mut changed, position, u32::MAX);
            assert!(CompiledRuleset::load(&changed).is_err(), "{position}");
        }
        let table = get(&original, 28) as usize;
        for position in [
            HEADER,
            HEADER + 4,
            HEADER + 8,
            HEADER + 12,
            HEADER + 16,
            HEADER + 20,
            HEADER + 24,
            HEADER + 28,
            HEADER + 44,
            HEADER + 48,
            HEADER + 52,
            HEADER + 60,
            HEADER + 64,
            HEADER + 108,
            HEADER + 112,
            HEADER + 124,
            HEADER + 128,
            HEADER + 132,
            HEADER + 136,
            HEADER + 140,
        ] {
            let mut changed = original.clone();
            let value = if get(&changed, position) == u32::MAX {
                0
            } else {
                u32::MAX
            };
            put(&mut changed, position, value);
            reseal(&mut changed);
            assert!(CompiledRuleset::load(&changed).is_err(), "{position}");
        }
        let occupied = (0..get(&original, 32) as usize)
            .map(|slot| table + slot * SLOT)
            .find(|&at| get(&original, at + 8) != 0)
            .unwrap();
        for field in [0, 8, 12, 16, 20, 24, 28] {
            let mut changed = original.clone();
            put(&mut changed, occupied + field, u32::MAX);
            reseal(&mut changed);
            assert!(CompiledRuleset::load(&changed).is_err(), "slot {field}");
        }
        let record = get(&original, HEADER + 44) as usize;
        let mut changed = original.clone();
        changed[record] = 255;
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

#[cfg(test)]
mod acceleration_tests {
    use super::*;
    use crate::{Action, Condition, ResourceType};

    #[test]
    fn domain_block_hash_folds_case_and_exact_checks_punctuation_collisions() {
        for domain in ["a", "Mixed.Example.Test", "long-subdomain.example.test"] {
            assert_eq!(
                hash_key(1, domain.as_bytes()),
                hash_key(1, domain.to_ascii_uppercase().as_bytes())
            );
        }
        assert_eq!(hash_key(1, b"alias[.test"), hash_key(1, b"alias{.test"));
        let rules = ["alias[.test", "alias{.test"]
            .into_iter()
            .enumerate()
            .map(|(id, domain)| Rule {
                id: id as u32 + 1,
                priority: 1,
                action: Action::Block,
                condition: Condition {
                    request_domains: Some(vec![domain.to_owned()]),
                    ..Condition::default()
                },
            })
            .collect();
        let compiled = CompiledRuleset::compile("collision", RulesetKind::Static, rules).unwrap();
        let bytes = compiled.to_bytes();
        let loaded = CompiledRuleset::load(&bytes).unwrap();
        let rulesets = [&loaded];
        let extensions = [Extension {
            id: "e",
            install_order: 1,
            rulesets: &rulesets,
        }];
        for (host, id) in [("alias[.test", 1), ("ALIAS{.TEST", 2)] {
            let request = Request::new("https://example.test/", host, ResourceType::Script);
            let result = evaluate(&request, &extensions);
            assert_eq!(result, evaluate_linear(&request, &extensions));
            assert_eq!(result.matched_rules[0].rule_id, id);
        }
    }

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
        let bytes = compiled.to_bytes();
        let loaded = CompiledRuleset::load(&bytes).unwrap();
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

#[cfg(test)]
mod domain_word_tests {
    use super::*;

    #[test]
    fn word_normalization_agrees_with_scalar_validation() {
        for position in 0..8 {
            for byte in 0..=u8::MAX {
                let mut bytes = [b'a'; 8];
                bytes[position] = byte;
                assert_eq!(
                    normalized_ascii_word(u64::from_le_bytes(bytes)),
                    bytes
                        .iter()
                        .all(|b| b.is_ascii() && !b.is_ascii_uppercase()),
                    "position {position}, byte {byte}"
                );
            }
        }
        for length in 1..=64 {
            let mut bytes = vec![b'a'; length];
            for position in 0..length {
                for byte in [0, b'@', b'A', b'Z', b'[', b'z', 127, 128, 255] {
                    bytes[position] = byte;
                    let normalized = domain_hash_checked::<true>(&bytes);
                    let valid = bytes
                        .iter()
                        .all(|b| b.is_ascii() && !b.is_ascii_uppercase());
                    assert_eq!(normalized.is_some(), valid);
                    if valid {
                        assert_eq!(normalized, Some(domain_hash(&bytes)));
                    }
                    let lower: Vec<_> = bytes.iter().map(u8::to_ascii_lowercase).collect();
                    assert_eq!(domain_hash(&bytes), domain_hash(&lower));
                }
                bytes[position] = b'a';
            }
        }
    }
}
