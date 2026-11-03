//! `!#if` blocks in filter lists, evaluated as uBlock Origin does on Chromium
//! (its "Static filter syntax" wiki page): `!#if` takes tokens joined by `!`,
//! `&&`, `||` and parentheses, `!#else` flips the innermost block, `!#endif`
//! closes it. Unknown tokens and malformed conditions are false, so their rules
//! stay out. WebKit has no response filtering either, so both platforms use the
//! same tokens.

const TRUE_TOKENS: [&str; 2] = ["env_chromium", "cap_user_stylesheet"];
const MAX_DEPTH: usize = 64;

/// Tracks the open `!#if` blocks of one list, line by line.
#[derive(Debug, Default)]
pub struct ListConditions {
    /// Each open block's value, and whether `!#else` may flip it.
    blocks: Vec<(bool, bool)>,
}

impl ListConditions {
    /// Whether `line` (trimmed) is a rule to keep. Directive lines and lines
    /// inside a false block are not.
    pub fn keep(&mut self, line: &str) -> bool {
        if let Some(condition) = line.strip_prefix("!#if ") {
            // Past the depth limit a block stays false, else and all.
            let within = self.blocks.len() < MAX_DEPTH;
            self.blocks.push((within && evaluate(condition), within));
            return false;
        }
        if line == "!#else" || line.starts_with("!#else ") {
            if let Some((value, true)) = self.blocks.last_mut() {
                *value = !*value;
            }
            return false;
        }
        if line == "!#endif" || line.starts_with("!#endif ") {
            self.blocks.pop();
            return false;
        }
        self.active()
    }

    /// Whether lines here are in effect.
    pub fn active(&self) -> bool {
        self.blocks.iter().all(|(value, _)| *value)
    }
}

fn evaluate(condition: &str) -> bool {
    let tokens = tokenize(condition);
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
    };
    match parser.or() {
        Some(value) if parser.at == tokens.len() => value,
        _ => false,
    }
}

#[derive(Debug, PartialEq)]
enum Token<'a> {
    Name(&'a str),
    Not,
    And,
    Or,
    Open,
    Close,
    Invalid,
}

fn tokenize(condition: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let bytes = condition.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let rest = &condition[at..];
        let (token, length) = match bytes[at] {
            b' ' | b'\t' => {
                at += 1;
                continue;
            }
            b'!' => (Token::Not, 1),
            b'(' => (Token::Open, 1),
            b')' => (Token::Close, 1),
            b'&' if rest.starts_with("&&") => (Token::And, 2),
            b'|' if rest.starts_with("||") => (Token::Or, 2),
            byte if byte.is_ascii_alphanumeric() || byte == b'_' => {
                let length = rest
                    .bytes()
                    .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                    .count();
                (Token::Name(&rest[..length]), length)
            }
            _ => (Token::Invalid, 1),
        };
        tokens.push(token);
        at += length;
    }
    tokens
}

struct Parser<'t, 'a> {
    tokens: &'t [Token<'a>],
    at: usize,
}

impl Parser<'_, '_> {
    fn or(&mut self) -> Option<bool> {
        let mut value = self.and()?;
        while self.eat(&Token::Or) {
            value |= self.and()?;
        }
        Some(value)
    }

    fn and(&mut self) -> Option<bool> {
        let mut value = self.unary()?;
        while self.eat(&Token::And) {
            value &= self.unary()?;
        }
        Some(value)
    }

    fn unary(&mut self) -> Option<bool> {
        match self.tokens.get(self.at)? {
            Token::Not => {
                self.at += 1;
                Some(!self.unary()?)
            }
            Token::Open => {
                self.at += 1;
                let value = self.or()?;
                self.eat(&Token::Close).then_some(value)
            }
            Token::Name(name) => {
                self.at += 1;
                Some(TRUE_TOKENS.contains(name))
            }
            _ => None,
        }
    }

    fn eat(&mut self, token: &Token<'_>) -> bool {
        let matched = self.tokens.get(self.at) == Some(token);
        if matched {
            self.at += 1;
        }
        matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(list: &str) -> Vec<&str> {
        let mut conditions = ListConditions::default();
        list.lines()
            .map(str::trim)
            .filter(|line| conditions.keep(line))
            .collect()
    }

    #[test]
    fn blocks_follow_chromium_s_tokens() {
        let list = "a\n!#if env_chromium\nb\n!#else\nc\n!#endif\n!#if env_firefox\nd\n!#else\ne\n!#endif\nf";
        assert_eq!(kept(list), ["a", "b", "e", "f"]);
    }

    #[test]
    fn negation_grouping_and_operators_combine() {
        for (condition, expected) in [
            ("!cap_html_filtering", true),
            ("!env_mv3 && !cap_html_filtering && env_firefox", false),
            ("env_firefox || env_chromium", true),
            ("!(env_safari || env_mobile)", true),
            ("env_chromium && (ext_ubol || !env_mobile)", true),
            ("!!env_chromium", true),
            ("unknown_token", false),
        ] {
            assert_eq!(evaluate(condition), expected, "{condition}");
        }
    }

    #[test]
    fn malformed_conditions_are_false() {
        for condition in [
            "",
            "env_chromium &&",
            "(env_chromium",
            "env_chromium)",
            "a & b",
            "env-chromium",
        ] {
            assert!(!evaluate(condition), "{condition}");
        }
    }

    #[test]
    fn nested_blocks_need_every_level_true() {
        let list = "!#if env_chromium\n!#if env_mobile\na\n!#endif\nb\n!#endif\nc";
        assert_eq!(kept(list), ["b", "c"]);
    }

    #[test]
    fn an_unmatched_else_or_endif_changes_nothing() {
        assert_eq!(kept("!#endif\na\n!#else\nb"), ["a", "b"]);
    }

    #[test]
    fn blocks_past_the_depth_limit_are_false() {
        let open = "!#if env_chromium\n".repeat(MAX_DEPTH + 1);
        let list = format!("{open}a\n!#else\nb\n{}c", "!#endif\n".repeat(MAX_DEPTH + 1));
        assert_eq!(kept(&list), ["c"]);
    }
}
