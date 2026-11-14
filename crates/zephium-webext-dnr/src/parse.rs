//! Validation is available in renderers; JSON support is compile-side only.

use std::collections::HashSet;
use std::fmt;

use regex::bytes::{Regex, RegexBuilder};

use crate::model::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    InvalidJson(String),
    InvalidRuleset(String),
    Rejected(Vec<RuleRejection>),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(reason) | Self::InvalidRuleset(reason) => f.write_str(reason),
            Self::Rejected(rules) => write!(f, "{} invalid rules; update rejected", rules.len()),
        }
    }
}

impl std::error::Error for ParseError {}

/// ASCII URL matching avoids Rust's Unicode `\w`/case-folding semantics.
pub fn compile_regex(pattern: &str, case_sensitive: bool) -> Result<Regex, String> {
    if pattern.is_empty() {
        return Err("regexFilter must not be empty".into());
    }
    if !pattern.is_ascii() {
        return Err("regexFilter must contain only ASCII characters".into());
    }
    let pattern = normalize_regex(pattern)?;
    RegexBuilder::new(&pattern)
        .unicode(false)
        .case_insensitive(!case_sensitive)
        .size_limit(MAX_REGEX_MEMORY_BYTES)
        .dfa_size_limit(MAX_REGEX_MEMORY_BYTES * 16)
        .build()
        .map_err(|error| match error {
            regex::Error::CompiledTooBig(_) => {
                "regexFilter exceeds the compiled-size limit (RE2 size approximation)".into()
            }
            _ => "regexFilter has invalid or unsupported RE2 syntax".into(),
        })
}

/// Translate RE2 escapes and refuse Rust extensions whose interpretation differs.
/// The caller must use this same normalization for a regex-set prefilter.
pub fn normalize_regex(pattern: &str) -> Result<String, String> {
    if pattern.len() > MAX_REGEX_SOURCE_BYTES {
        return Err("regexFilter exceeds the implementation source-size limit".into());
    }
    if !pattern.is_ascii() {
        return Err("regexFilter must contain only ASCII characters".into());
    }
    let bytes = pattern.as_bytes();
    let mut output = String::with_capacity(pattern.len());
    let mut position = 0;
    let mut repeated = false;
    let mut lazy = false;
    while position < bytes.len() {
        match bytes[position] {
            b'\\' => {
                let (escape, _) = normalize_escape(pattern, &mut position, false)?;
                output.push_str(&escape);
                repeated = false;
            }
            b'[' => {
                output.push_str(&normalize_class(pattern, &mut position)?);
                repeated = false;
            }
            b'(' if bytes.get(position + 1) == Some(&b'?') => {
                let suffix = &pattern[position + 2..];
                if suffix.starts_with(':') || suffix.starts_with('P') || suffix.starts_with('<') {
                    output.push('(');
                    position += 1;
                } else {
                    let end = suffix.find([':', ')']).ok_or("invalid RE2 flags")?;
                    if !suffix[..end].bytes().all(|flag| b"imsU-".contains(&flag)) {
                        return Err("Rust-only or unsupported RE2 inline flags".into());
                    }
                    output.push_str(&pattern[position..position + 3 + end]);
                    position += 3 + end;
                }
                repeated = false;
            }
            b'*' | b'+' | b'?' => {
                let character = bytes[position];
                if (character != b'?' || lazy) && repeated {
                    return Err("RE2 does not support repeated repetition operators".into());
                }
                lazy = repeated;
                repeated = true;
                output.push(char::from(character));
                position += 1;
            }
            b'{' => {
                let suffix = &pattern[position + 1..];
                if let Some(end) = suffix.find('}') {
                    let counts = &suffix[..end];
                    let parts: Vec<_> = counts.split(',').collect();
                    if parts.len() <= 2
                        && !parts[0].is_empty()
                        && parts
                            .iter()
                            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
                    {
                        if repeated
                            || parts
                                .iter()
                                .filter(|part| !part.is_empty())
                                .any(|part| part.parse::<u32>().map_or(true, |count| count > 1_000))
                        {
                            return Err("RE2 repetition counts must not exceed 1000 or repeat another operator".into());
                        }
                        output.push_str(&pattern[position..position + end + 2]);
                        position += end + 2;
                        repeated = true;
                        lazy = false;
                        continue;
                    }
                }
                output.push('{');
                position += 1;
                repeated = false;
            }
            character => {
                output.push(char::from(character));
                position += 1;
                repeated = false;
            }
        }
    }
    Ok(output)
}

fn normalize_class(pattern: &str, position: &mut usize) -> Result<String, String> {
    let bytes = pattern.as_bytes();
    let mut output = String::from("[");
    let mut unicode = false;
    *position += 1;
    if bytes.get(*position) == Some(&b'^') {
        output.push('^');
        *position += 1;
    }
    if bytes.get(*position) == Some(&b']') {
        output.push_str(r"\]");
        *position += 1;
    }
    while *position < bytes.len() {
        match bytes[*position] {
            b']' => {
                output.push(']');
                *position += 1;
                // Requests are canonical ASCII; intersect before NFA expansion.
                return Ok(if unicode {
                    format!(r"(?u:[{output}&&\x00-\x7F])")
                } else {
                    output
                });
            }
            b'\\' => {
                let (escape, requires_unicode) = normalize_escape(pattern, position, true)?;
                unicode |= requires_unicode;
                output.push_str(&escape);
            }
            b'[' if bytes.get(*position + 1) == Some(&b':') => {
                let end = pattern[*position + 2..]
                    .find(":]")
                    .ok_or("invalid POSIX character class")?;
                let end = *position + end + 4;
                output.push_str(&pattern[*position..end]);
                *position = end;
            }
            b'[' => return Err("nested character classes are unsupported RE2 syntax".into()),
            character
                if matches!(character, b'&' | b'~' | b'-')
                    && bytes.get(*position + 1) == Some(&character) =>
            {
                return Err("character-class set operators are unsupported RE2 syntax".into());
            }
            character => {
                output.push(char::from(character));
                *position += 1;
            }
        }
    }
    Err("unterminated regex character class".into())
}

fn normalize_escape(
    pattern: &str,
    position: &mut usize,
    in_class: bool,
) -> Result<(String, bool), String> {
    let bytes = pattern.as_bytes();
    let start = *position;
    let character = *bytes.get(start + 1).ok_or("trailing regex escape")?;
    *position += 2;
    let unicode_escape = |escape: String| {
        if in_class {
            (escape, true)
        } else {
            (format!("(?u:{escape})"), false)
        }
    };
    match character {
        b'Q' if !in_class => {
            let quoted = &pattern[*position..];
            let end = quoted.find(r"\E").unwrap_or(quoted.len());
            let literal = regex::escape(&quoted[..end]);
            *position += end + usize::from(end < quoted.len()) * 2;
            Ok((literal, false))
        }
        b'C' if !in_class => Ok(("(?s:.)".into(), false)),
        b'Q' | b'C' | b'u' | b'U' => Err("invalid or unsupported RE2 escape".into()),
        b'b' | b'B' | b'A' | b'z' if in_class => {
            Err("invalid RE2 escape in character class".into())
        }
        b'b' | b'B' | b'A' | b'z' => Ok((format!("(?:\\{})", char::from(character)), false)),
        b'0'..=b'7' => {
            let mut code = u32::from(character - b'0');
            let mut digits = 1;
            while digits < 3 {
                let Some(digit @ b'0'..=b'7') = bytes.get(*position) else {
                    break;
                };
                code = code * 8 + u32::from(digit - b'0');
                *position += 1;
                digits += 1;
            }
            if digits == 1 && character != b'0' {
                return Err("RE2 does not support backreferences".into());
            }
            let escape = format!(r"\x{{{code:X}}}");
            Ok(if code > 127 {
                unicode_escape(escape)
            } else {
                (escape, false)
            })
        }
        b'p' | b'P' => {
            if bytes.get(*position) == Some(&b'{') {
                let end = pattern[*position + 1..]
                    .find('}')
                    .ok_or("invalid Unicode class")?;
                let property = &pattern[*position + 1..*position + end + 1];
                if property.is_empty()
                    || !property
                        .bytes()
                        .all(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                {
                    return Err("unsupported RE2 Unicode class name".into());
                }
                *position += end + 2;
            } else {
                if !bytes.get(*position).is_some_and(u8::is_ascii_alphabetic) {
                    return Err("invalid Unicode class".into());
                }
                *position += 1;
            }
            let escape = &pattern[start..*position];
            Ok(if in_class {
                (escape.to_owned(), true)
            } else {
                (format!(r"(?u:[{escape}&&\x00-\x7F])"), false)
            })
        }
        b'x' => {
            let digits = if bytes.get(*position) == Some(&b'{') {
                let end = pattern[*position + 1..]
                    .find('}')
                    .ok_or("invalid hex escape")?;
                let digits = &pattern[*position + 1..*position + end + 1];
                *position += end + 2;
                digits
            } else {
                let digits = pattern
                    .get(*position..*position + 2)
                    .ok_or("invalid hex escape")?;
                *position += 2;
                digits
            };
            let code = u32::from_str_radix(digits, 16).map_err(|_| "invalid hex escape")?;
            let escape = pattern[start..*position].to_owned();
            Ok(if code > 127 {
                unicode_escape(escape)
            } else {
                (escape, false)
            })
        }
        b'w' if in_class => Ok(("0-9A-Za-z_".into(), false)),
        b'W' if in_class => Ok(("[^0-9A-Za-z_]".into(), false)),
        b'd' if in_class => Ok(("0-9".into(), false)),
        b'D' if in_class => Ok(("[^0-9]".into(), false)),
        b's' => Ok((
            if in_class {
                r"\t\n\f\r ".into()
            } else {
                r"[\t\n\f\r ]".into()
            },
            false,
        )),
        b'S' => Ok((r"[^\t\n\f\r ]".into(), false)),
        character if !character.is_ascii_alphanumeric() => {
            Ok((regex::escape(&char::from(character).to_string()), false))
        }
        _ => Ok((pattern[start..*position].to_owned(), false)),
    }
}

pub fn validate_rule(rule: &Rule, kind: RulesetKind) -> Result<(), String> {
    if rule.id == 0 || rule.id > i32::MAX as u32 {
        return Err("id must be an integer from 1 to 2147483647".into());
    }
    if rule.priority == 0 || rule.priority > i32::MAX as u32 {
        return Err("priority must be an integer from 1 to 2147483647".into());
    }
    let c = &rule.condition;
    if c.url_filter.is_some() && c.regex_filter.is_some() {
        return Err("urlFilter and regexFilter cannot both be specified".into());
    }
    if let Some(filter) = &c.url_filter {
        if filter.is_empty() || !filter.is_ascii() || filter.starts_with("||*") {
            return Err("urlFilter must be nonempty ASCII and cannot begin with ||*".into());
        }
    }
    if c.domains.is_some() && c.initiator_domains.is_some() {
        return Err("domains and initiatorDomains cannot both be specified".into());
    }
    if c.excluded_domains.is_some() && c.excluded_initiator_domains.is_some() {
        return Err("excludedDomains and excludedInitiatorDomains cannot both be specified".into());
    }
    for (name, values, allow_empty) in [
        ("domains", &c.domains, false),
        ("excludedDomains", &c.excluded_domains, true),
        ("initiatorDomains", &c.initiator_domains, false),
        (
            "excludedInitiatorDomains",
            &c.excluded_initiator_domains,
            true,
        ),
        ("requestDomains", &c.request_domains, false),
        ("excludedRequestDomains", &c.excluded_request_domains, true),
        ("topDomains", &c.top_domains, false),
        ("excludedTopDomains", &c.excluded_top_domains, true),
    ] {
        if let Some(values) = values {
            if !allow_empty && values.is_empty() {
                return Err(format!("{name} must not be empty"));
            }
            if values.iter().any(|value| !value.is_ascii()) {
                return Err(format!(
                    "{name} must contain only ASCII domains (use punycode)"
                ));
            }
        }
    }
    nonempty("resourceTypes", &c.resource_types)?;
    nonempty("requestMethods", &c.request_methods)?;
    nonempty("tabIds", &c.tab_ids)?;
    nonempty("responseHeaders", &c.response_headers)?;
    nonempty("excludedResponseHeaders", &c.excluded_response_headers)?;
    if c.resource_types.is_some() && c.excluded_resource_types.is_some() {
        return Err("resourceTypes and excludedResourceTypes cannot both be specified".into());
    }
    if c.request_methods.is_some() && c.excluded_request_methods.is_some() {
        return Err("requestMethods and excludedRequestMethods cannot both be specified".into());
    }
    if kind != RulesetKind::Session && (c.tab_ids.is_some() || c.excluded_tab_ids.is_some()) {
        return Err("tabIds and excludedTabIds are only valid for session rules".into());
    }
    if let (Some(included), Some(excluded)) = (&c.tab_ids, &c.excluded_tab_ids) {
        if included.iter().any(|id| excluded.contains(id)) {
            return Err("a tab ID cannot be both included and excluded".into());
        }
    }
    if matches!(rule.action, Action::AllowAllRequests)
        && !c.resource_types.as_ref().is_some_and(|types| {
            !types.is_empty()
                && types
                    .iter()
                    .all(|t| matches!(t, ResourceType::MainFrame | ResourceType::SubFrame))
        })
    {
        return Err(
            "allowAllRequests requires resourceTypes containing only main_frame/sub_frame".into(),
        );
    }
    validate_response_headers(c)?;
    let regex = c
        .regex_filter
        .as_deref()
        .map(|filter| compile_regex(filter, c.is_url_filter_case_sensitive))
        .transpose()?;
    match &rule.action {
        Action::Redirect { redirect } => validate_redirect(redirect, regex.as_ref())?,
        Action::ModifyHeaders {
            request_headers,
            response_headers,
        } => {
            if request_headers.is_empty() && response_headers.is_empty() {
                return Err("modifyHeaders requires requestHeaders or responseHeaders".into());
            }
            if c.has_response_header_condition() && !request_headers.is_empty() {
                return Err("response-header conditions cannot modify request headers".into());
            }
            validate_header_modifications(request_headers, true)?;
            validate_header_modifications(response_headers, false)?;
        }
        _ => {}
    }
    Ok(())
}

fn nonempty<T>(name: &str, values: &Option<Vec<T>>) -> Result<(), String> {
    if values.as_ref().is_some_and(Vec::is_empty) {
        return Err(format!("{name} must not be empty"));
    }
    Ok(())
}

fn validate_response_headers(condition: &Condition) -> Result<(), String> {
    if header_pattern_count(condition) > MAX_HEADER_PATTERNS_PER_RULE {
        return Err("MAX_HEADER_PATTERNS_PER_RULE exceeded".into());
    }
    for header in condition
        .response_headers
        .iter()
        .flatten()
        .chain(condition.excluded_response_headers.iter().flatten())
    {
        if !valid_header_name(&header.header) {
            return Err("invalid response header condition name".into());
        }
        for value in header
            .values
            .iter()
            .flatten()
            .chain(header.excluded_values.iter().flatten())
        {
            if !valid_header_value(value) {
                return Err("invalid response header condition value".into());
            }
            if value.len() > MAX_HEADER_PATTERN_BYTES {
                return Err("MAX_HEADER_PATTERN_BYTES exceeded".into());
            }
            crate::matcher::compile_header_pattern(value)?;
        }
    }
    if condition
        .excluded_response_headers
        .iter()
        .flatten()
        .any(|excluded| {
            excluded.values.is_none()
                && excluded.excluded_values.is_none()
                && condition
                    .response_headers
                    .iter()
                    .flatten()
                    .any(|included| included.header.eq_ignore_ascii_case(&excluded.header))
        })
    {
        return Err("response header cannot be required and excluded by name".into());
    }
    Ok(())
}

fn header_pattern_count(condition: &Condition) -> usize {
    condition
        .response_headers
        .iter()
        .flatten()
        .chain(condition.excluded_response_headers.iter().flatten())
        .map(|header| {
            header
                .values
                .as_ref()
                .map_or(0, Vec::len)
                .saturating_add(header.excluded_values.as_ref().map_or(0, Vec::len))
        })
        .fold(0, usize::saturating_add)
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

fn valid_header_value(value: &str) -> bool {
    !value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
}

fn validate_header_modifications(
    headers: &[HeaderModification],
    request: bool,
) -> Result<(), String> {
    for header in headers {
        if !valid_header_name(&header.header) {
            return Err("invalid header modification name".into());
        }
        match (header.operation, header.value.as_deref()) {
            (HeaderOperation::Remove, Some(_)) => {
                return Err("remove header operation must not specify value".into());
            }
            (HeaderOperation::Append | HeaderOperation::Set, None) => {
                return Err("append/set header operation requires value".into());
            }
            (_, Some(value)) if !valid_header_value(value) => {
                return Err("invalid header modification value".into());
            }
            _ => {}
        }
        // developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest#header_modification
        if request
            && header.operation == HeaderOperation::Append
            && ![
                "accept",
                "accept-encoding",
                "accept-language",
                "access-control-request-headers",
                "cache-control",
                "connection",
                "content-language",
                "cookie",
                "forwarded",
                "if-match",
                "if-none-match",
                "keep-alive",
                "range",
                "te",
                "trailer",
                "transfer-encoding",
                "upgrade",
                "user-agent",
                "via",
                "want-digest",
                "x-forwarded-for",
            ]
            .contains(&header.header.as_str())
        {
            return Err("append is not supported for this request header".into());
        }
    }
    Ok(())
}

fn validate_redirect(redirect: &Redirect, regex: Option<&Regex>) -> Result<(), String> {
    // Chromium indexed_rule.cc ParseRedirect chooses the first present target.
    if let Some(target) = &redirect.url {
        let url = url::Url::parse(target).map_err(|_| "invalid redirect URL")?;
        if url.scheme() == "javascript" {
            return Err("javascript redirect URLs are forbidden".into());
        }
    } else if let Some(path) = &redirect.extension_path {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(
                "extensionPath must start with / and remain in the extension origin".into(),
            );
        }
    } else if let Some(transform) = &redirect.transform {
        validate_transform(transform)?;
    } else if let Some(substitution) = &redirect.regex_substitution {
        let regex = regex.ok_or("regexSubstitution requires regexFilter")?;
        validate_substitution(substitution, regex.captures_len())?;
    } else {
        return Err("redirect requires url, extensionPath, transform or regexSubstitution".into());
    }
    Ok(())
}

fn validate_transform(transform: &UrlTransform) -> Result<(), String> {
    if transform
        .scheme
        .as_deref()
        .is_some_and(|scheme| !["http", "https", "ftp", "chrome-extension"].contains(&scheme))
    {
        return Err("invalid transform scheme".into());
    }
    if transform.port.as_deref().is_some_and(|port| {
        !port.is_empty()
            && (port.parse::<u16>().is_err() || !port.bytes().all(|b| b.is_ascii_digit()))
    }) {
        return Err("transform port must be empty or an integer from 0 to 65535".into());
    }
    if transform
        .query
        .as_deref()
        .is_some_and(|query| !query.is_empty() && !query.starts_with('?'))
    {
        return Err("transform query must be empty or start with ?".into());
    }
    if transform
        .fragment
        .as_deref()
        .is_some_and(|fragment| !fragment.is_empty() && !fragment.starts_with('#'))
    {
        return Err("transform fragment must be empty or start with #".into());
    }
    if transform.query.is_some() && transform.query_transform.is_some() {
        return Err("transform query and queryTransform cannot both be specified".into());
    }
    Ok(())
}

fn validate_substitution(substitution: &str, capture_count: usize) -> Result<(), String> {
    if substitution.is_empty() {
        return Err("regexSubstitution must not be empty".into());
    }
    let mut bytes = substitution.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'\\' {
            match bytes.next() {
                Some(b'\\') => {}
                Some(digit @ b'0'..=b'9') if usize::from(digit - b'0') < capture_count => {}
                _ => {
                    return Err(
                        "regexSubstitution has an invalid escape or capture reference".into(),
                    )
                }
            }
        }
    }
    Ok(())
}

/// Quotas apply to the accepted rules; static callers must also aggregate across enabled files.
pub fn validate_rules(rules: &[Rule], kind: RulesetKind) -> Vec<RuleRejection> {
    let mut seen = HashSet::with_capacity(rules.len().min(MAX_RULES_PER_RULESET));
    let mut accepted = 0;
    let mut regex_count = 0;
    let mut unsafe_count = 0;
    let mut header_patterns = 0usize;
    let mut rejected = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        let reason = validate_rule(rule, kind).err().or_else(|| {
            if seen.contains(&rule.id) {
                Some("duplicate rule id".into())
            } else if accepted >= MAX_RULES_PER_RULESET {
                Some("implementation ruleset resource limit exceeded".into())
            } else if kind == RulesetKind::Dynamic && accepted >= MAX_NUMBER_OF_DYNAMIC_RULES {
                Some("MAX_NUMBER_OF_DYNAMIC_RULES exceeded".into())
            } else if kind == RulesetKind::Session && accepted >= MAX_NUMBER_OF_SESSION_RULES {
                Some("MAX_NUMBER_OF_SESSION_RULES exceeded".into())
            } else if rule.condition.regex_filter.is_some()
                && regex_count >= MAX_NUMBER_OF_REGEX_RULES
            {
                Some("MAX_NUMBER_OF_REGEX_RULES exceeded".into())
            } else if kind == RulesetKind::Dynamic
                && !rule.action.is_safe()
                && unsafe_count >= MAX_NUMBER_OF_UNSAFE_DYNAMIC_RULES
            {
                Some("MAX_NUMBER_OF_UNSAFE_DYNAMIC_RULES exceeded".into())
            } else if header_patterns.saturating_add(header_pattern_count(&rule.condition))
                > MAX_HEADER_PATTERNS_PER_RULESET
            {
                Some("MAX_HEADER_PATTERNS_PER_RULESET exceeded".into())
            } else {
                None
            }
        });
        if let Some(reason) = reason {
            rejected.push(RuleRejection {
                index,
                id: Some(rule.id),
                reason,
            });
        } else {
            seen.insert(rule.id);
            accepted += 1;
            regex_count += usize::from(rule.condition.regex_filter.is_some());
            unsafe_count += usize::from(!rule.action.is_safe());
            header_patterns += header_pattern_count(&rule.condition);
        }
    }
    rejected
}

pub fn validate_and_filter_rules(
    rules: Vec<Rule>,
    kind: RulesetKind,
) -> Result<ParsedRuleset, ParseError> {
    let rejected = validate_rules(&rules, kind);
    finish_validation(rules, rejected, kind)
}

fn finish_validation(
    rules: Vec<Rule>,
    rejected: Vec<RuleRejection>,
    kind: RulesetKind,
) -> Result<ParsedRuleset, ParseError> {
    if kind != RulesetKind::Static && !rejected.is_empty() {
        return Err(ParseError::Rejected(rejected));
    }
    let indices: HashSet<_> = rejected.iter().map(|rejection| rejection.index).collect();
    Ok(ParsedRuleset {
        rules: rules
            .into_iter()
            .enumerate()
            .filter_map(|(index, rule)| (!indices.contains(&index)).then_some(rule))
            .collect(),
        rejected,
    })
}

/// Removal precedes addition; assignment happens only after full validation succeeds.
pub fn update_rules(
    current: &mut Vec<Rule>,
    kind: RulesetKind,
    remove_rule_ids: &[u32],
    add_rules: Vec<Rule>,
) -> Result<(), ParseError> {
    if kind == RulesetKind::Static {
        return Err(ParseError::InvalidRuleset(
            "static rules cannot use dynamic/session updates".into(),
        ));
    }
    let removals: HashSet<_> = remove_rule_ids.iter().copied().collect();
    let mut replacement: Vec<_> = current
        .iter()
        .filter(|rule| !removals.contains(&rule.id))
        .cloned()
        .collect();
    replacement.extend(add_rules);
    let replacement = validate_and_filter_rules(replacement, kind)?;
    *current = replacement.rules;
    Ok(())
}

#[cfg(feature = "compile")]
pub fn parse_rules_json(bytes: &[u8], kind: RulesetKind) -> Result<ParsedRuleset, ParseError> {
    if bytes.len() > MAX_RULESET_JSON_BYTES {
        return Err(ParseError::InvalidRuleset(
            "ruleset JSON resource limit exceeded".into(),
        ));
    }
    let values: Vec<serde_json::Value> = serde_json::from_slice(bytes)
        .map_err(|error| ParseError::InvalidJson(error.to_string()))?;
    if values.len() > MAX_RULES_PER_RULESET {
        return Err(ParseError::InvalidRuleset(
            "implementation ruleset resource limit exceeded".into(),
        ));
    }
    let mut rules = Vec::with_capacity(values.len());
    let mut original_indices = Vec::with_capacity(values.len());
    let mut rejected = Vec::new();
    for (index, value) in values.into_iter().enumerate() {
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|id| u32::try_from(id).ok());
        match check_json_shape(&value)
            .and_then(|()| serde_json::from_value::<Rule>(value).map_err(|error| error.to_string()))
        {
            Ok(rule) => {
                rules.push(rule);
                original_indices.push(index);
            }
            Err(reason) => rejected.push(RuleRejection { index, id, reason }),
        }
    }
    let validation_errors = validate_rules(&rules, kind);
    let invalid: HashSet<_> = validation_errors.iter().map(|error| error.index).collect();
    rejected.extend(validation_errors.into_iter().map(|mut error| {
        error.index = original_indices[error.index];
        error
    }));
    rejected.sort_by_key(|error| error.index);
    if kind != RulesetKind::Static && !rejected.is_empty() {
        return Err(ParseError::Rejected(rejected));
    }
    Ok(ParsedRuleset {
        rules: rules
            .into_iter()
            .enumerate()
            .filter_map(|(index, rule)| (!invalid.contains(&index)).then_some(rule))
            .collect(),
        rejected,
    })
}

#[cfg(feature = "compile")]
fn check_json_shape(value: &serde_json::Value) -> Result<(), String> {
    if let Some(condition) = value.get("condition") {
        reject_null_fields(
            condition,
            &[
                "urlFilter",
                "regexFilter",
                "initiatorDomains",
                "excludedInitiatorDomains",
                "requestDomains",
                "excludedRequestDomains",
                "topDomains",
                "excludedTopDomains",
                "domains",
                "excludedDomains",
                "resourceTypes",
                "excludedResourceTypes",
                "requestMethods",
                "excludedRequestMethods",
                "domainType",
                "tabIds",
                "excludedTabIds",
                "responseHeaders",
                "excludedResponseHeaders",
            ],
        )?;
        for key in ["responseHeaders", "excludedResponseHeaders"] {
            for header in condition
                .get(key)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                reject_null_fields(header, &["values", "excludedValues"])?;
            }
        }
    }
    if let Some(action) = value.get("action") {
        let action_type = action.get("type").and_then(serde_json::Value::as_str);
        if action_type != Some("redirect") && action.get("redirect").is_some() {
            return Err("action.redirect is only valid for redirect rules".into());
        }
        if action_type != Some("modifyHeaders")
            && (action.get("requestHeaders").is_some() || action.get("responseHeaders").is_some())
        {
            return Err("header modifications are only valid for modifyHeaders rules".into());
        }
        if action.get("type").and_then(serde_json::Value::as_str) == Some("modifyHeaders") {
            for key in ["requestHeaders", "responseHeaders"] {
                if action
                    .get(key)
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(Vec::is_empty)
                {
                    return Err(format!("{key} must not be empty when specified"));
                }
                for header in action
                    .get(key)
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    reject_null_fields(header, &["value"])?;
                }
            }
        }
        if action.get("type").and_then(serde_json::Value::as_str) == Some("redirect") {
            if let Some(redirect) = action.get("redirect") {
                reject_null_fields(
                    redirect,
                    &["url", "extensionPath", "transform", "regexSubstitution"],
                )?;
                if let Some(transform) = redirect.get("transform") {
                    reject_null_fields(
                        transform,
                        &[
                            "scheme",
                            "host",
                            "port",
                            "path",
                            "query",
                            "queryTransform",
                            "fragment",
                            "username",
                            "password",
                        ],
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(feature = "compile")]
fn reject_null_fields(value: &serde_json::Value, fields: &[&str]) -> Result<(), String> {
    for field in fields {
        if value.get(*field).is_some_and(serde_json::Value::is_null) {
            return Err(format!("{field} must not be null"));
        }
    }
    Ok(())
}

#[cfg(feature = "compile")]
pub fn update_rules_json(
    current: &mut Vec<Rule>,
    kind: RulesetKind,
    remove_rule_ids: &[u32],
    add_rules_json: &[u8],
) -> Result<(), ParseError> {
    let additions = parse_rules_json(add_rules_json, kind)?;
    update_rules(current, kind, remove_rule_ids, additions.rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: u32) -> Rule {
        Rule {
            id,
            priority: 1,
            action: Action::Block,
            condition: Condition::default(),
        }
    }

    #[test]
    fn identifiers_priorities_and_duplicate_ids_are_rejected() {
        let mut invalid_priority = block(2);
        invalid_priority.priority = 0;
        let invalid = validate_rules(
            &[
                block(0),
                block(1),
                block(1),
                invalid_priority,
                block(u32::MAX),
            ],
            RulesetKind::Static,
        );
        assert_eq!(
            invalid.iter().map(|error| error.index).collect::<Vec<_>>(),
            [0, 2, 3, 4]
        );
        assert_eq!(invalid[1].id, Some(1));
        assert!(invalid[1].reason.contains("duplicate"));
    }

    #[test]
    fn url_filters_and_regex_filters_validate_their_separate_grammars() {
        for invalid in ["", "||*example", "é"] {
            let mut rule = block(1);
            rule.condition.url_filter = Some(invalid.into());
            assert!(
                validate_rule(&rule, RulesetKind::Static).is_err(),
                "{invalid}"
            );
        }
        for valid in [
            "|https://example.com/",
            "||example.com^",
            "*",
            "|",
            "||",
            "foo|bar",
        ] {
            let mut rule = block(1);
            rule.condition.url_filter = Some(valid.into());
            assert!(validate_rule(&rule, RulesetKind::Static).is_ok(), "{valid}");
        }
        let mut rule = block(1);
        rule.condition.url_filter = Some("example".into());
        rule.condition.regex_filter = Some("example".into());
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        assert!(compile_regex("", false).is_err());
        assert!(compile_regex("é", false).is_err());
        assert!(compile_regex("(?=foo)", false).is_err());
        assert!(compile_regex(r"(foo)\1", false).is_err());
        assert!(compile_regex("(?:ab|cd|ef){1000}", false)
            .unwrap_err()
            .contains("compiled-size"));
        assert!(compile_regex(r"\w+", false).unwrap().is_match(b"abc"));
        assert!(!compile_regex(r"^\w+$", false)
            .unwrap()
            .is_match("é".as_bytes()));
        assert!(compile_regex("EXAMPLE", false)
            .unwrap()
            .is_match(b"example"));
        assert!(!compile_regex("EXAMPLE", true).unwrap().is_match(b"example"));
    }

    #[test]
    fn re2_escapes_are_normalized_without_enabling_rust_extensions() {
        for (pattern, matching, missing) in [
            (
                r"\Qexample.com/?x=1\E",
                "example.com/?x=1",
                "exampleXcom/?x=1",
            ),
            (r"\Qunclosed.*", "unclosed.*", "unclosed-anything"),
            (r"a\Cb", "a\nb", "ab"),
            (r"\141\x62\x{63}", "abc", "123"),
            (r"[\141-\143]+", "abc", "xyz"),
            (r"[[:alpha:]]+", "abc", "123"),
            (r"\p{Latin}+", "abc", "123"),
            (r"[\p{Latin}\d]+", "abc123", "_-"),
            (r"\<literal\>", "<literal>", "literal"),
        ] {
            let regex =
                compile_regex(pattern, false).unwrap_or_else(|error| panic!("{pattern}: {error}"));
            assert!(regex.is_match(matching.as_bytes()), "{pattern}");
            assert!(!regex.is_match(missing.as_bytes()), "{pattern}");
        }
        for pattern in [
            r"(?x)a b",
            r"(?u:a)",
            r"(?R:a)",
            r"\u0061",
            r"\U00000061",
            r"[a&&b]",
            r"[a--b]",
            r"[a~~b]",
            r"[a[b]]",
            r"[\b]",
            r"a{1001}",
            r"(?:){1001}",
            r"a**",
            r"a*??",
            r"(a)\1",
        ] {
            assert!(compile_regex(pattern, false).is_err(), "{pattern}");
        }
        assert!(compile_regex(r"a*?", false).is_ok());
        assert!(compile_regex(r"(?imsU:a+)", false).is_ok());
        assert!(!compile_regex(r"\s", false).unwrap().is_match(b"\x0b"));
        assert!(compile_regex(r"\v", false).unwrap().is_match(b"\x0b"));
        assert!(!compile_regex(r"[\p{Latin}\d]+", false)
            .unwrap()
            .is_match("١".as_bytes()));
    }

    #[test]
    fn domain_aliases_non_ascii_and_empty_positive_lists_are_invalid() {
        let mut rule = block(1);
        rule.condition.domains = Some(vec!["example.com".into()]);
        rule.condition.initiator_domains = Some(vec!["example.com".into()]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.domains = None;
        rule.condition.initiator_domains = Some(vec!["é.example".into()]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.initiator_domains = Some(vec![]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.initiator_domains = None;
        rule.condition.excluded_initiator_domains = Some(vec![]);
        rule.condition.excluded_request_domains = Some(vec![]);
        rule.condition.excluded_top_domains = Some(vec![]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_ok());
        rule.condition.top_domains = Some(vec![]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
    }

    #[test]
    fn resource_and_method_lists_follow_documented_exclusivity() {
        let mut rule = block(1);
        rule.condition.resource_types = Some(vec![]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.resource_types = Some(vec![ResourceType::Script]);
        rule.condition.excluded_resource_types = Some(vec![ResourceType::Image]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.resource_types = None;
        rule.condition.excluded_resource_types = None;
        rule.condition.request_methods = Some(vec![]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.request_methods = Some(vec![RequestMethod::Get]);
        rule.condition.excluded_request_methods = Some(vec![RequestMethod::Post]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
    }

    #[test]
    fn allow_all_requires_frame_resource_types() {
        let mut rule = block(1);
        rule.action = Action::AllowAllRequests;
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.resource_types = Some(vec![ResourceType::MainFrame, ResourceType::SubFrame]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_ok());
        rule.condition.resource_types = Some(vec![ResourceType::Script]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
    }

    #[test]
    fn tab_filters_are_session_only_and_cannot_overlap() {
        let mut rule = block(1);
        rule.condition.tab_ids = Some(vec![-1, 2]);
        assert!(validate_rule(&rule, RulesetKind::Session).is_ok());
        assert!(validate_rule(&rule, RulesetKind::Dynamic).is_err());
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.excluded_tab_ids = Some(vec![2]);
        assert!(validate_rule(&rule, RulesetKind::Session).is_err());
    }

    #[test]
    fn redirects_validate_selected_target_and_capture_references() {
        let mut rule = block(1);
        for redirect in [
            Redirect::default(),
            Redirect {
                url: Some("javascript:alert(1)".into()),
                ..Redirect::default()
            },
            Redirect {
                url: Some("relative".into()),
                ..Redirect::default()
            },
            Redirect {
                extension_path: Some("relative".into()),
                ..Redirect::default()
            },
            Redirect {
                extension_path: Some("//other/asset".into()),
                ..Redirect::default()
            },
            Redirect {
                regex_substitution: Some(r"https://new.example/\1".into()),
                ..Redirect::default()
            },
        ] {
            rule.action = Action::Redirect {
                redirect: Box::new(redirect),
            };
            assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        }
        rule.condition.regex_filter = Some("(old)".into());
        for substitution in [
            r"https://new.example/\1",
            r"https://new.example/\0",
            r"https://new.example/\\",
        ] {
            rule.action = Action::Redirect {
                redirect: Box::new(Redirect {
                    regex_substitution: Some(substitution.into()),
                    ..Redirect::default()
                }),
            };
            assert!(validate_rule(&rule, RulesetKind::Static).is_ok());
        }
        for substitution in [
            "",
            r"https://new.example/\2",
            r"https://new.example/\x",
            "trailing\\",
        ] {
            rule.action = Action::Redirect {
                redirect: Box::new(Redirect {
                    regex_substitution: Some(substitution.into()),
                    ..Redirect::default()
                }),
            };
            assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        }
        rule.action = Action::Redirect {
            redirect: Box::new(Redirect {
                url: Some("https://new.example/".into()),
                regex_substitution: Some(r"bad\9".into()),
                ..Redirect::default()
            }),
        };
        assert!(validate_rule(&rule, RulesetKind::Static).is_ok());
    }

    #[test]
    fn transforms_validate_scheme_port_query_and_fragment() {
        let mut rule = block(1);
        for transform in [
            UrlTransform {
                scheme: Some("javascript".into()),
                ..UrlTransform::default()
            },
            UrlTransform {
                port: Some("65536".into()),
                ..UrlTransform::default()
            },
            UrlTransform {
                port: Some("+1".into()),
                ..UrlTransform::default()
            },
            UrlTransform {
                query: Some("a=b".into()),
                ..UrlTransform::default()
            },
            UrlTransform {
                fragment: Some("fragment".into()),
                ..UrlTransform::default()
            },
            UrlTransform {
                query: Some("?a=b".into()),
                query_transform: Some(QueryTransform::default()),
                ..UrlTransform::default()
            },
        ] {
            rule.action = Action::Redirect {
                redirect: Box::new(Redirect {
                    transform: Some(transform),
                    ..Redirect::default()
                }),
            };
            assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        }
        rule.action = Action::Redirect {
            redirect: Box::new(Redirect {
                transform: Some(UrlTransform {
                    scheme: Some("https".into()),
                    port: Some("443".into()),
                    query: Some("?a=b".into()),
                    fragment: Some("#fragment".into()),
                    ..UrlTransform::default()
                }),
                ..Redirect::default()
            }),
        };
        assert!(validate_rule(&rule, RulesetKind::Static).is_ok());
    }

    #[test]
    fn header_operations_validate_values_and_request_append_allowlist() {
        let mut rule = block(1);
        for header in [
            HeaderModification {
                header: "bad\r\nname".into(),
                operation: HeaderOperation::Remove,
                value: None,
            },
            HeaderModification {
                header: "x-test".into(),
                operation: HeaderOperation::Set,
                value: None,
            },
            HeaderModification {
                header: "x-test".into(),
                operation: HeaderOperation::Remove,
                value: Some("bad".into()),
            },
            HeaderModification {
                header: "x-test".into(),
                operation: HeaderOperation::Set,
                value: Some("bad\r\nvalue".into()),
            },
            HeaderModification {
                header: "x-test".into(),
                operation: HeaderOperation::Append,
                value: Some("value".into()),
            },
            HeaderModification {
                header: "Cookie".into(),
                operation: HeaderOperation::Append,
                value: Some("value".into()),
            },
        ] {
            rule.action = Action::ModifyHeaders {
                request_headers: vec![header],
                response_headers: vec![],
            };
            assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        }
        rule.action = Action::ModifyHeaders {
            request_headers: vec![HeaderModification {
                header: "cookie".into(),
                operation: HeaderOperation::Append,
                value: Some("key=value".into()),
            }],
            response_headers: vec![],
        };
        assert!(validate_rule(&rule, RulesetKind::Static).is_ok());
        rule.action = Action::ModifyHeaders {
            request_headers: vec![],
            response_headers: vec![HeaderModification {
                header: "x-test".into(),
                operation: HeaderOperation::Append,
                value: Some("value".into()),
            }],
        };
        assert!(validate_rule(&rule, RulesetKind::Static).is_ok());
        assert!(rule.action.has_unsupported_response_header_modifications());
    }

    #[test]
    fn response_header_conditions_refuse_invalid_or_incompatible_actions() {
        let mut rule = block(1);
        rule.condition.response_headers = Some(vec![]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.response_headers = Some(vec![HeaderCondition {
            header: "content-type".into(),
            values: Some(vec!["text/*".into()]),
            excluded_values: None,
        }]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_ok());
        rule.condition.excluded_response_headers = Some(vec![HeaderCondition {
            header: "Content-Type".into(),
            values: None,
            excluded_values: None,
        }]);
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
        rule.condition.excluded_response_headers = None;
        rule.action = Action::ModifyHeaders {
            request_headers: vec![HeaderModification {
                header: "cookie".into(),
                operation: HeaderOperation::Remove,
                value: None,
            }],
            response_headers: vec![],
        };
        assert!(validate_rule(&rule, RulesetKind::Static).is_err());
    }

    #[test]
    fn response_header_pattern_limits_refuse_per_rule_and_aggregate_excess() {
        let with_patterns = |id, count| {
            let mut rule = block(id);
            rule.condition.response_headers = Some(vec![HeaderCondition {
                header: "x-test".into(),
                values: Some(vec!["value".into(); count]),
                excluded_values: None,
            }]);
            rule
        };
        let error = validate_rule(
            &with_patterns(1, MAX_HEADER_PATTERNS_PER_RULE + 1),
            RulesetKind::Static,
        )
        .unwrap_err();
        assert!(error.contains("MAX_HEADER_PATTERNS_PER_RULE"));
        let mut huge = with_patterns(1, 1);
        huge.condition.response_headers.as_mut().unwrap()[0].values =
            Some(vec!["a".repeat(MAX_HEADER_PATTERN_BYTES + 1)]);
        assert!(validate_rule(&huge, RulesetKind::Static)
            .unwrap_err()
            .contains("MAX_HEADER_PATTERN_BYTES"));
        huge.condition.response_headers.as_mut().unwrap()[0].values =
            Some(vec!["*a".repeat(1_000)]);
        assert!(validate_rule(&huge, RulesetKind::Static).is_err());
        let accepted_count = MAX_HEADER_PATTERNS_PER_RULESET / MAX_HEADER_PATTERNS_PER_RULE;
        let mut rules: Vec<_> = (1..=(accepted_count + 1) as u32)
            .map(|id| with_patterns(id, MAX_HEADER_PATTERNS_PER_RULE))
            .collect();
        let errors = validate_rules(&rules, RulesetKind::Static);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].index, accepted_count);
        assert!(errors[0].reason.contains("MAX_HEADER_PATTERNS_PER_RULESET"));
        let addition = rules.pop().unwrap();
        let original = rules.clone();
        assert!(update_rules(&mut rules, RulesetKind::Session, &[], vec![addition]).is_err());
        assert_eq!(rules, original);
    }

    #[test]
    fn static_skips_bad_rules_and_updates_preserve_state_on_failure() {
        let parsed =
            validate_and_filter_rules(vec![block(1), block(0), block(2)], RulesetKind::Static)
                .unwrap();
        assert_eq!(
            parsed.rules.iter().map(|rule| rule.id).collect::<Vec<_>>(),
            [1, 2]
        );
        let mut current = vec![block(1)];
        assert!(update_rules(&mut current, RulesetKind::Dynamic, &[1], vec![block(0)]).is_err());
        assert_eq!(current, [block(1)]);
        assert!(update_rules(&mut current, RulesetKind::Dynamic, &[], vec![block(1)]).is_err());
        assert_eq!(current, [block(1)]);
        let mut replacement = block(1);
        replacement.action = Action::Allow;
        update_rules(
            &mut current,
            RulesetKind::Dynamic,
            &[0, 1, 999],
            vec![replacement.clone()],
        )
        .unwrap();
        assert_eq!(current, [replacement]);
        assert!(update_rules(&mut current, RulesetKind::Static, &[], vec![]).is_err());
    }

    #[test]
    fn dynamic_safe_unsafe_session_and_regex_quotas_are_distinct() {
        let rules: Vec<_> = (1..=(MAX_NUMBER_OF_DYNAMIC_RULES + 1) as u32)
            .map(block)
            .collect();
        let errors = validate_rules(&rules, RulesetKind::Dynamic);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].reason.contains("MAX_NUMBER_OF_DYNAMIC_RULES"));
        let errors = validate_rules(
            &rules[..MAX_NUMBER_OF_SESSION_RULES + 1],
            RulesetKind::Session,
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].reason.contains("MAX_NUMBER_OF_SESSION_RULES"));
        let rules: Vec<_> = (1..=(MAX_NUMBER_OF_UNSAFE_DYNAMIC_RULES + 1) as u32)
            .map(|id| {
                let mut rule = block(id);
                rule.action = Action::Redirect {
                    redirect: Box::new(Redirect {
                        url: Some("https://example.com/".into()),
                        ..Redirect::default()
                    }),
                };
                rule
            })
            .collect();
        let errors = validate_rules(&rules, RulesetKind::Dynamic);
        assert_eq!(errors.len(), 1);
        assert!(errors[0]
            .reason
            .contains("MAX_NUMBER_OF_UNSAFE_DYNAMIC_RULES"));
        let rules: Vec<_> = (1..=(MAX_NUMBER_OF_REGEX_RULES + 1) as u32)
            .map(|id| {
                let mut rule = block(id);
                rule.condition.regex_filter = Some("example".into());
                rule
            })
            .collect();
        let errors = validate_rules(&rules, RulesetKind::Static);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].reason.contains("MAX_NUMBER_OF_REGEX_RULES"));
        assert!(validate_rules(&[block(1)], RulesetKind::Static).is_empty());
    }

    #[cfg(feature = "compile")]
    #[test]
    fn json_parser_defaults_priority_and_reports_original_error_indices() {
        let json = br#"[
            {"id":1,"action":{"type":"block"},"condition":{}},
            {"id":2,"action":{"type":"unknown"},"condition":{}},
            {"id":3,"priority":0,"action":{"type":"allow"},"condition":{}},
            {"id":4,"action":{"type":"upgradeScheme"},"condition":{}},
            {"id":4,"action":{"type":"block"},"condition":{}}
        ]"#;
        let parsed = parse_rules_json(json, RulesetKind::Static).unwrap();
        assert_eq!(parsed.rules.len(), 2);
        assert_eq!(parsed.rules[0].priority, 1);
        assert_eq!(
            parsed
                .rejected
                .iter()
                .map(|error| error.index)
                .collect::<Vec<_>>(),
            [1, 2, 4]
        );
        assert_eq!(parsed.rejected[0].id, Some(2));
        assert!(matches!(
            parse_rules_json(json, RulesetKind::Dynamic),
            Err(ParseError::Rejected(_))
        ));
        assert!(parse_rules_json(b"{}", RulesetKind::Static).is_err());
        assert!(parse_rules_json(b"[", RulesetKind::Static).is_err());
    }

    #[cfg(feature = "compile")]
    #[test]
    fn json_model_carries_every_current_condition_and_transform_field() {
        let json = br##"[{
            "id":1,"priority":2,"action":{"type":"redirect","redirect":{"transform":{
                "scheme":"https","host":"new.example","port":"443","path":"/new",
                "fragment":"#part","username":"user","password":"pass",
                "queryTransform":{"removeParams":["tracking"],"addOrReplaceParams":[{"key":"q","value":"new","replaceOnly":true}]}
            }}},"condition":{
                "urlFilter":"||example.com^","isUrlFilterCaseSensitive":true,
                "initiatorDomains":["initiator.example"],"excludedInitiatorDomains":["excluded.example"],
                "requestDomains":["request.example"],"excludedRequestDomains":["request-excluded.example"],
                "topDomains":["top.example"],"excludedTopDomains":["top-excluded.example"],
                "resourceTypes":["main_frame","sub_frame","stylesheet","script","image","font","object","xmlhttprequest","ping","csp_report","media","websocket","webtransport","webbundle","other"],
                "requestMethods":["connect","delete","get","head","options","patch","post","put","other"],
                "domainType":"thirdParty","tabIds":[-1,3],"excludedTabIds":[4],
                "responseHeaders":[{"header":"content-type","values":["text/*"],"excludedValues":["text/secret"]}],
                "excludedResponseHeaders":[{"header":"x-test","values":["secret"]}]
            }
        }]"##;
        let parsed = parse_rules_json(json, RulesetKind::Session).unwrap();
        assert!(parsed.rejected.is_empty());
        let rule = &parsed.rules[0];
        assert_eq!(rule.condition.resource_types.as_ref().unwrap().len(), 15);
        assert_eq!(rule.condition.request_methods.as_ref().unwrap().len(), 9);
        assert_eq!(rule.condition.domain_type, Some(DomainType::ThirdParty));
        let Action::Redirect { redirect } = &rule.action else {
            panic!("expected redirect")
        };
        let transform = redirect.transform.as_ref().unwrap();
        assert_eq!(transform.username.as_deref(), Some("user"));
        assert!(
            transform
                .query_transform
                .as_ref()
                .unwrap()
                .add_or_replace_params[0]
                .replace_only
        );
    }

    #[cfg(feature = "compile")]
    #[test]
    fn json_explicit_empty_header_arrays_and_invalid_updates_are_atomic() {
        let json = br#"[{"id":2,"action":{"type":"modifyHeaders","requestHeaders":[],"responseHeaders":[{"header":"x-test","operation":"remove"}]},"condition":{}}]"#;
        assert_eq!(
            parse_rules_json(json, RulesetKind::Static)
                .unwrap()
                .rejected
                .len(),
            1
        );
        let mut rules = vec![block(1)];
        assert!(update_rules_json(&mut rules, RulesetKind::Dynamic, &[1], json).is_err());
        assert_eq!(rules, [block(1)]);
        let json = br#"[{"id":1,"action":{"type":"allow"},"condition":{}}]"#;
        update_rules_json(&mut rules, RulesetKind::Dynamic, &[1], json).unwrap();
        assert_eq!(rules[0].action, Action::Allow);
    }

    #[cfg(feature = "compile")]
    #[test]
    fn json_optional_null_values_are_invalid_and_serialization_omits_absent_fields() {
        for json in [
            br#"[{"id":1,"action":{"type":"block"},"condition":{"urlFilter":null}}]"#.as_slice(),
            br#"[{"id":1,"action":{"type":"block"},"condition":{"requestDomains":null}}]"#,
            br#"[{"id":1,"action":{"type":"redirect","redirect":{"url":null}},"condition":{}}]"#,
            br#"[{"id":1,"action":{"type":"block"},"condition":{"responseHeaders":[{"header":"x-test","values":null}]}}]"#,
            br#"[{"id":1,"action":{"type":"block","redirect":{"url":"https://example.com/"}},"condition":{}}]"#,
        ] {
            assert_eq!(parse_rules_json(json, RulesetKind::Static).unwrap().rejected.len(), 1);
        }
        let rules = vec![
            block(1),
            Rule {
                action: Action::ModifyHeaders {
                    request_headers: vec![],
                    response_headers: vec![HeaderModification {
                        header: "x-test".into(),
                        operation: HeaderOperation::Remove,
                        value: None,
                    }],
                },
                ..block(2)
            },
        ];
        let json = serde_json::to_vec(&rules).unwrap();
        assert_eq!(
            parse_rules_json(&json, RulesetKind::Static).unwrap().rules,
            rules
        );
    }
}
