use thiserror::Error;

pub const MAX_QUERY_BYTES: usize = 2048;
pub const MAX_QUERY_TERMS: usize = 64;
const ENTITY_WORDS: usize = 4;
const PUNCTUATED_BYTES: usize = 40;
const PUNCTUATION_MARKS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    Low,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionRole {
    Pricing,
    Documentation,
    Api,
    Reference,
    Changelog,
    Releases,
    Status,
    Download,
    Install,
    Login,
    Blog,
    About,
    Careers,
    Support,
}

pub const SECTION_WORDS: &[(&str, SectionRole)] = &[
    ("release notes", SectionRole::Releases),
    ("sign in", SectionRole::Login),
    ("pricing", SectionRole::Pricing),
    ("docs", SectionRole::Documentation),
    ("documentation", SectionRole::Documentation),
    ("api", SectionRole::Api),
    ("reference", SectionRole::Reference),
    ("changelog", SectionRole::Changelog),
    ("releases", SectionRole::Releases),
    ("status", SectionRole::Status),
    ("downloads", SectionRole::Download),
    ("download", SectionRole::Download),
    ("install", SectionRole::Install),
    ("login", SectionRole::Login),
    ("blog", SectionRole::Blog),
    ("about", SectionRole::About),
    ("careers", SectionRole::Careers),
    ("support", SectionRole::Support),
    ("help", SectionRole::Support),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub word: &'static str,
    pub role: SectionRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassageKind {
    Question,
    Pasted,
}

#[derive(Clone, PartialEq, Eq)]
pub enum QueryKind {
    Place {
        entity: Vec<String>,
        section: Option<Section>,
    },
    Passage(PassageKind),
    Compare {
        left: Vec<String>,
        right: Vec<String>,
        shared: Option<Section>,
    },
}

pub struct QueryRead {
    kind: QueryKind,
    terms: Vec<String>,
    confidence: Confidence,
}
impl QueryRead {
    pub fn kind(&self) -> &QueryKind {
        &self.kind
    }
    pub fn terms(&self) -> &[String] {
        &self.terms
    }
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum QueryError {
    #[error("empty query")]
    Empty,
    #[error("query exceeds byte limit")]
    Bytes,
    #[error("query exceeds term limit")]
    Terms,
}

fn split_section(terms: &[String]) -> (&[String], Option<Section>) {
    for &(word, role) in SECTION_WORDS {
        let count = word.split_whitespace().count();
        if terms.len() > count {
            let split = terms.len() - count;
            if terms[split..]
                .iter()
                .map(String::as_str)
                .eq(word.split_whitespace())
            {
                return (&terms[..split], Some(Section { word, role }));
            }
        }
    }
    (terms, None)
}

fn entity_like(terms: &[String]) -> bool {
    const AMBIGUOUS: &[&str] = &[
        "vs", "vs.", "or", "versus", "and", "the", "a", "to", "for", "i", "my", "is", "it", "best",
        "in", "of", "should", "either", "with", "your", "how", "why", "what", "better", "than",
        "near", "me",
    ];
    !terms.is_empty()
        && terms.len() <= ENTITY_WORDS
        && terms.iter().all(|word| {
            word.is_ascii()
                && word.bytes().any(|b| b.is_ascii_alphanumeric())
                && word
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".+#-_".contains(&b))
                && !AMBIGUOUS.contains(&word.as_str())
        })
}

fn pasted(text: &str, terms: &[String]) -> bool {
    let punct = text.bytes().filter(u8::is_ascii_punctuation).count();
    terms.iter().any(|term| {
        term.split(|c: char| !c.is_alphanumeric()).any(|token| {
            token.len() == 5
                && token.starts_with('e')
                && token.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        }) || term
            .split_once(':')
            .is_some_and(|(kind, _)| kind.ends_with("error") || kind.ends_with("exception"))
            || term.starts_with('/') && term.len() > 1
            || term.starts_with("./")
            || term.starts_with("../")
            || term.contains(":\\")
            || term.contains("/src/")
            || term.starts_with("src/")
    }) || text.contains("thread '") && text.contains("panicked")
        || text.contains("Traceback (")
        || text.contains("\n    at ")
        || [('"', '"'), ('\'', '\''), ('`', '`'), ('“', '”')]
            .iter()
            .any(|&(start, end)| {
                text.match_indices(start).any(|(at, _)| {
                    let before = &text[..at];
                    (start != '\''
                        || before
                            .chars()
                            .next_back()
                            .is_none_or(|c| !c.is_alphanumeric()))
                        && text[at + start.len_utf8()..]
                            .split_once(end)
                            .is_some_and(|(quote, _)| !quote.trim().is_empty())
                })
            })
        || (text.len() >= PUNCTUATED_BYTES && punct >= PUNCTUATION_MARKS && punct >= text.len() / 8)
}

pub fn read_query(text: &str) -> Result<QueryRead, QueryError> {
    if text.len() > MAX_QUERY_BYTES {
        return Err(QueryError::Bytes);
    }
    let terms: Vec<String> = text
        .split_whitespace()
        .take(MAX_QUERY_TERMS + 1)
        .map(str::to_lowercase)
        .collect();
    if terms.is_empty() {
        return Err(QueryError::Empty);
    }
    if terms.len() > MAX_QUERY_TERMS {
        return Err(QueryError::Terms);
    }
    let phrase = terms.join(" ");
    let question = phrase.ends_with('?')
        || [
            "how to ",
            "how do ",
            "how does ",
            "how can ",
            "what is ",
            "what are ",
            "what does ",
            "why is ",
            "why does ",
            "where is ",
            "when is ",
            "who is ",
            "can i ",
        ]
        .iter()
        .any(|prefix| phrase.starts_with(prefix));
    let (entity, section) = split_section(&terms);
    let mut confidence = Confidence::High;
    let kind = if pasted(text, &terms) {
        QueryKind::Passage(PassageKind::Pasted)
    } else if question {
        QueryKind::Passage(PassageKind::Question)
    } else if let Some(index) = entity
        .iter()
        .position(|word| matches!(word.as_str(), "vs" | "vs." | "versus" | "or"))
        .filter(|&i| entity_like(&entity[..i]) && entity_like(&entity[i + 1..]))
    {
        QueryKind::Compare {
            left: entity[..index].to_vec(),
            right: entity[index + 1..].to_vec(),
            shared: section,
        }
    } else {
        if !entity_like(entity) || (section.is_none() && entity.len() > 1) {
            confidence = Confidence::Low;
        }
        QueryKind::Place {
            entity: entity.to_vec(),
            section,
        }
    };
    Ok(QueryRead {
        kind,
        confidence,
        terms,
    })
}
