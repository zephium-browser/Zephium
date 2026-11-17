//! Reply text to speakable chunks: the panel's markdown and cite marks become
//! plain sentences, cut where a listener expects a pause. The result depends
//! only on the text, never on how it was split into deltas.
use crate::ipc::{PAGE_ASSIST_CITE_CLOSE as CLOSE, PAGE_ASSIST_CITE_OPEN as OPEN};

/// Longest chunk, in characters.
pub const MAX_CHUNK_CHARS: usize = 220;
/// Words the first chunk needs before it may be released at a clause break.
pub const EARLY_WORDS: usize = 8;
const MAX_URL_CHARS: usize = 2048;
const ABBREVIATIONS: &[&str] = &[
    "approx", "dr", "e.g", "etc", "fig", "i.e", "inc", "jr", "ltd", "mr", "mrs", "ms", "mt",
    "prof", "sr", "st", "vs",
];

/// Streams one reply's text into chunks.
#[derive(Clone, Debug)]
pub struct Cutter {
    max: usize,
    raw: Vec<char>,
    line_start: bool,
    fence: bool,
    skip_line: bool,
    code: bool,
    cite: bool,
    link: u8,
    url: u8,
    space: bool,
    last: Option<char>,
    plain: Vec<char>,
    scan: usize,
    clause: Option<usize>,
    gap: Option<usize>,
    released: bool,
}

impl Default for Cutter {
    fn default() -> Self {
        Self::new(MAX_CHUNK_CHARS)
    }
}

enum Mark {
    Plain,
    Clause(usize),
    Sentence(usize),
    Hard,
}

impl Cutter {
    /// A cutter bounding chunks to `max` characters.
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(16),
            raw: Vec::new(),
            line_start: true,
            fence: false,
            skip_line: false,
            code: false,
            cite: false,
            link: 0,
            url: 0,
            space: false,
            last: None,
            plain: Vec::new(),
            scan: 0,
            clause: None,
            gap: None,
            released: false,
        }
    }

    /// Adds a text delta; returns the chunks it completes.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.raw.extend(delta.chars());
        self.clean(false);
        self.cut(false)
    }

    /// The text block ended: a sentence end.
    pub fn end_block(&mut self) -> Vec<String> {
        self.clean(true);
        self.raw.clear();
        self.line_start = true;
        self.fence = false;
        self.skip_line = false;
        self.code = false;
        self.cite = false;
        self.link = 0;
        self.url = 0;
        self.out_break();
        self.cut(true)
    }

    /// The reply ended: everything still held.
    pub fn finish(&mut self) -> Vec<String> {
        self.end_block()
    }

    fn at(&self, index: usize, finished: bool) -> Option<Option<char>> {
        match self.raw.get(index) {
            Some(character) => Some(Some(*character)),
            None if finished => Some(None),
            None => None,
        }
    }

    fn clean(&mut self, finished: bool) {
        let mut index = 0;
        while index < self.raw.len() {
            match self.step(index, finished) {
                Some(next) => index = next,
                None => break,
            }
        }
        self.raw.drain(..index);
    }

    fn step(&mut self, i: usize, finished: bool) -> Option<usize> {
        let c = self.raw[i];
        if c == '\n' {
            self.code = false;
            self.cite = false;
            self.link = 0;
            self.url = 0;
            self.skip_line = false;
            self.line_start = true;
            if !self.fence {
                self.out_break();
            }
            return Some(i + 1);
        }
        if self.skip_line {
            return Some(i + 1);
        }
        if self.line_start && matches!(c, ' ' | '\t' | '\r') {
            return Some(i + 1);
        }
        if self.line_start && c == '`' {
            let ticks = [self.at(i + 1, finished)?, self.at(i + 2, finished)?];
            if ticks == [Some('`'), Some('`')] {
                self.fence = !self.fence;
                self.skip_line = true;
                self.out_break();
                return Some(i + 3);
            }
        }
        if self.fence {
            self.line_start = false;
            return Some(i + 1);
        }
        if self.cite {
            self.cite = c != CLOSE;
            return Some(i + 1);
        }
        if self.url > 0 {
            match c {
                '(' => self.url = self.url.saturating_add(1),
                ')' => self.url -= 1,
                _ => {}
            }
            return Some(i + 1);
        }
        if self.line_start {
            if let Some(next) = self.marker(i, finished)? {
                return Some(next);
            }
        }
        if self.code {
            if c == '`' {
                self.code = false;
            } else if !(matches!(c, OPEN | CLOSE | '*' | '~' | '\\') || emoji(c) || c.is_control())
            {
                self.out(c);
            }
            return Some(i + 1);
        }
        match c {
            OPEN => self.cite = true,
            CLOSE | '*' | '~' | '\\' | '^' | '<' | '>' | '\r' => {}
            '`' => self.code = true,
            '[' => self.link = self.link.saturating_add(1),
            ']' if self.link > 0 => {
                let next = self.at(i + 1, finished)?;
                self.link -= 1;
                if next == Some('(') {
                    self.url = 1;
                    return Some(i + 2);
                }
            }
            ']' => {}
            '|' | '\t' => self.out(' '),
            '_' => {
                let next = self.at(i + 1, finished)?;
                if self.last.is_some_and(char::is_alphanumeric)
                    && !self.space
                    && next.is_some_and(char::is_alphanumeric)
                {
                    self.out(' ');
                }
            }
            'h' | 'H' | 'w' | 'W' if self.word_start() => {
                if let Some(next) = self.url(i, finished)? {
                    return Some(next);
                }
                self.out(c);
            }
            c if emoji(c) || c.is_control() => {}
            c => self.out(c),
        }
        Some(i + 1)
    }

    fn marker(&mut self, i: usize, finished: bool) -> Option<Option<usize>> {
        let c = self.raw[i];
        match c {
            '#' | '>' => return Some(Some(i + 1)),
            '-' | '*' | '+' => {
                let next = self.at(i + 1, finished)?;
                if next == Some(' ') {
                    self.line_start = false;
                    return Some(Some(i + 2));
                }
                if c == '-' && next == Some('-') {
                    return Some(Some(i + 1));
                }
            }
            '0'..='9' => {
                let mut end = i;
                while self.raw.get(end).is_some_and(char::is_ascii_digit) && end - i < 9 {
                    end += 1;
                }
                let after = [self.at(end, finished)?, self.at(end + 1, finished)?];
                if matches!(after[0], Some('.' | ')')) && after[1] == Some(' ') {
                    self.line_start = false;
                    return Some(Some(end + 2));
                }
            }
            _ => {}
        }
        self.line_start = false;
        Some(None)
    }

    fn url(&mut self, i: usize, finished: bool) -> Option<Option<usize>> {
        let mut matched = None;
        for prefix in ["https://", "http://", "www."] {
            let mut whole = true;
            for (offset, expected) in prefix.chars().enumerate() {
                match self.at(i + offset, finished)? {
                    Some(c) if c.to_ascii_lowercase() == expected => {}
                    _ => {
                        whole = false;
                        break;
                    }
                }
            }
            if whole {
                matched = Some(prefix);
                break;
            }
        }
        if matched.is_none() {
            return Some(None);
        }
        let mut end = i;
        loop {
            match self.at(end, finished)? {
                Some(c)
                    if end - i < MAX_URL_CHARS
                        && !c.is_whitespace()
                        && !matches!(c, ')' | ']' | '>' | '<' | '"' | OPEN | CLOSE) =>
                {
                    end += 1
                }
                _ => break,
            }
        }
        let mut url: String = self.raw[i..end].iter().collect();
        let mut trailing = String::new();
        while let Some(c) = url.chars().last().filter(|c| ".,;:!?".contains(*c)) {
            url.pop();
            trailing.insert(0, c);
        }
        let rest = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
        let host = rest.split(['/', '?', '#', ':']).next().unwrap_or_default();
        let host = host
            .get(..4)
            .filter(|head| head.eq_ignore_ascii_case("www."))
            .map_or(host, |_| &host[4..]);
        for c in host.chars().chain(trailing.chars()) {
            self.out(c);
        }
        Some(Some(end))
    }

    fn word_start(&self) -> bool {
        self.space
            || self
                .last
                .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '.' | '/' | '@' | '-')))
    }

    fn out(&mut self, c: char) {
        if c.is_whitespace() {
            self.space = self.last.is_some_and(|last| last != '\n');
            return;
        }
        if std::mem::take(&mut self.space) && !matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')')
        {
            self.plain.push(' ');
        }
        self.plain.push(c);
        self.last = Some(c);
    }

    fn out_break(&mut self) {
        self.space = false;
        if self.last.is_some_and(|last| last != '\n') {
            self.plain.push('\n');
            self.last = Some('\n');
        }
    }

    fn peek(&self, index: usize, finished: bool) -> Option<Option<char>> {
        match self.plain.get(index) {
            Some(c) => Some(Some(*c)),
            None if finished => Some(None),
            None => None,
        }
    }

    fn mark(&self, i: usize, finished: bool) -> Option<Mark> {
        let c = self.plain[i];
        Some(match c {
            '\n' => Mark::Hard,
            '。' | '！' | '？' => Mark::Sentence(self.closers(i + 1, finished)?),
            '.' | '!' | '?' => {
                if c == '.' && i > 0 && self.plain[i - 1] == '.' {
                    return Some(Mark::Plain);
                }
                let end = self.closers(i + 1, finished)?;
                match self.peek(end, finished)? {
                    None | Some('\n') => Mark::Sentence(end),
                    Some(' ') if c != '.' => Mark::Sentence(end),
                    Some(' ') => {
                        if self.abbreviated(i) {
                            return Some(Mark::Plain);
                        }
                        match self.peek(end + 1, finished)? {
                            Some(next) if next.is_lowercase() => Mark::Plain,
                            _ => Mark::Sentence(end),
                        }
                    }
                    Some(_) => Mark::Plain,
                }
            }
            ',' | ';' | ':' => match self.peek(i + 1, finished)? {
                Some(' ') => Mark::Clause(i + 1),
                _ => Mark::Plain,
            },
            '，' | '、' | '；' | '：' => Mark::Clause(i + 1),
            _ => Mark::Plain,
        })
    }

    fn closers(&self, mut index: usize, finished: bool) -> Option<usize> {
        while let Some(c) = self.peek(index, finished)? {
            if !matches!(c, '"' | '\'' | ')' | '”' | '’' | '」' | '』' | '）') {
                break;
            }
            index += 1;
        }
        Some(index)
    }

    fn abbreviated(&self, dot: usize) -> bool {
        let begin = self.plain[..dot]
            .iter()
            .rposition(|c| *c == ' ' || *c == '\n')
            .map_or(0, |space| space + 1);
        let word: String = self.plain[begin..dot]
            .iter()
            .skip_while(|c| matches!(c, '(' | '"' | '\''))
            .flat_map(|c| c.to_lowercase())
            .collect();
        let mut letters = word.chars();
        let single = letters.next().is_some_and(char::is_alphabetic) && letters.next().is_none();
        single || word.contains('.') || ABBREVIATIONS.contains(&word.as_str())
    }

    fn cut(&mut self, finished: bool) -> Vec<String> {
        let mut chunks = Vec::new();
        while self.scan < self.plain.len() {
            let i = self.scan;
            let Some(mark) = self.mark(i, finished) else {
                break;
            };
            match mark {
                Mark::Hard => {
                    self.emit(i, true, &mut chunks);
                    self.restart(i + 1);
                    continue;
                }
                Mark::Sentence(end) => {
                    self.emit(end, true, &mut chunks);
                    self.restart(end);
                    continue;
                }
                Mark::Clause(end) if !self.released && words(&self.plain[..end]) >= EARLY_WORDS => {
                    self.emit(end, false, &mut chunks);
                    self.restart(end);
                    continue;
                }
                Mark::Clause(end) => self.clause = Some(end),
                Mark::Plain => {}
            }
            if self.plain[i] == ' ' {
                self.gap = Some(i);
            }
            if i + 1 >= self.max {
                let end = self
                    .clause
                    .or(self.gap)
                    .filter(|end| *end > 0)
                    .unwrap_or(i + 1);
                self.emit(end, false, &mut chunks);
                self.restart(end);
                continue;
            }
            self.scan += 1;
        }
        chunks
    }

    fn emit(&mut self, end: usize, sentence: bool, chunks: &mut Vec<String>) {
        let text: String = self.plain[..end].iter().collect();
        let mut text = text.trim().to_owned();
        if !text.chars().any(char::is_alphanumeric) {
            return;
        }
        if sentence {
            match text.chars().last() {
                Some(c) if cjk(c) => text.push('。'),
                Some(c) if c.is_alphanumeric() => text.push('.'),
                _ => {}
            }
        }
        self.released = true;
        chunks.push(text);
    }

    fn restart(&mut self, end: usize) {
        self.plain.drain(..end.min(self.plain.len()));
        self.scan = 0;
        self.clause = None;
        self.gap = None;
    }
}

fn words(text: &[char]) -> usize {
    let tokens = text.split(|c| *c == ' ').filter(|t| !t.is_empty()).count();
    tokens + text.iter().filter(|c| cjk(**c)).count() / 2
}

pub(crate) fn cjk(c: char) -> bool {
    matches!(
        c,
        '\u{3040}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{AC00}'..='\u{D7AF}'
    )
}

fn emoji(c: char) -> bool {
    matches!(
        c,
        '\u{1F000}'..='\u{1FAFF}'
            | '\u{2600}'..='\u{27BF}'
            | '\u{2B00}'..='\u{2BFF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{200D}'
            | '\u{20E3}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn all(deltas: &[&str]) -> Vec<String> {
        let mut cutter = Cutter::default();
        let mut chunks: Vec<String> = deltas.iter().flat_map(|d| cutter.push(d)).collect();
        chunks.extend(cutter.finish());
        chunks
    }

    fn one(text: &str) -> Vec<String> {
        all(&[text])
    }

    #[test]
    fn markdown_reads_as_plain_sentences() {
        assert_eq!(
            one("**Shipping** starts in _May_ ⟦3⟧. See [the policy](https://example.com/a?b=1) for `returns`."),
            ["Shipping starts in May.", "See the policy for returns."]
        );
        assert_eq!(
            one("# Options\n- First plan\n- Second plan\n\n1. Third\n2) Fourth"),
            [
                "Options.",
                "First plan.",
                "Second plan.",
                "Third.",
                "Fourth."
            ]
        );
        assert_eq!(one("> Quoted line 🎉 here ✅"), ["Quoted line here."]);
        assert_eq!(
            one("Read https://www.example.com/x, then stop."),
            ["Read example.com, then stop."]
        );
        assert_eq!(
            one("Code:\n```rust\nfn main() {}\n```\nDone"),
            ["Code:", "Done."]
        );
        assert_eq!(one("snake_case and *a* | b"), ["snake case and a b."]);
        assert_eq!(
            one("-5 degrees, 1,200 people"),
            ["-5 degrees, 1,200 people."]
        );
        assert!(one("*** --- ⟦1⟧").is_empty());
    }

    #[test]
    fn never_cuts_inside_numbers_abbreviations_ellipses_urls_or_times() {
        for text in [
            "It costs 3.5 dollars and 1,200 cents now",
            "Ask Dr. Smith about it, e.g. tomorrow",
            "The U.S. economy grew",
            "Wait... then go",
            "Wait… then go",
            "Open example.com/docs.html today",
            "Meet at 10:30 tomorrow",
            "Call J. Smith now",
            "It is approx. five",
        ] {
            assert_eq!(one(text).len(), 1, "{text}");
        }
    }

    #[test]
    fn cuts_on_latin_and_cjk_sentence_ends() {
        assert_eq!(
            one("Yes! Really? \"Sure.\" Done"),
            ["Yes!", "Really?", "\"Sure.\"", "Done."]
        );
        assert_eq!(
            one("今日は晴れです。明日は雨！"),
            ["今日は晴れです。", "明日は雨！"]
        );
        assert_eq!(
            one("First paragraph\n\nSecond"),
            ["First paragraph.", "Second."]
        );
    }

    #[test]
    fn the_first_chunk_leaves_early_and_long_chunks_cut_at_clauses() {
        let chunks = one("This reply has quite a few words in its opening clause, and then it goes on. Later, short.");
        assert_eq!(
            chunks[0],
            "This reply has quite a few words in its opening clause,"
        );
        assert_eq!(chunks[2], "Later, short.");
        let long = format!(
            "W{}, {} end.",
            "word ".repeat(30).trim(),
            "more ".repeat(40).trim()
        );
        let mut cutter = Cutter::default();
        cutter.push("Short start. ");
        let chunks: Vec<String> = cutter
            .push(&long)
            .into_iter()
            .chain(cutter.finish())
            .collect();
        assert!(
            chunks.iter().all(|c| c.chars().count() <= MAX_CHUNK_CHARS),
            "{chunks:?}"
        );
        assert_eq!(chunks[0], "Short start.");
        assert!(
            chunks[1].starts_with("Wword") && chunks[1].ends_with("word,"),
            "{chunks:?}"
        );
        let unbroken = "x".repeat(500);
        assert!(one(&unbroken)
            .iter()
            .all(|c| c.chars().count() <= MAX_CHUNK_CHARS));
    }

    #[test]
    fn a_block_boundary_ends_a_sentence() {
        let mut cutter = Cutter::default();
        assert!(cutter.push("First block").is_empty());
        assert_eq!(cutter.end_block(), ["First block."]);
        assert_eq!(cutter.push("[open"), Vec::<String>::new());
        assert_eq!(cutter.finish(), ["open."]);
    }

    proptest! {
        #[test]
        fn any_split_cuts_the_same(
            text in "([a-zA-Z0-9]{1,6}|[ .,:;!?\\-*_`#>\\[\\]()\\n]|https://a\\.b/c|e\\.g\\.|Dr\\.|⟦2⟧|。|3\\.5|10:30|\\.\\.\\.){0,60}",
            cuts in proptest::collection::vec(0usize..400, 0..8),
        ) {
            let chars: Vec<char> = text.chars().collect();
            let mut points: Vec<usize> = cuts.into_iter().map(|c| c % (chars.len() + 1)).collect();
            points.sort_unstable();
            let mut deltas = Vec::new();
            let mut from = 0;
            for point in points {
                deltas.push(chars[from..point].iter().collect::<String>());
                from = point;
            }
            deltas.push(chars[from..].iter().collect::<String>());
            let borrowed: Vec<&str> = deltas.iter().map(String::as_str).collect();
            let split = all(&borrowed);
            prop_assert_eq!(&split, &one(&text));
            for chunk in &split {
                prop_assert!(chunk.chars().count() <= MAX_CHUNK_CHARS + 1);
                prop_assert!(!chunk.contains(['⟦', '⟧', '*', '`', '\n']));
            }
        }
    }
}
