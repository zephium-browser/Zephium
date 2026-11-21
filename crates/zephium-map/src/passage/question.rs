use super::*;

pub(super) fn vocabulary(terms: &BTreeMap<String, usize>) -> BTreeMap<String, usize> {
    let mut words = terms.clone();
    for (term, &i) in terms {
        if term.len() < 3 || !term.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        let root = if let Some(prefix) = term.strip_suffix("ies") {
            Cow::Owned(format!("{prefix}y"))
        } else if ["ches", "shes", "xes", "zes", "sses"]
            .iter()
            .any(|suffix| term.ends_with(suffix))
        {
            Cow::Borrowed(&term[..term.len() - 2])
        } else {
            Cow::Borrowed(
                term.strip_suffix('s')
                    .filter(|_| !term.ends_with("ss"))
                    .unwrap_or(term),
            )
        };
        let verb = root.strip_suffix('e').unwrap_or(&root);
        let plural = if ["ch", "sh", "x", "z", "ss"]
            .iter()
            .any(|suffix| root.ends_with(suffix))
        {
            format!("{root}es")
        } else {
            format!("{root}s")
        };
        for form in [
            root.to_string(),
            plural,
            format!("{verb}ing"),
            format!("{verb}ed"),
        ] {
            words.entry(form).or_insert(i);
        }
    }
    words
}

pub(super) fn comparison(q: &QueryRead) -> Option<(&[String], &[String])> {
    let (words, separators): (_, &[_]) = match q.terms() {
        [first, second, rest @ ..] if first == "difference" && second == "between" => {
            (rest, &["and"])
        }
        terms => (terms, &["vs", "vs.", "versus"]),
    };
    let at = words
        .iter()
        .position(|t| separators.contains(&t.as_str()))?;
    let (left, right) = (&words[..at], &words[at + 1..]);
    (!left.is_empty() && !right.is_empty()).then_some((left, right))
}

pub(super) fn contrast(
    sides: (&[String], &[String]),
    docs: &[&Doc],
    blocks: &[Block<'_>],
) -> Vec<Passage> {
    let subject = |side: &[String]| {
        side.iter()
            .flat_map(|t| tokens(t))
            .filter(|t| !STOP.contains(&t.as_str()))
            .collect::<BTreeSet<_>>()
    };
    let sides = [subject(sides.0), subject(sides.1)];
    if sides.iter().any(BTreeSet::is_empty) || sides[0] == sides[1] {
        return Vec::new();
    }
    let mut leads = [Vec::new(), Vec::new()];
    for (page, doc) in docs.iter().take(MAX_RESULT_PAGES).enumerate() {
        let title: BTreeSet<_> = tokens(&doc.head().title).collect();
        let mut first = true;
        for b in blocks.iter().filter(|b| {
            b.doc == page && (doc.nodes()[b.node].marks.has(Marks::PROSE) || b.structured)
        }) {
            let present: BTreeSet<_> = tokens(cut(&b.text, false).unwrap_or_default()).collect();
            if b.text.trim_end().ends_with('?') {
                continue;
            }
            if sides.iter().all(|s| s.is_subset(&present)) {
                if let Some(answer) = make(b, doc) {
                    return vec![answer];
                }
            }
            if first && doc.nodes()[b.node].marks.has(Marks::PROSE) {
                let heading: BTreeSet<_> = b
                    .heading
                    .into_iter()
                    .filter(|&h| matches!(doc.nodes()[h].role, Role::Heading { level: Some(1) }))
                    .flat_map(|h| tokens(&doc.nodes()[h].text))
                    .collect();
                for (i, side) in sides.iter().enumerate() {
                    if [&heading, &title]
                        .iter()
                        .any(|label| side.is_subset(label) && !sides[1 - i].is_subset(label))
                        && side.is_subset(&present)
                    {
                        if let Some(passage) = make(b, doc) {
                            leads[i].push(passage);
                        }
                    }
                }
                first = false;
            }
        }
    }
    let [left, right] = leads;
    for a in left {
        if let Some(b) = right
            .iter()
            .find(|b| a.source.page_match() != b.source.page_match())
        {
            return vec![a, b.clone()];
        }
    }
    Vec::new()
}

pub(super) struct Question {
    context: u64,
    all: u64,
    task: bool,
    qualifiers: TaskQualifiers,
}

pub(super) struct TaskQualifiers {
    quantity: Option<String>,
    last: Option<String>,
}

impl Question {
    pub(super) fn new(
        q: &QueryRead,
        words: &BTreeMap<String, usize>,
        count: usize,
        doc: &Doc,
    ) -> Self {
        let terms = q.terms();
        let title: BTreeSet<_> = tokens(&doc.head().title)
            .chain(tokens(doc.page().host_str().unwrap_or_default()))
            .collect();
        let action = |word: &str| crate::query::ACTIONS.contains(&word);
        let context = terms
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                (*i == 0 && !action(t)
                    || i.checked_sub(1)
                        .is_some_and(|p| matches!(terms[p].as_str(), "on" | "in" | "using")))
                    && title.contains(*t)
                    && !t.bytes().all(|b| b.is_ascii_digit())
            })
            .filter_map(|(_, t)| words.get(t))
            .fold(0, |a, i| a | 1 << i);
        Self {
            context,
            all: u64::MAX >> (MAX_CONTENT_TERMS - count),
            task: terms.first().is_some_and(|t| t == "how") || terms.iter().any(|t| action(t)),
            qualifiers: TaskQualifiers::new(terms, doc.page()),
        }
    }

    pub(super) fn permits(&self, text: &str) -> bool {
        self.qualifiers.permits(text)
    }

    pub(super) fn best(
        &self,
        local: &[&Block<'_>],
        doc: &Doc,
        vocabulary: &BTreeMap<String, usize>,
    ) -> Option<Passage> {
        let mut best = None;
        for (i, &b) in local.iter().enumerate() {
            let role = doc.nodes()[b.node].role;
            let code = role == Role::CodeBlock && self.task;
            let intro = code
                .then(|| {
                    local[..i]
                        .iter()
                        .rev()
                        .copied()
                        .take_while(|p| p.scope == b.scope && p.heading == b.heading)
                        .find(|p| doc.nodes()[p.node].role != Role::Text)
                        .filter(|p| {
                            matches!(doc.nodes()[p.node].role, Role::Paragraph | Role::ListItem)
                        })
                })
                .flatten();
            if !matches!(
                role,
                Role::CodeBlock | Role::Paragraph | Role::ListItem | Role::Row | Role::Definition
            ) || b.text.trim_end().ends_with('?')
            {
                continue;
            }
            let option_heading = b
                .heading
                .filter(|&h| doc.nodes()[h].text.trim_start().starts_with(['-', '\\']));
            let near = if code {
                b.terms | intro.map_or(0, |p| p.opening)
            } else {
                b.opening
            };
            let section = b.heading_terms | b.path_terms;
            if (near | section | self.context) != self.all
                || near & !self.context == 0 && !(code && section & !self.context != 0)
            {
                continue;
            }
            if !self.permits(&b.text) && !intro.is_some_and(|p| self.permits(&p.text)) {
                continue;
            }
            if !code && !b.structured && !b.text.contains(['.', '!', ':']) {
                continue;
            }
            if role == Role::ListItem
                && !b.text.contains(['.', '!', ':'])
                && (self.task
                    || !tokens(&b.text)
                        .any(|t| !STOP.contains(&t.as_str()) && !vocabulary.contains_key(&t)))
            {
                continue;
            }
            let rank = (
                code,
                b.accepted,
                matches!(role, Role::Definition | Role::Row) || option_heading.is_some(),
                (b.heading_terms & !self.context).count_ones(),
                b.path_terms.count_ones(),
                (near & !self.context & !section).count_ones(),
                b.structured,
            );
            if best.as_ref().is_none_or(|(old, _, _)| rank > *old) {
                best = Some((rank, b, intro));
            }
        }
        let (_, b, intro) = best?;
        if let Some(intro) = intro {
            paired(intro, b, doc)
        } else {
            let mut answer = make(b, doc)?;
            let heading = b
                .heading
                .filter(|&h| doc.nodes()[h].text.trim_start().starts_with(['-', '\\']))
                .or_else(|| {
                    (doc.nodes()[b.node].role == Role::ListItem && !self.task)
                        .then_some(b.subject_heading)
                        .flatten()
                });
            if let Some(h) = heading {
                let title = &doc.nodes()[h].text;
                if title.chars().count() + 1 + answer.text.chars().count() <= MAX_PROSE_CHARS {
                    answer.text = format!("{title}\n{}", answer.text);
                    answer.heading = Some(clip(title, TITLE_BYTES));
                }
            }
            Some(answer)
        }
    }
}

impl TaskQualifiers {
    pub(super) fn new(terms: &[String], source: &url::Url) -> Self {
        let site = crate::SiteKey::from_url(source);
        let entity = site
            .as_ref()
            .and_then(|site| site.as_str().split_once('.'))
            .and_then(|(label, _)| tokens(label).next());
        Self {
            last: terms.iter().position(|t| t == "last").and_then(|at| {
                let mut tail = terms[at + 1..].iter();
                let first = tokens(tail.next()?).next()?;
                let qualifier = entity.as_ref() == Some(&first);
                Some(
                    qualifier
                        .then(|| {
                            tail.next()
                                .and_then(|term| tokens(term).next())
                                .filter(|noun| !STOP.contains(&noun.as_str()))
                        })
                        .flatten()
                        .unwrap_or(first),
                )
            }),
            quantity: terms.windows(2).find(|pair| pair[0] == "all").map(|pair| {
                let noun = pair[1].trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                let noun = if noun.is_empty() { &pair[1] } else { noun };
                format!("all {noun}")
            }),
        }
    }

    pub(super) fn permits(&self, text: &str) -> bool {
        if self.quantity.is_none() && self.last.is_none() {
            return true;
        }
        let text = text.to_lowercase();
        let quantified = self.quantity.as_ref().is_none_or(|phrase| {
            text.match_indices(phrase).any(|(i, _)| {
                let rest = &text[i + phrase.len()..];
                !text[..i].ends_with(char::is_alphanumeric)
                    && (rest.is_empty()
                        || rest.starts_with(['.', ',', ':', '!', '?', '\n'])
                        || rest.starts_with(char::is_whitespace)
                            && matches!(
                                rest.split_whitespace().next(),
                                Some("before" | "after" | "when")
                            ))
            })
        });
        quantified
            && self.last.as_ref().is_none_or(|noun| {
                let mut words = tokens(&text);
                let mut previous = words.next();
                words.any(|word| {
                    let found = word == *noun && previous.as_deref() == Some("last");
                    previous = Some(word);
                    found
                })
            })
    }
}
