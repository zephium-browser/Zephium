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

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, PartialOrd, Ord,
)]
pub enum SectionRole {
    News,
    Product,
    Features,
    Solutions,
    Enterprise,
    Customers,
    Community,
    Security,
    Legal,
    Contact,
    SignUp,
    Learn,
    Examples,
    Playground,
    Integrations,
    Home,
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
    ("whats new", SectionRole::News),
    ("what’s new", SectionRole::News),
    ("what's new", SectionRole::News),
    ("news", SectionRole::News),
    ("announcement", SectionRole::News),
    ("announcements", SectionRole::News),
    ("latest", SectionRole::News),
    ("new", SectionRole::News),
    ("product", SectionRole::Product),
    ("products", SectionRole::Product),
    ("features", SectionRole::Features),
    ("solutions", SectionRole::Solutions),
    ("enterprise", SectionRole::Enterprise),
    ("customers", SectionRole::Customers),
    ("community", SectionRole::Community),
    ("forum", SectionRole::Community),
    ("security", SectionRole::Security),
    ("legal", SectionRole::Legal),
    ("privacy", SectionRole::Legal),
    ("terms", SectionRole::Legal),
    ("cookies", SectionRole::Legal),
    ("contact", SectionRole::Contact),
    ("sign up", SectionRole::SignUp),
    ("signup", SectionRole::SignUp),
    ("register", SectionRole::SignUp),
    ("learn", SectionRole::Learn),
    ("tutorial", SectionRole::Learn),
    ("tutorials", SectionRole::Learn),
    ("examples", SectionRole::Examples),
    ("playground", SectionRole::Playground),
    ("integrations", SectionRole::Integrations),
    ("home", SectionRole::Home),
    ("pricing", SectionRole::Pricing),
    ("book", SectionRole::Documentation),
    ("hub", SectionRole::Product),
    ("docs", SectionRole::Documentation),
    ("developers", SectionRole::Documentation),
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
    pub role: SectionRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassageKind {
    Question,
    Pasted,
}

#[derive(Clone, PartialEq, Eq)]
pub enum QueryKind {
    Instant,
    List,
    Place {
        fresh: bool,
        entity: Vec<String>,
        section: Option<Section>,
    },
    Passage {
        kind: PassageKind,
    },
    Compare {
        left: Vec<String>,
        right: Vec<String>,
        shared: Option<Section>,
    },
}

#[derive(Clone)]
pub struct QueryRead {
    kind: QueryKind,
    terms: Vec<String>,
    confidence: Confidence,
    pub(crate) diagnostic: Option<String>,
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
    #[error("query refused by search configuration")]
    Search,
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
                return (&terms[..split], Some(Section { role }));
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
    }) || terms.windows(2).any(|w| w[0] == "err!" && w[1] == "code")
        || terms.iter().any(|t| t == "certificate_verify_failed")
        || text.contains("thread '") && text.contains("panicked")
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

pub(crate) const ACTIONS: &[&str] = &[
    "create", "rename", "remove", "delete", "convert", "generate", "list", "clone", "undo",
    "reset", "enable", "disable", "change", "set", "get", "find", "fix", "update", "merge",
    "revert", "install", "download", "login",
];
const FACTS: &[&str] = &[
    "port",
    "command",
    "shortcut",
    "syntax",
    "example",
    "flag",
    "default",
    "limit",
    "version number",
    "status code",
    "error code",
    "meaning",
];

fn task(terms: &[String]) -> bool {
    if terms.len() < 2 {
        return false;
    }
    terms[..terms.len() - 1].iter().enumerate().any(|(i, word)| {
        ACTIONS.contains(&word.as_str())
            && !matches!(terms.get(i+1..i+4), Some([article, generic, to]) if article == "a" && matches!(generic.as_str(),"way" | "place") && to == "to")
    }) || FACTS.iter().any(|fact| {
        terms[1..]
            .windows(fact.split_whitespace().count())
            .any(|w| w.iter().map(String::as_str).eq(fact.split_whitespace()))
    })
}

fn concept_comparison(terms: &[String]) -> bool {
    const CONCEPTS: &[&str] = &[
        "authentication",
        "authorization",
        "concurrency",
        "parallelism",
        "latency",
        "throughput",
        "encoding",
        "encryption",
        "hashing",
        "stack",
        "heap",
        "tcp",
        "udp",
        "http",
        "https",
        "iteration",
        "recursion",
        "thread",
        "process",
        "threads",
        "processes",
    ];
    let concept = |side: &[String]| {
        !side.is_empty()
            && side
                .iter()
                .all(|w| w.bytes().all(|b| b.is_ascii_digit()) || CONCEPTS.contains(&w.as_str()))
    };
    terms
        .iter()
        .position(|w| matches!(w.as_str(), "vs" | "vs." | "versus"))
        .is_some_and(|i| concept(&terms[..i]) && concept(&terms[i + 1..]))
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
            "difference between ",
            "how to ",
            "how do ",
            "how does ",
            "how long ",
            "how much ",
            "how many ",
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
        .any(|prefix| phrase.starts_with(prefix))
        || [
            " how to ",
            " how do i ",
            " what is ",
            " difference between ",
        ]
        .iter()
        .any(|frame| phrase.contains(frame));
    let (entity, section) = split_section(&terms);
    let mut confidence = Confidence::High;
    let kind = if arithmetic(text) {
        QueryKind::Instant
    } else if pasted(text, &terms) {
        QueryKind::Passage {
            kind: PassageKind::Pasted,
        }
    } else if instant(&terms) {
        QueryKind::Instant
    } else if question || concept_comparison(&terms) {
        QueryKind::Passage {
            kind: PassageKind::Question,
        }
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
    } else if listing(&terms) {
        QueryKind::List
    } else if task(&terms) {
        QueryKind::Passage {
            kind: PassageKind::Question,
        }
    } else {
        let fresh_word = SECTION_WORDS.iter().find(|(word, role)| {
            if !matches!(role, SectionRole::News | SectionRole::Releases) {
                return false;
            }
            if *word == "new"
                && !(terms.first().is_some_and(|s| s == "new")
                    && terms.last().is_some_and(|s| {
                        matches!(s.as_str(), "model" | "version" | "release" | "features")
                    }))
            {
                return false;
            }
            terms
                .windows(word.split_whitespace().count())
                .any(|window| {
                    window
                        .iter()
                        .map(String::as_str)
                        .eq(word.split_whitespace())
                })
        });
        let fresh = fresh_word.is_some()
            || section.is_some_and(|s| matches!(s.role, SectionRole::News | SectionRole::Releases));
        let section = section.or_else(|| fresh_word.map(|&(_, role)| Section { role }));
        let entity = if fresh {
            entity
                .iter()
                .enumerate()
                .filter(|(index, w)| {
                    !(matches!(
                        w.as_str(),
                        "latest" | "news" | "announcement" | "announcements"
                    ) || *index == 0
                        && w.as_str() == "new"
                        && fresh_word.is_some_and(|(word, _)| *word == "new"))
                })
                .map(|(_, word)| word.clone())
                .collect()
        } else {
            entity.to_vec()
        };
        if !entity_like(&entity) || (section.is_none() && entity.len() > 1) {
            confidence = Confidence::Low;
        }
        QueryKind::Place {
            entity,
            fresh,
            section,
        }
    };
    let diagnostic = matches!(
        kind,
        QueryKind::Passage {
            kind: PassageKind::Pasted
        }
    )
    .then(|| {
        let line = text
            .lines()
            .rev()
            .find(|line| {
                line.split_whitespace().any(|w| {
                    w.to_ascii_lowercase()
                        .split_once(':')
                        .is_some_and(|(t, _)| t.ends_with("error") || t.ends_with("exception"))
                })
            })
            .or_else(|| text.lines().find(|l| !l.trim().is_empty()))
            .unwrap_or_default();
        let quoted = [('"', '"'), ('“', '”'), ('`', '`'), ('\'', '\'')]
            .into_iter()
            .find_map(|(open, close)| line.split_once(open)?.1.split_once(close).map(|(s, _)| s));
        if line.starts_with(['"', '“', '\'', '`']) || line.starts_with("quoted ") {
            quoted.unwrap_or(line).to_lowercase()
        } else {
            line.to_lowercase()
        }
    });
    Ok(QueryRead {
        kind,
        diagnostic,
        confidence,
        terms,
    })
}

fn instant(terms: &[String]) -> bool {
    let words: Vec<_> = terms.iter().map(String::as_str).collect();
    let number = |s: &str| s.parse::<f64>().is_ok_and(f64::is_finite);
    let unit = |s: &str| {
        matches!(
            s,
            "usd"
                | "eur"
                | "gbp"
                | "jpy"
                | "pln"
                | "cad"
                | "aud"
                | "chf"
                | "inr"
                | "cny"
                | "miles"
                | "mile"
                | "km"
                | "kilometers"
                | "m"
                | "cm"
                | "mm"
                | "inches"
                | "in"
                | "ft"
                | "feet"
                | "kg"
                | "lb"
                | "lbs"
                | "oz"
                | "g"
                | "c"
                | "f"
                | "liters"
                | "gallons"
        )
    };
    match words.as_slice() {
        [n, from, "to" | "in", to] if number(n) && unit(from) && unit(to) => true,
        [n, "of", m] if n.strip_suffix('%').is_some_and(number) && number(m) => true,
        ["define", _, ..] | ["time", "in", _, ..] | ["weather", "in", _, ..] => true,
        ["translate", rest @ ..] => rest.len() >= 3 && rest.contains(&"to"),
        ["timer"] | ["calculator"] | ["weather"] => true,
        ["timer", n, unit] | [n, unit, "timer"] => {
            number(n)
                && matches!(
                    *unit,
                    "second" | "seconds" | "minute" | "minutes" | "hour" | "hours" | "sec" | "min"
                )
        }
        ["weather", place @ ..] | [place @ .., "weather" | "time"] => {
            const CITIES: &[&str] = &[
                "san francisco",
                "london",
                "tokyo",
                "paris",
                "new york",
                "berlin",
                "warsaw",
                "seattle",
                "sydney",
                "boston",
                "chicago",
                "singapore",
                "los angeles",
                "toronto",
                "delhi",
                "utc",
            ];
            CITIES.contains(&place.join(" ").as_str())
                || words.first() == Some(&"weather") && place.is_empty()
        }
        _ => false,
    }
}

fn arithmetic(text: &str) -> bool {
    text.bytes().any(|b| b.is_ascii_digit())
        && text.bytes().any(|b| b"+-*/".contains(&b))
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_whitespace() || b"+-*/().".contains(&b))
        && !text.trim_start().starts_with('/')
}

fn listing(terms: &[String]) -> bool {
    let words: Vec<_> = terms.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["best", "buy" | "western", ..] => false,
        ["best", _, ..]
        | ["flights", "to", _, ..]
        | ["hotels", "in", _, ..]
        | ["cheap", "flights", _, ..]
        | ["recipes", "for", _, ..]
        | ["jobs", "in", _, ..]
        | ["buy", _, ..]
        | [_, .., "near", "me"] => true,
        ["tech" | "world" | "business" | "local" | "sports" | "breaking" | "science", "news", ..] => {
            true
        }
        ["remote" | "freelance", .., "jobs"]
        | ["entry", "level", .., "jobs"]
        | ["part", "time", .., "jobs"] => true,
        [_, .., "recipe" | "recipes"] => words.iter().any(|w| {
            matches!(
                *w,
                "easy"
                    | "dinner"
                    | "soup"
                    | "cookie"
                    | "chicken"
                    | "pasta"
                    | "bread"
                    | "vegan"
                    | "vegetarian"
                    | "salad"
                    | "cake"
            )
        }),
        [_, .., "icon" | "icons" | "images" | "photos"] => words.iter().any(|w| {
            matches!(
                *w,
                "analytics"
                    | "calendar"
                    | "search"
                    | "stock"
                    | "free"
                    | "arrow"
                    | "settings"
                    | "dashboard"
                    | "outline"
                    | "vector"
                    | "background"
            )
        }),
        [_, .., "headphones" | "shoes" | "laptops" | "monitors" | "chairs" | "keyboards"] => {
            words.iter().all(|w| {
                matches!(
                    *w,
                    "wireless"
                        | "noise"
                        | "cancelling"
                        | "running"
                        | "gaming"
                        | "budget"
                        | "ergonomic"
                        | "mechanical"
                        | "office"
                        | "headphones"
                        | "shoes"
                        | "laptops"
                        | "monitors"
                        | "chairs"
                        | "keyboards"
                )
            })
        }
        _ => false,
    }
}
