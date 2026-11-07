//! The queries the edit scriptlets read, checked when a rule is admitted so
//! that a rule the page would not read never reaches the program. The page's
//! reader, `object-editor.js`, accepts the same grammar: uBlock Origin's wiki,
//! "uBO flavored JSONPath", v2 without `call()`, and the older form today's
//! YouTube rules use, read from those rules. A regular expression JavaScript
//! cannot compile passes here and makes the rule do nothing in the page.

const MAX_QUERY_BYTES: usize = 4096;
const UNSAFE: [&str; 3] = ["__proto__", "constructor", "prototype"];
const LEGACY_OPERATORS: &[&str] = &["*=", "=="];
const V2_OPERATORS: &[&str] = &["==", "!=", "<=", ">=", "<", ">", "^=", "$=", "*="];

/// Whether the page's reader accepts `query`.
pub(crate) fn accepts(query: &str) -> bool {
    if query.is_empty() || query.len() > MAX_QUERY_BYTES {
        return false;
    }
    let mut parser = Parser { text: query, at: 0 };
    let parsed = if query.starts_with('$') || query.starts_with("v2:") {
        parser.v2()
    } else {
        parser.legacy()
    };
    parsed.is_some() && parser.at == query.len()
}

#[derive(Clone, Copy)]
enum Literal {
    String,
    Scalar,
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn peek(&self, token: &str) -> bool {
        self.rest().starts_with(token)
    }

    fn take(&mut self, token: &str) -> bool {
        let found = self.peek(token);
        if found {
            self.at += token.len();
        }
        found
    }

    fn expect(&mut self, token: &str) -> Option<()> {
        self.take(token).then_some(())
    }

    fn name(&mut self) -> Option<()> {
        let rest = self.rest().as_bytes();
        if !rest
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            return None;
        }
        let length = rest
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count();
        let name = &self.rest()[..length];
        if UNSAFE.contains(&name) {
            return None;
        }
        self.at += length;
        Some(())
    }

    fn json_string(&mut self) -> Option<String> {
        if !self.peek("\"") {
            return None;
        }
        let bytes = self.text.as_bytes();
        let mut end = self.at + 1;
        while end < bytes.len() && bytes[end] != b'"' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        if end >= bytes.len() {
            return None;
        }
        let value = serde_json::from_str::<String>(&self.text[self.at..=end]).ok()?;
        self.at = end + 1;
        Some(value)
    }

    fn quoted_name(&mut self) -> Option<()> {
        if self.peek("\"") {
            let value = self.json_string()?;
            return (!UNSAFE.contains(&value.as_str())).then_some(());
        }
        self.expect("'")?;
        let length = self.rest().find('\'')?;
        let value = &self.rest()[..length];
        if value.contains('\\') || UNSAFE.contains(&value) {
            return None;
        }
        self.at += length + 1;
        Some(())
    }

    fn regex(&mut self) -> Option<()> {
        self.expect("/")?;
        let bytes = self.text.as_bytes();
        let (start, mut end, mut in_class) = (self.at, self.at, false);
        while end < bytes.len() && (in_class || bytes[end] != b'/') {
            match bytes[end] {
                b'\\' => end += 1,
                b'[' => in_class = true,
                b']' => in_class = false,
                _ => {}
            }
            end += 1;
        }
        if end >= bytes.len() || end == start {
            return None;
        }
        self.at = end + 1;
        let flags = self
            .rest()
            .bytes()
            .take_while(|byte| matches!(byte, b'i' | b'm' | b's' | b'u'))
            .count();
        self.at += flags;
        Some(())
    }

    fn scalar(&mut self) -> Option<()> {
        if self.peek("\"") {
            return self.json_string().map(drop);
        }
        for word in ["true", "false", "null"] {
            if self.take(word) {
                return Some(());
            }
        }
        let bytes = self.rest().as_bytes();
        let mut length = usize::from(bytes.first() == Some(&b'-'));
        let digits = |from: usize| {
            bytes[from..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count()
        };
        let whole = digits(length);
        if whole == 0 || (whole > 1 && bytes[length] == b'0') {
            return None;
        }
        length += whole;
        if bytes.get(length) == Some(&b'.') && digits(length + 1) > 0 {
            length += 1 + digits(length + 1);
        }
        if matches!(bytes.get(length), Some(b'e' | b'E')) {
            let sign = usize::from(matches!(bytes.get(length + 1), Some(b'+' | b'-')));
            let exponent = digits(length + 1 + sign);
            if exponent > 0 {
                length += 1 + sign + exponent;
            }
        }
        self.at += length;
        Some(())
    }

    fn test(&mut self, operators: &[&str], literal: Literal) -> Option<()> {
        if self.peek("=/") {
            self.at += 1;
            return self.regex();
        }
        for operator in operators {
            if self.take(operator) {
                return match literal {
                    Literal::String => self.json_string().map(drop),
                    Literal::Scalar => self.scalar(),
                };
            }
        }
        Some(())
    }

    /// The assignment ending a query: `Some(false)` when there is none.
    fn assignment(&mut self) -> Option<bool> {
        if self.at == self.text.len() {
            return Some(false);
        }
        if self.take("+=") {
            let value = safe_json(self.rest())?;
            self.at = self.text.len();
            return value.is_object().then_some(true);
        }
        self.expect("=")?;
        if self.take("repl(") {
            let inner = self.rest().strip_suffix(')')?;
            let spec = safe_json(inner)?;
            self.at = self.text.len();
            let spec = spec.as_object()?;
            let text = |key: &str| spec.get(key).and_then(serde_json::Value::as_str);
            let valid = spec.len() == 2
                && text("replacement").is_some()
                && (text("regex").is_some_and(|regex| !regex.is_empty())
                    || text("pattern").is_some_and(|pattern| !pattern.is_empty()));
            return valid.then_some(true);
        }
        if self.peek("call(") {
            return None;
        }
        let source = self.rest();
        let value = safe_json(source)?;
        self.at = self.text.len();
        let now = value.as_str() == Some("${now}");
        (now || !source.contains("${")).then_some(true)
    }

    fn legacy(&mut self) -> Option<()> {
        let mut steps = 0;
        let mut child = false;
        loop {
            if self.take("[?") {
                self.expect(".")?;
                self.name()?;
                while self.take(".") {
                    self.name()?;
                }
                self.test(LEGACY_OPERATORS, Literal::String)?;
                self.expect("]")?;
            } else if self.peek(".") && !self.peek("..") {
                self.at += 1;
                self.name()?;
                child = true;
            } else {
                break;
            }
            steps += 1;
        }
        let merge = self.peek("+=");
        (steps > 0 && self.assignment()? && (child || merge)).then_some(())
    }

    fn selector(&mut self) -> Option<()> {
        if self.take("*") {
            return Some(());
        }
        if self.take("?") {
            self.expect("@")?;
            loop {
                if self.peek(".") && !self.peek("..") {
                    self.at += 1;
                    self.name()?;
                } else if self.take("[") {
                    self.quoted_name()?;
                    self.expect("]")?;
                } else {
                    break;
                }
            }
            return self.test(V2_OPERATORS, Literal::Scalar);
        }
        if self.peek("/") {
            return self.regex();
        }
        let bytes = self.rest().as_bytes();
        let sign = usize::from(bytes.first() == Some(&b'-'));
        let digits = bytes[sign..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits > 0 {
            self.at += sign + if bytes[sign] == b'0' { 1 } else { digits };
            return Some(());
        }
        self.quoted_name()?;
        while self.take(",") {
            self.quoted_name()?;
        }
        Some(())
    }

    /// One `$` query, whether it ends with a quantifier, and whether it
    /// reaches below the root.
    fn v2_query(&mut self) -> Option<(bool, bool)> {
        self.expect("$")?;
        let mut child = false;
        loop {
            if !self.take("..") && !self.take(".") && !self.peek("[") {
                break;
            }
            if self.take("[") {
                self.selector()?;
                self.expect("]")?;
            } else if self.take("*") {
            } else if self.peek("/") {
                self.regex()?;
            } else {
                self.name()?;
            }
            child = true;
        }
        let rest = self.rest();
        let Some(inner) = rest
            .strip_prefix('{')
            .and_then(|rest| rest.split_once('}'))
            .map(|(inner, _)| inner)
        else {
            return Some((false, child));
        };
        let (low, high) = inner.split_once(',').unwrap_or((inner, ""));
        let numeric = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
        if !numeric(low) || !numeric(high) || (low.is_empty() && high.is_empty()) {
            return Some((false, child));
        }
        self.at += inner.len() + 2;
        Some((true, child))
    }

    /// An assignment that replaces a value needs a value below the root to
    /// replace; one that merges into an object may name the root.
    fn v2(&mut self) -> Option<()> {
        self.take("v2:");
        let (mut bounded, mut child) = self.v2_query()?;
        while self.take(";") {
            (bounded, child) = self.v2_query()?;
        }
        if bounded {
            return None;
        }
        let set = self.at < self.text.len() && !self.peek("+=");
        self.assignment()?;
        (child || !set).then_some(())
    }
}

/// A JSON value with no key that could reach an object's prototype.
fn safe_json(source: &str) -> Option<serde_json::Value> {
    fn safe(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => map
                .iter()
                .all(|(key, value)| !UNSAFE.contains(&key.as_str()) && safe(value)),
            serde_json::Value::Array(items) => items.iter().all(safe),
            _ => true,
        }
    }
    let value = serde_json::from_str(source).ok()?;
    safe(&value).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::accepts;

    #[derive(serde::Deserialize)]
    struct Queries {
        accepted: Vec<String>,
        rejected: Vec<String>,
    }

    #[test]
    fn the_compiler_reads_queries_as_the_page_does() {
        let queries: Queries = serde_json::from_str(include_str!(
            "../../../../scripts/blocker-scriptlets/edit-queries.json"
        ))
        .unwrap();
        for query in &queries.accepted {
            assert!(accepts(query), "{query}");
        }
        for query in &queries.rejected {
            assert!(!accepts(query), "{query}");
        }
    }
}
