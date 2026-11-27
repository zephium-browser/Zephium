//! Takes what a person must not share out of a log line before it is copied
//! into a bug report. The log is written to hold counters and closed facts;
//! this is the second line of defence, applied again at the copy even to
//! lines that already say `content=redacted`. Everything it cannot prove
//! harmless and that looks like an address, a name, a secret or a place on
//! the person's disk is replaced, and what it is unsure of is removed.

use std::collections::HashMap;

const MAX_INPUT: usize = 4096;
const MAX_LINE: usize = 400;
const MAX_DEPTH: usize = 3;
const PATH_CONTINUATION: u8 = 4;

const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "sk_",
    "pk_",
    "rk_",
    "gsk_",
    "xai-",
    "pplx-",
    "AIza",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "xapp-",
    "hf_",
    "AKIA",
    "ASIA",
    "ya29.",
    "npm_",
    "shpat_",
    "whsec_",
];

/// Names whose value is a credential, once case and separators are dropped.
const SECRET_KEYS: &[&str] = &[
    "key",
    "apikey",
    "token",
    "bearer",
    "secret",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "authorization",
    "auth",
    "cookie",
    "setcookie",
    "sessionid",
    "sid",
    "credential",
    "credentials",
    "signature",
    "license",
    "otp",
];

const SECRET_SUFFIXES: &[&str] = &[
    "apikey",
    "secret",
    "secretkey",
    "privatekey",
    "token",
    "password",
    "passwd",
    "cookie",
    "authorization",
];

/// Names whose value is something the person typed, visited or named.
const CONTENT_KEYS: &[&str] = &[
    "title",
    "url",
    "uri",
    "href",
    "link",
    "query",
    "q",
    "search",
    "searchterm",
    "term",
    "text",
    "content",
    "body",
    "snippet",
    "prompt",
    "input",
    "goal",
    "task",
    "subject",
    "name",
    "label",
    "description",
    "host",
    "hostname",
    "origin",
    "site",
    "page",
    "email",
    "mail",
    "user",
    "username",
    "login",
    "file",
    "filename",
    "path",
    "dir",
    "folder",
    "directory",
    "address",
    "referrer",
    "referer",
];

/// Account names that are words anyway; replacing them would only blur the log.
const COMMON_NAMES: &[&str] = &[
    "users",
    "user",
    "home",
    "admin",
    "administrator",
    "guest",
    "public",
    "default",
    "root",
    "test",
    "temp",
];

const SPECIAL_SCHEMES: &[&str] = &[
    "mailto:",
    "data:",
    "blob:",
    "javascript:",
    "view-source:",
    "tel:",
    "sms:",
    "magnet:",
];

/// Top-level domains of three letters or more that a bare name may end in.
/// Two-letter endings are all country codes and are handled as a class.
const TLDS: &[&str] = &[
    "com",
    "net",
    "org",
    "edu",
    "gov",
    "mil",
    "int",
    "info",
    "biz",
    "name",
    "pro",
    "dev",
    "app",
    "io",
    "xyz",
    "top",
    "site",
    "online",
    "tech",
    "cloud",
    "store",
    "shop",
    "blog",
    "news",
    "live",
    "club",
    "space",
    "page",
    "link",
    "wiki",
    "email",
    "zone",
    "world",
    "network",
    "systems",
    "digital",
    "media",
    "agency",
    "studio",
    "design",
    "art",
    "fun",
    "life",
    "today",
    "website",
    "vip",
    "icu",
    "run",
    "build",
    "chat",
    "social",
    "company",
    "group",
    "global",
    "local",
    "lan",
    "internal",
    "home",
    "corp",
    "intranet",
    "localdomain",
    "onion",
    "download",
    "work",
    "tools",
    "software",
    "services",
    "solutions",
    "mobi",
    "asia",
    "cat",
    "tel",
    "travel",
    "jobs",
    "example",
    "test",
    "invalid",
];

/// Two-letter endings that name files and code far more often than countries.
const NOT_COUNTRIES: &[&str] = &[
    "rs", "ts", "js", "py", "md", "sh", "rb", "gz", "so", "db", "cs", "go", "xz", "bz", "hs", "jl",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Secret,
    Content,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    /// The next quoted string or word is the value.
    Value(Kind),
    /// The next piece is the value only if it is quoted, as in `title: "x"`;
    /// the same words in prose are not a name for something.
    Quoted,
    /// Everything to the end of the line is the value.
    Rest,
    /// A value that may run on, until the next `name=`.
    Fields,
    /// A path that may have had a space in a folder name.
    Path(u8),
}

pub(super) struct Redactor {
    homes: Vec<String>,
    names: Vec<String>,
    kept: Vec<String>,
    aliases: HashMap<(&'static str, String), usize>,
}

impl Redactor {
    pub(super) fn new(homes: impl IntoIterator<Item = String>, user: Option<String>) -> Self {
        let mut variants: Vec<String> = Vec::new();
        let mut names: Vec<String> = user.into_iter().collect();
        for home in homes {
            let trimmed = home.trim_end_matches(['/', '\\']);
            if trimmed.len() < 4 {
                continue;
            }
            let folder = trimmed.rsplit(['/', '\\']).next().unwrap_or_default();
            names.extend(
                folder
                    .split(|c: char| !c.is_alphanumeric())
                    .map(str::to_owned),
            );
            variants.push(trimmed.to_owned());
            if trimmed.contains('\\') {
                variants.push(trimmed.replace('\\', "\\\\"));
                variants.push(trimmed.replace('\\', "/"));
            }
        }
        variants.sort_by_key(|variant| std::cmp::Reverse(variant.len()));
        variants.dedup();
        names.retain(|name| {
            name.chars().count() >= 3
                && !COMMON_NAMES
                    .iter()
                    .any(|common| common.eq_ignore_ascii_case(name))
        });
        names.sort();
        names.dedup();
        Self {
            homes: variants,
            names,
            kept: Vec::new(),
            aliases: HashMap::new(),
        }
    }

    /// A value that is not private and is worth reading as it is, such as the
    /// running build, which would otherwise pass for a key.
    pub(super) fn keep(&mut self, value: impl Into<String>) {
        self.kept.push(value.into());
    }

    pub(super) fn line(&mut self, line: &str) -> String {
        let mut text = clean(cap(line, MAX_INPUT))
            .replace("\\\\\\\\?\\\\", "")
            .replace("\\\\?\\", "");
        for home in &self.homes {
            text = replace_home(&text, home);
        }
        text = replace_other_homes(&text);
        let mut scrubbed = self.scrub(&text, 0);
        for name in &self.names {
            scrubbed = replace_word(&scrubbed, name, "<user>");
        }
        shorten(&scrubbed, MAX_LINE)
    }

    fn scrub(&mut self, text: &str, depth: usize) -> String {
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut pending = Pending::None;
        let mut gap = false;
        let mut named: Option<Kind> = None;
        let mut at = 0;
        while at < bytes.len() {
            let byte = bytes[at];
            if byte.is_ascii_whitespace() {
                if swallowing(pending) {
                    gap = true;
                } else {
                    out.push(byte as char);
                }
                at += 1;
                continue;
            }
            if opens_quote(bytes, at) {
                named = None;
                let (inner_end, next) = quote_span(bytes, at);
                let quote = byte as char;
                let inner = &text[at + 1..inner_end];
                let closed = next > inner_end;
                at = next;
                match pending {
                    Pending::Rest | Pending::Fields | Pending::Path(_) => {}
                    Pending::Value(_) | Pending::Quoted => {
                        let kind = match pending {
                            Pending::Value(kind) => kind,
                            _ => Kind::Content,
                        };
                        pending = Pending::None;
                        out.push(quote);
                        out.push_str(placeholder(kind));
                        if closed {
                            out.push(quote);
                        }
                    }
                    Pending::None => {
                        out.push(quote);
                        out.push_str(&if is_path_start(inner) {
                            self.path(inner)
                        } else if depth < MAX_DEPTH {
                            self.scrub(inner, depth + 1)
                        } else {
                            "<redacted>".to_owned()
                        });
                        if closed {
                            out.push(quote);
                        }
                        if let Some((through, kind)) = json_key(text, at, inner) {
                            out.push_str(&text[at..through]);
                            at = through;
                            pending = Pending::Value(kind);
                        }
                    }
                }
                continue;
            }
            let end = chunk_end(bytes, at);
            let chunk = &text[at..end];
            at = end;
            let before = named.take();
            match pending {
                Pending::Rest => continue,
                Pending::Fields => {
                    if !is_field_start(chunk) {
                        continue;
                    }
                }
                Pending::Path(left) => {
                    pending = if left > 1 && !ends_path(chunk) {
                        Pending::Path(left - 1)
                    } else {
                        gap = false;
                        Pending::None
                    };
                    continue;
                }
                Pending::Value(kind) => {
                    let (lead, core, trail) = peel(chunk);
                    pending = if kind == Kind::Content && !trail.contains([',', ';']) {
                        Pending::Fields
                    } else {
                        Pending::None
                    };
                    if gap {
                        out.push(' ');
                        gap = false;
                    }
                    out.push_str(lead);
                    let keep = kind == Kind::Secret && benign_value(core);
                    out.push_str(if keep { core } else { placeholder(kind) });
                    out.push_str(trail);
                    continue;
                }
                Pending::Quoted | Pending::None => {}
            }
            if gap {
                out.push(' ');
                gap = false;
            }
            let (lead, core, trail) = peel(chunk);
            out.push_str(lead);
            let (text, next) = match before {
                // `password = value`: the sign came after the name.
                Some(kind) if core.starts_with('=') => assigned(kind, &core[1..]),
                _ => self.word(core, trail),
            };
            named = (next == Pending::None && !core.is_empty() && core == ident_suffix(core))
                .then(|| key_kind(core))
                .flatten();
            out.push_str(&text);
            out.push_str(trail);
            if next == Pending::Rest {
                out.push_str(" <secret>");
            }
            pending = next;
        }
        out
    }

    /// One whitespace-free word with its punctuation taken off, and what the
    /// words after it are.
    fn word(&mut self, core: &str, trail: &str) -> (String, Pending) {
        if core.is_empty() || self.kept.iter().any(|kept| kept == core) {
            return (core.to_owned(), Pending::None);
        }
        if let Some(colon) = core.find("://") {
            let start = scheme_start(core, colon);
            return (format!("{}<url>", &core[..start]), Pending::None);
        }
        if let Some(start) = special_scheme(core) {
            return (format!("{}<url>", &core[..start]), Pending::None);
        }
        if let Some(equals) = core.find('=') {
            let (name, value) = (&core[..equals], &core[equals + 1..]);
            let key = ident_suffix(name);
            let before = &name[..name.len() - key.len()];
            // Padding at the end of a base64 string is not a value, and a
            // name that hangs off a path or an address is part of it.
            if !key.is_empty()
                && key.len() <= 40
                && !before.contains(['/', '\\', ':', '@', '?', '&', '#'])
                && (value.is_empty() || !value.starts_with('='))
            {
                let kind = key_kind(key);
                if value.is_empty() {
                    return (core.to_owned(), kind.map_or(Pending::None, Pending::Value));
                }
                return match kind {
                    Some(Kind::Secret) if !benign_value(value) => {
                        let scheme = value.eq_ignore_ascii_case("bearer")
                            || value.eq_ignore_ascii_case("basic");
                        let next = if scheme {
                            Pending::Value(Kind::Secret)
                        } else {
                            Pending::None
                        };
                        (format!("{name}=<secret>"), next)
                    }
                    Some(Kind::Content) if !benign_value(value) => {
                        (format!("{name}=<redacted>"), Pending::Fields)
                    }
                    Some(_) => (core.to_owned(), Pending::None),
                    None => {
                        let (value, next) = self.word(value, trail);
                        (format!("{name}={value}"), next)
                    }
                };
            }
        }
        if core.contains('&') && core.contains('=') {
            return ("<redacted>".into(), Pending::None);
        }
        if trail.starts_with(':') {
            match key_kind(ident_suffix(core)) {
                Some(Kind::Secret) => return (core.to_owned(), Pending::Rest),
                Some(Kind::Content) => return (core.to_owned(), Pending::Quoted),
                None => {}
            }
        }
        if core.eq_ignore_ascii_case("bearer") || core.eq_ignore_ascii_case("basic") {
            return (core.to_owned(), Pending::Value(Kind::Secret));
        }
        let (text, next) = self.atom(core);
        let ended = trail.starts_with([':', ',', ';']);
        (text, if ended { Pending::None } else { next })
    }

    fn atom(&mut self, atom: &str) -> (String, Pending) {
        if has_secret_prefix(atom) {
            return ("<secret>".into(), Pending::None);
        }
        if is_email(atom) {
            return ("<email>".into(), Pending::None);
        }
        if atom.starts_with("S-1-") && atom.len() >= 12 {
            return ("<sid>".into(), Pending::None);
        }
        if let Some(rest) = atom.strip_prefix("//") {
            if host_part(rest).is_some() {
                return ("<url>".into(), Pending::None);
            }
        }
        if is_path_start(atom) {
            let replaced = self.path(atom);
            // Folder names with spaces in them run on into the next words,
            // except in routes, which have none.
            let named = (!atom.starts_with('/') || atom.starts_with("/Volumes/"))
                && atom
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|name| !name.contains('.'));
            let next = if replaced == "<path>" && named {
                Pending::Path(PATH_CONTINUATION)
            } else {
                Pending::None
            };
            return (replaced, next);
        }
        if is_address(atom) {
            return ("<ip>".into(), Pending::None);
        }
        if host_part(atom).is_some() {
            return ("<host>".into(), Pending::None);
        }
        (self.segments(atom), Pending::None)
    }

    /// Names along a path, or a lone name: random-looking ones are aliased.
    fn segments(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for segment in text.split_inclusive(['/', '\\']) {
            let name = segment.trim_end_matches(['/', '\\']);
            out.push_str(&self.token(name));
            out.push_str(&segment[name.len()..]);
        }
        out
    }

    /// Long random-looking strings stand for a thing the person's machine
    /// made up; the same one reads the same everywhere in the report.
    fn token(&mut self, atom: &str) -> String {
        if is_extension_id(atom) {
            self.alias("extension", atom)
        } else if is_uuid(atom) || is_long_token(atom) {
            self.alias("id", atom)
        } else {
            atom.to_owned()
        }
    }

    fn alias(&mut self, kind: &'static str, value: &str) -> String {
        let known = self.aliases.len();
        let number = *self
            .aliases
            .entry((kind, value.to_owned()))
            .or_insert(known + 1);
        if number > 999 {
            format!("<{kind}>")
        } else {
            format!("<{kind}{number}>")
        }
    }

    /// A path stays when it is somewhere the system or Zephium itself keeps
    /// things; anywhere else it names the person's files.
    fn path(&mut self, path: &str) -> String {
        if !is_shared_location(path) {
            return "<path>".into();
        }
        self.segments(path)
    }
}

/// What follows a name whose `=` stands apart from it.
fn assigned(kind: Kind, value: &str) -> (String, Pending) {
    if value.is_empty() {
        return ("=".into(), Pending::Value(kind));
    }
    match kind {
        _ if benign_value(value) => (format!("={value}"), Pending::None),
        Kind::Secret => ("=<secret>".into(), Pending::None),
        Kind::Content => ("=<redacted>".into(), Pending::Fields),
    }
}

fn swallowing(pending: Pending) -> bool {
    matches!(pending, Pending::Rest | Pending::Fields | Pending::Path(_))
}

fn placeholder(kind: Kind) -> &'static str {
    match kind {
        Kind::Secret => "<secret>",
        Kind::Content => "<redacted>",
    }
}

fn cap(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn shorten(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    format!("{} …", cap(text, max).trim_end())
}

fn clean(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            c if c.is_control() => Some(' '),
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}' => None,
            c => Some(c),
        })
        .collect::<String>()
        .replace("```", "'''")
}

fn replace_home(text: &str, home: &str) -> String {
    let (hay, needle) = (text.as_bytes(), home.as_bytes());
    let mut out = String::with_capacity(text.len());
    let (mut last, mut at) = (0, 0);
    while at + needle.len() <= hay.len() {
        let rest = &hay[at + needle.len()..];
        if hay[at..at + needle.len()].eq_ignore_ascii_case(needle)
            && rest
                .first()
                .is_none_or(|next| b"/\\\"' \t:,;)]}>|".contains(next))
        {
            out.push_str(&text[last..at]);
            out.push('~');
            at += needle.len();
            last = at;
        } else {
            at += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Another account's folder, as a CI runner's paths or a shared machine's
/// would carry it: `/Users/name`, `C:\Users\name`.
fn replace_other_homes(text: &str) -> String {
    const MARKERS: &[&str] = &["/users/", "\\users\\", "\\\\users\\\\"];
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let found = MARKERS
            .iter()
            .filter_map(|marker| {
                lower[at..]
                    .find(marker)
                    .map(|offset| (at + offset, marker.len()))
            })
            .min();
        let Some((start, marker)) = found else {
            break;
        };
        let before = start.checked_sub(1).map(|index| text.as_bytes()[index]);
        let drive = start >= 2 + at
            && text.as_bytes()[start - 1] == b':'
            && text.as_bytes()[start - 2].is_ascii_alphabetic();
        let name_start = start + marker;
        if !(drive || before.is_none_or(|byte| b" \t\"'([{<=,;:|>".contains(&byte))) {
            out.push_str(&text[at..name_start]);
            at = name_start;
            continue;
        }
        let name_len = text[name_start..]
            .find(|c: char| "/\\\"' \t<>|?*:,;)]}".contains(c))
            .unwrap_or(text.len() - name_start);
        if name_len == 0 {
            out.push_str(&text[at..name_start]);
            at = name_start;
            continue;
        }
        let from = if drive { start - 2 } else { start };
        out.push_str(&text[at..from]);
        out.push('~');
        at = name_start + name_len;
    }
    out.push_str(&text[at..]);
    out
}

fn replace_word(text: &str, word: &str, with: &str) -> String {
    let (hay, needle) = (text.as_bytes(), word.as_bytes());
    let mut out = String::with_capacity(text.len());
    let (mut last, mut at) = (0, 0);
    while at + needle.len() <= hay.len() {
        let before = at == 0 || !hay[at - 1].is_ascii_alphanumeric();
        let after = hay
            .get(at + needle.len())
            .is_none_or(|next| !next.is_ascii_alphanumeric());
        if before && after && hay[at..at + needle.len()].eq_ignore_ascii_case(needle) {
            out.push_str(&text[last..at]);
            out.push_str(with);
            at += needle.len();
            last = at;
        } else {
            at += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

fn opens_quote(bytes: &[u8], at: usize) -> bool {
    match bytes[at] {
        b'"' => true,
        b'\'' => at == 0 || !bytes[at - 1].is_ascii_alphanumeric(),
        _ => false,
    }
}

/// The end of the inside of a quoted string and where reading goes on.
fn quote_span(bytes: &[u8], at: usize) -> (usize, usize) {
    let quote = bytes[at];
    let mut end = at + 1;
    while end < bytes.len() {
        match bytes[end] {
            b'\\' => end += 2,
            byte if byte == quote => return (end, end + 1),
            _ => end += 1,
        }
    }
    (bytes.len(), bytes.len())
}

fn chunk_end(bytes: &[u8], at: usize) -> usize {
    let mut end = at;
    while end < bytes.len() {
        let byte = bytes[end];
        if byte.is_ascii_whitespace()
            || byte == b'"'
            || (byte == b'\'' && end > at && !bytes[end - 1].is_ascii_alphanumeric())
        {
            break;
        }
        end += 1;
    }
    end
}

fn peel(chunk: &str) -> (&str, &str, &str) {
    let rest = chunk.trim_start_matches(['(', '[', '{', '<', ',', ';']);
    let lead = chunk.len() - rest.len();
    let core = rest.trim_end_matches([')', ']', '}', '>', ',', ';', '.', ':', '!', '?']);
    (&chunk[..lead], core, &rest[core.len()..])
}

fn ident_suffix(text: &str) -> &str {
    let start = text
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
        .map_or(0, |(at, c)| at + c.len_utf8());
    &text[start..]
}

fn is_field_start(chunk: &str) -> bool {
    chunk.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    })
}

fn normalized(key: &str) -> String {
    key.chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn key_kind(key: &str) -> Option<Kind> {
    if key.is_empty() {
        return None;
    }
    let name = normalized(key);
    if SECRET_KEYS.contains(&name.as_str())
        || SECRET_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
    {
        return Some(Kind::Secret);
    }
    let singular = name.strip_suffix('s').unwrap_or(&name);
    (CONTENT_KEYS.contains(&name.as_str()) || CONTENT_KEYS.contains(&singular))
        .then_some(Kind::Content)
}

/// A value that carries no secret and is worth keeping: a count, a flag, or
/// something already taken out.
fn benign_value(value: &str) -> bool {
    let value = value.trim_end_matches([',', ';', ')', ']', '}']);
    (value.len() <= 40
        && value.bytes().any(|b| b.is_ascii_digit())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'.')))
        || [
            "true", "false", "none", "null", "some", "ok", "yes", "no", "on", "off", "redacted",
        ]
        .iter()
        .any(|word| value.eq_ignore_ascii_case(word))
        || (value.starts_with('<') && value.ends_with('>'))
}

/// `"name": value`: the quoted string just read names a field.
fn json_key(text: &str, at: usize, inner: &str) -> Option<(usize, Kind)> {
    let kind = key_kind(inner)?;
    let rest = &text[at..];
    let colon = rest.len() - rest.trim_start_matches(' ').len();
    rest[colon..]
        .starts_with(':')
        .then_some((at + colon + 1, kind))
}

fn scheme_start(core: &str, colon: usize) -> usize {
    core[..colon]
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
        .map_or(0, |(at, c)| at + c.len_utf8())
}

fn special_scheme(core: &str) -> Option<usize> {
    let start = core.find('=').map_or(0, |at| at + 1);
    let lower = core[start..].to_ascii_lowercase();
    SPECIAL_SCHEMES
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        .then_some(start)
}

fn has_secret_prefix(atom: &str) -> bool {
    let token =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '=' | '+' | '/');
    SECRET_PREFIXES.iter().any(|prefix| {
        atom.len() >= prefix.len() + 8
            && atom.starts_with(prefix)
            && atom[prefix.len()..].chars().all(token)
    }) || (atom.len() >= 30 && atom.starts_with("eyJ") && atom.matches('.').count() == 2)
}

fn is_email(atom: &str) -> bool {
    atom.rsplit_once('@')
        .is_some_and(|(local, domain)| !local.is_empty() && looks_like_host(domain))
}

fn is_path_start(atom: &str) -> bool {
    let bytes = atom.as_bytes();
    if atom.starts_with("~/") || atom.starts_with("~\\") {
        return atom.len() > 2;
    }
    if atom.starts_with("\\\\") {
        return atom.len() > 3;
    }
    if bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return matches!(bytes[2], b'\\' | b'/');
    }
    bytes.len() > 3
        && bytes[0] == b'/'
        && (bytes[1].is_ascii_alphanumeric() || matches!(bytes[1], b'_' | b'.' | b'~'))
        && atom[1..].contains('/')
}

fn ends_path(chunk: &str) -> bool {
    let (_, core, trail) = peel(chunk);
    trail.starts_with([':', ',', ';'])
        || core.contains(['/', '\\'])
        || core
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| name.trim_start_matches('.').contains('.'))
}

/// Folders the system or Zephium keep their own things in. Names there are
/// not the person's.
fn is_shared_location(path: &str) -> bool {
    const UNIX: &[&str] = &[
        "/system/",
        "/usr/",
        "/library/",
        "/applications/",
        "/opt/",
        "/bin/",
        "/sbin/",
        "/etc/",
        "/dev/",
        "/private/etc/",
        "~/library/application",
        "~/library/logs/",
        "~/library/caches/",
        "~/library/preferences/",
        "~/library/webkit/",
        "~/library/saved",
        "~/library/httpstorages/",
    ];
    let mut lower = path.to_ascii_lowercase().replace('/', "\\");
    while lower.contains("\\\\") {
        lower = lower.replace("\\\\", "\\");
    }
    if lower.as_bytes().get(1) == Some(&b':') {
        let rest = &lower[2..];
        return rest.starts_with("\\windows\\") || rest.starts_with("\\program");
    }
    if let Some(rest) = lower.strip_prefix("~\\appdata\\") {
        return ["roaming\\", "local\\", "locallow\\"]
            .iter()
            .any(|folder| rest.starts_with(folder));
    }
    let unix = lower.replace('\\', "/");
    UNIX.iter().any(|prefix| unix.starts_with(prefix))
}

fn is_address(atom: &str) -> bool {
    let host = atom
        .split_once(':')
        .filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(atom, |(host, _)| host);
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.len() <= 3 && part.parse::<u8>().is_ok())
    {
        return true;
    }
    let bare = atom.trim_matches(['[', ']']);
    let colons = bare.matches(':').count();
    let mac = bare.len() == 17
        && colons == 5
        && bare
            .split(':')
            .all(|pair| pair.len() == 2 && pair.bytes().all(|b| b.is_ascii_hexdigit()));
    mac || (colons >= 2
        && (bare.contains("::") || colons >= 5)
        && bare.bytes().any(|b| b.is_ascii_hexdigit())
        && bare
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.'))
}

/// A name that ends in a real top-level domain, with the path or port a
/// browser would add; a source location such as `lib.rs:12:3` is not one.
fn host_part(atom: &str) -> Option<&str> {
    let head = atom.split('/').next()?;
    let mut pieces = head.split(':');
    let name = pieces.next()?;
    pieces
        .all(|piece| {
            !piece.is_empty() && piece.len() <= 5 && piece.bytes().all(|b| b.is_ascii_digit())
        })
        .then_some(())?;
    looks_like_host(name).then_some(name)
}

fn looks_like_host(name: &str) -> bool {
    let labels: Vec<&str> = name.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let valid = |label: &&str| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_alphanumeric() || c == '-')
    };
    if !labels.iter().all(valid) {
        return false;
    }
    let last = labels[labels.len() - 1].to_lowercase();
    if last.starts_with("xn--") {
        return true;
    }
    if !last.chars().all(char::is_alphabetic) || !(2..=24).contains(&last.chars().count()) {
        return false;
    }
    !last.is_ascii()
        || TLDS.contains(&last.as_str())
        || (last.len() == 2 && !NOT_COUNTRIES.contains(&last.as_str()))
}

fn is_extension_id(atom: &str) -> bool {
    atom.len() == 32 && atom.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

fn is_uuid(atom: &str) -> bool {
    let parts: Vec<&str> = atom.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(length, part)| {
            part.len() == *length && part.bytes().all(|b| b.is_ascii_hexdigit())
        })
}

fn is_long_token(atom: &str) -> bool {
    if atom.len() < 24
        || !atom
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'=' | b'+'))
    {
        return false;
    }
    let digits = atom.bytes().any(|b| b.is_ascii_digit());
    let letters = atom.bytes().any(|b| b.is_ascii_alphabetic());
    if !(digits && letters) {
        return false;
    }
    // snake_case and kebab-case names with at most a digit or two in a word.
    let name = atom
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
        && atom
            .split(['_', '-'])
            .all(|word| word.bytes().filter(u8::is_ascii_digit).count() <= 2);
    !name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor() -> Redactor {
        Redactor::new(
            ["/Users/jane".to_owned(), r"C:\Users\Jane Doe".to_owned()],
            Some("jane".into()),
        )
    }

    fn clean(line: &str) -> String {
        redactor().line(line)
    }

    #[track_caller]
    fn gone(line: &str, private: &[&str]) -> String {
        let out = clean(line);
        for text in private {
            assert!(
                !out.contains(text),
                "{text:?} survived in {out:?} (from {line:?})"
            );
        }
        out
    }

    #[test]
    fn addresses_in_every_shape_are_replaced() {
        for line in [
            "navigated to https://bank.example/accounts/4417?session=abc&q=cats",
            "url=https://news.example/story/secret-title",
            "(https://hidden.example/x), then more",
            "request \"http://192.168.1.20:8080/admin\" failed",
            "file:///Users/jane/Documents/taxes.pdf",
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop/popup.html",
            "zephium://settings/secret",
            "blob:https://video.example/6f1a2b3c",
            "data:text/html;base64,PGgxPnNlY3JldDwvaDE+",
            "href=mailto:jane@mail.example",
            "javascript:alert(document.cookie)",
            "ftp://files.example/private.zip",
            "see //cdn.tracker.example/lib.js",
            "error sending request for url (https://api.provider.example/v1/chat): timed out",
        ] {
            let out = gone(
                line,
                &[
                    "bank.example",
                    "news.example",
                    "hidden.example",
                    "192.168",
                    "Documents",
                    "taxes",
                    "abcdefghijklmnop",
                    "secret",
                    "video.example",
                    "PGgx",
                    "mail.example",
                    "alert",
                    "files.example",
                    "tracker.example",
                    "provider.example",
                ],
            );
            assert!(out.contains("<url>"), "{out}");
        }
        assert_eq!(
            clean(
                "error sending request for url (https://api.provider.example/v1/chat): timed out"
            ),
            "error sending request for url (<url>): timed out"
        );
    }

    #[test]
    fn bare_hosts_addresses_and_emails_are_replaced() {
        let out = gone(
            "dns failed for api.openai.com:443, www.example.org, shop.example.xyz, macbook.local, 10.0.0.7, [fe80::1c2d:3e4f], aa:bb:cc:dd:ee:ff",
            &["openai", "example", "macbook", "10.0.0.7", "fe80", "aa:bb"],
        );
        assert!(out.contains("<host>") && out.contains("<ip>"), "{out}");
        let out = gone(
            "sent to jane@example.com and <jane.doe+news@sub.gmail.com>, email=\"x@y.co\"",
            &["jane", "example", "gmail", "x@y"],
        );
        assert!(out.contains("<email>"), "{out}");
    }

    #[test]
    fn what_looks_like_an_address_but_is_not_one_stays() {
        for line in [
            "desktop/src/lib.rs:123:45 panicked",
            "version 1.0.3 on WebView2 156.0.4314.42",
            "icon@2x.png missing",
            "table bookmarks.url has no column",
            "loaded com.apple.WebKit.Networking in 0.25ms at 14:03:21.123",
            "std::io::Error kind NotFound code 2",
            "Info.plist and libfoo.dylib and cmd.exe",
        ] {
            assert_eq!(clean(line), line);
        }
    }

    #[test]
    fn keys_tokens_and_passwords_are_replaced_in_every_form() {
        for line in [
            "provider: key sk-proj-AbCdEf0123456789AbCdEf0123456789 refused",
            "vendor sk-ant-api03-ZZZZZZZZZZZZZZZZZZZZ rejected",
            "Authorization: Bearer abcdef0123456789",
            "authorization=Basic dXNlcjpwYXNzd29yZA==",
            "api_key=hunter2hunter2 model=x",
            "apiKey=hunter2hunter2",
            "x-api-key: hunter2hunter2",
            "{\"token\": \"hunter2hunter2\", \"model\": \"x\"}",
            "password=hunter2 user=bob",
            "Cookie: sid=hunter2hunter2; theme=dark",
            "Set-Cookie: session=hunter2hunter2",
            "refresh_token=\"hunter2hunter2\"",
            "github ghp_AbCdEf0123456789AbCdEf0123456789 revoked",
            "aws AKIAIOSFODNN7EXAMPLE used",
            "google AIzaSyA-1234567890abcdefghijklmnopqrstuv",
            "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r",
            "bearer hunter2hunter2",
            "cache 0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c",
            "secret_key=ZmFrZS1zZWNyZXQ=",
        ] {
            gone(
                line,
                &[
                    "hunter2",
                    "AbCdEf",
                    "ZZZZ",
                    "abcdef0123",
                    "dXNlcjpw",
                    "IOSFODNN",
                    "1234567890abcdef",
                    "eyJ",
                    "0f1e2d3c",
                    "ZmFrZS1",
                ],
            );
        }
        assert_eq!(
            clean("password=hunter2 user=bob"),
            "password=<secret> user=<redacted>"
        );
        assert_eq!(
            clean("Authorization: Bearer abc"),
            "Authorization: <secret>"
        );
    }

    #[test]
    fn a_sign_set_apart_from_its_name_still_hides_the_value() {
        assert_eq!(
            clean("config password = hunter2 mode = fast"),
            "config password = <secret> mode = fast"
        );
        assert_eq!(clean("api_key =sk-abcdefgh1234 ok"), "api_key =<secret> ok");
        assert_eq!(clean("a = b"), "a = b");
    }

    #[test]
    fn international_addresses_are_replaced_too() {
        let out = gone(
            "lookup bücher.de and пример.рф and xn--bcher-kva.example failed",
            &["bücher", "пример", "xn--"],
        );
        assert_eq!(out, "lookup <host> and <host> and <host> failed");
    }

    #[test]
    fn counts_and_flags_beside_a_secret_looking_name_stay() {
        let line = "work: tokens=1500 max_tokens=4096 has_token=true missing_key=false key=7 session=1792627200000-4242";
        assert_eq!(clean(line), line);
    }

    #[test]
    fn home_folders_become_a_tilde_on_both_systems() {
        assert_eq!(
            clean("log at /Users/jane/Library/Logs/app.zephium/zephium.log"),
            "log at ~/Library/Logs/app.zephium/zephium.log"
        );
        assert_eq!(
            clean(r"opened C:\Users\Jane Doe\AppData\Roaming\app.zephium\zephium.db"),
            r"opened ~\AppData\Roaming\app.zephium\zephium.db"
        );
        assert_eq!(
            clean(
                r#"Os { code: 3, message: "no C:\\Users\\Jane Doe\\AppData\\Local\\app.zephium\\EBWebView" }"#
            ),
            r#"Os { code: 3, message: "no ~\\AppData\\Local\\app.zephium\\EBWebView" }"#
        );
        assert_eq!(
            clean(r"cwd \\?\C:\Users\Jane Doe\AppData\Roaming\app.zephium"),
            r"cwd ~\AppData\Roaming\app.zephium"
        );
        assert_eq!(
            clean(r"c:\users\JANE DOE\appdata\local\temp\x.tmp"),
            r"~\appdata\local\temp\x.tmp"
        );
        assert_eq!(
            clean("backtrace at /Users/runner/.cargo/registry/src/x-1.0/src/lib.rs:5"),
            "backtrace at <path>"
        );
        assert_eq!(clean(r"D:\Users\bob\Desktop\x.txt gone"), "<path> gone");
    }

    #[test]
    fn files_outside_the_places_apps_keep_theirs_are_not_named() {
        for line in [
            "saved to /Users/jane/Documents/Taxes 2025/return.pdf",
            "saved to ~/Downloads/passport-scan.png",
            r"imported D:\Clients\Acme\plan.docx",
            r"share \\fileserver\finance\q3.xlsx",
            "volume /Volumes/Jane's Backup/notes/todo.txt",
            "opening '/Users/jane/Desktop/My Secret Plans/draft one.txt' failed",
            "opening /Users/jane/Documents/My Secret Plans/draft.txt failed",
            r#"path: "C:\\Users\\Jane Doe\\Documents\\Private Notes\\a.txt""#,
        ] {
            gone(
                line,
                &[
                    "Taxes",
                    "return.pdf",
                    "passport",
                    "Acme",
                    "plan.docx",
                    "fileserver",
                    "finance",
                    "Backup",
                    "todo",
                    "Secret",
                    "draft",
                    "Private",
                    "Jane",
                    "jane",
                ],
            );
        }
        assert_eq!(
            clean("saved to /Users/jane/Documents/Taxes 2025/return.pdf done"),
            "saved to <path> done"
        );
    }

    #[test]
    fn the_places_apps_keep_their_own_files_stay_readable() {
        for line in [
            "from /Applications/Zephium.app/Contents/MacOS/Zephium",
            "loaded /System/Library/Frameworks/WebKit.framework/WebKit",
            r"module C:\Windows\System32\kernel32.dll",
            r"module C:\Program Files\Zephium\zephium.exe loaded",
        ] {
            let out = clean(line);
            assert!(
                !out.contains("<path>") || line.contains("Program Files"),
                "{out}"
            );
        }
        assert_eq!(
            clean("data in /Users/jane/Library/Application Support/app.zephium/zephium.db"),
            "data in ~/Library/Application Support/app.zephium/zephium.db"
        );
        assert_eq!(
            clean("data in ~/Library/Application Support/app.zephium/webext/packages/abcdefghijklmnopabcdefghijklmnop/1.0.crx"),
            "data in ~/Library/Application Support/app.zephium/webext/packages/<extension1>/1.0.crx"
        );
    }

    #[test]
    fn what_a_person_typed_or_visited_is_replaced_by_its_name() {
        for line in [
            "work: phase=lead title=\"My bank - Login\" state=ok",
            "page title='Dr. Smith - Oncology' loaded",
            "search query=cats and dogs phase=lead",
            "Page { title: \"Medical results\", url: \"x\", host: \"clinic\" }",
            "{\"title\":\"Medical results\",\"count\":3}",
            "site=github-private-repo name=Secret Project",
            "input=how to leave my job next=1",
        ] {
            gone(
                line,
                &[
                    "bank",
                    "Login",
                    "Smith",
                    "Oncology",
                    "cats",
                    "dogs",
                    "Medical",
                    "clinic",
                    "github-private",
                    "Secret",
                    "Project",
                    "leave",
                    "my job",
                ],
            );
        }
        assert_eq!(
            clean("search query=cats and dogs phase=lead"),
            "search query=<redacted> phase=lead"
        );
        assert_eq!(
            clean("work: phase=lead engine=duckduckgo content=redacted"),
            "work: phase=lead engine=duckduckgo content=redacted"
        );
    }

    #[test]
    fn prose_that_happens_to_name_a_field_is_left_readable() {
        for line in [
            "engine: cannot secure profile data directory: Permission denied (os error 13)",
            "store: cannot open file: No such file or directory",
            "startup: page: loaded in 120 ms, site: ready",
            "Os { code: 2, kind: NotFound, message: \"No such file or directory\" }",
        ] {
            assert_eq!(clean(line), line);
        }
    }

    #[test]
    fn queries_and_routes_are_not_left_in_a_request_line() {
        assert_eq!(
            clean("GET /api/v1/users/12345/messages?folder=inbox sent"),
            "GET <path> sent"
        );
        assert_eq!(clean("params q=cats&lang=en"), "params q=<redacted>");
        assert_eq!(
            clean("params lang=en&x=cats ok"),
            "params lang=<redacted> ok"
        );
        assert_eq!(clean("GET /api/users/12345/messages"), "GET <path>");
    }

    #[test]
    fn the_account_name_and_machine_made_ids_are_not_shown() {
        assert_eq!(clean("hello jane, welcome"), "hello <user>, welcome");
        assert_eq!(clean("hello Jane Doe!"), "hello <user> <user>!");
        assert_eq!(clean("janet and Jane's"), "janet and <user>'s");
        let mut redactor = redactor();
        redactor.keep("882c92d8f1a2b3c4d5e6f708192a3b4c5d6e7f80+dirty");
        assert_eq!(
            redactor.line("build=882c92d8f1a2b3c4d5e6f708192a3b4c5d6e7f80+dirty"),
            "build=882c92d8f1a2b3c4d5e6f708192a3b4c5d6e7f80+dirty"
        );
        let out = redactor.line(
            "store: profile 01J9ZK3Q4R5S6T7V8W9X0Y1Z2A a b 01J9ZK3Q4R5S6T7V8W9X0Y1Z2B c 01J9ZK3Q4R5S6T7V8W9X0Y1Z2A \
             ext abcdefghijklmnopabcdefghijklmnop vol 3f2504e0-4f89-41d3-9a0c-0305e82c3301",
        );
        assert_eq!(
            out,
            "store: profile <id1> a b <id2> c <id1> ext <extension3> vol <id4>"
        );
    }

    #[test]
    fn foreign_lines_are_cleaned_of_control_text_and_bounded() {
        let out = clean("\u{1b}[31mred\u{1b}[0m\ttab\r\u{202e}evil\u{200b}zero");
        assert_eq!(out, " [31mred [0m tab evilzero");
        assert_eq!(clean("```text\nescape"), "'''text escape");
        let long = clean(&format!("start {} end", "word ".repeat(2000)));
        assert!(
            long.len() <= MAX_LINE + 4 && long.ends_with('…'),
            "{}",
            long.len()
        );
        let unicode = clean(&"é".repeat(5000));
        assert!(unicode.len() <= MAX_LINE + 4);
    }

    #[test]
    fn a_whole_nslog_line_with_a_page_in_it_loses_the_page() {
        let out = gone(
            "2026-10-25 14:03:21.123 Zephium[4242:99887] [Process] Failed to load https://mail.example.com/inbox?user=jane@example.com with Error Domain=NSURLErrorDomain Code=-1009 \"The Internet connection appears to be offline.\" UserInfo={NSErrorFailingURLStringKey=https://mail.example.com/inbox, NSLocalizedDescription=The Internet connection appears to be offline.}",
            &["mail.example", "jane", "inbox"],
        );
        assert!(out.contains("NSURLErrorDomain Code=-1009"), "{out}");
        assert!(
            out.contains("The Internet connection appears to be offline."),
            "{out}"
        );
    }

    #[test]
    fn the_lines_the_app_writes_itself_read_as_written() {
        for line in [
            "ts_ms=1792627200000 session=1792627200000-4242 startup: WebView2 runtime selected at launch: 156.0.4314.42",
            "ts_ms=1792627200000 session=1792627200000-4242 view-create: resident ceiling live=3 limit=8",
            "ts_ms=1792627200000 session=1792627200000-4242 chrome: recovering main renderer, attempt 2",
            "ts_ms=1792627200000 session=1792627200000-4242 shutdown: clean proof incomplete terminal=true storage=false native=true",
            "ts_ms=1792627200000 session=1792627200000-4242 webview: main content process 5150",
            "ts_ms=1792627200000 session=1792627200000-4242 engine: web process terminated: Crashed",
            "ts_ms=1792627200000 session=1792627200000-4242 voice: take=3 ended=Cancelled",
            "ts_ms=1792627200000 session=1792627200000-4242 work: phase=page event=terminal_refused stage=scoped_drain; content=redacted",
        ] {
            assert_eq!(clean(line), line);
        }
    }

    #[test]
    fn hostile_input_never_panics_or_runs_away() {
        let pieces = [
            "\"",
            "'",
            "\\",
            "=",
            ":",
            "://",
            "/",
            "~",
            "~/",
            "C:\\",
            "\\\\",
            "{",
            "}",
            "(",
            ")",
            "<",
            ">",
            "@",
            "é",
            "日本",
            " ",
            "  ",
            "key=",
            "token:",
            "title=\"",
            "Bearer ",
            "sk-",
            "https://",
            "a.b",
            "1.2.3.4",
            "::",
            "\u{0}",
            "\n",
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        ];
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let mut redactor = redactor();
        for _ in 0..3000 {
            let mut line = String::new();
            for _ in 0..(seed % 14) {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                line.push_str(pieces[(seed >> 33) as usize % pieces.len()]);
            }
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let out = redactor.line(&line);
            assert!(out.len() <= MAX_LINE + 4);
            assert!(!out.contains('\n'));
        }
        assert_eq!(redactor.line(""), "");
        assert_eq!(redactor.line("   "), "   ");
    }
}
