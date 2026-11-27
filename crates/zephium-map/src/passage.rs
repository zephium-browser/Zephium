mod question;
use crate::{
    ancestors,
    doc::{Landmark, Marks, Role},
    query::{Confidence, PassageKind, QueryKind, QueryRead},
    results::{clip, Featured, Organic, TITLE_BYTES},
    site::{challenge, quote},
    visible, Doc, Locator, PageKey,
};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PASSAGES: usize = 3;
pub const MAX_RESULT_PAGES: usize = 5;
pub const MAX_PROSE_CHARS: usize = 1200;
pub const MAX_CODE_BYTES: usize = 4096;
pub const MAX_CONTENT_TERMS: usize = u64::BITS as usize;
pub const SUBJECT_START_WORDS: usize = 16;
const ERROR_FOLLOWING_BLOCKS: usize = 3;
const STOP: &[&str] = &[
    "a", "an", "the", "how", "do", "does", "i", "to", "is", "are", "was", "be", "can", "in", "of",
    "on", "and", "or", "my", "what", "where", "when", "why", "who", "for", "with", "it", "at",
    "from", "error", "please", "that", "this", "by", "work",
];

#[derive(Clone, PartialEq, Eq)]
pub struct Passage {
    pub text: String,
    pub code: bool,
    pub language: Option<String>,
    pub source: PageKey,
    pub source_title: String,
    pub heading: Option<String>,
    pub locator: Locator,
    pub confidence: Confidence,
}

fn tokens(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
}
fn content_words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split_whitespace()
        .filter(|t| !t.contains(['/', '\\']) && !t.trim_start_matches(['(', '[']).starts_with("0x"))
        .flat_map(tokens)
        .filter(|s| !STOP.contains(&s.as_str()) && !s.bytes().all(|b| b.is_ascii_digit()))
}
fn query_terms(q: &QueryRead) -> BTreeSet<String> {
    if let Some(message) = &q.diagnostic {
        content_words(message).collect()
    } else {
        q.terms()
            .iter()
            .flat_map(|t| tokens(t))
            .filter(|s| !STOP.contains(&s.as_str()) && s != "meaning")
            .collect()
    }
}
struct Block<'a> {
    doc: usize,
    text: Cow<'a, str>,
    node: usize,
    heading: Option<usize>,
    heading_end: usize,
    scope: Option<usize>,
    accepted: bool,
    suggested: bool,
    terms: u64,
    subject_heading: Option<usize>,
    quote_offset: usize,
    path_terms: u64,
    structured: bool,
    opening: u64,
    heading_terms: u64,
}
fn blocks<'a>(page: usize, doc: &'a Doc, terms: &BTreeMap<String, usize>) -> Vec<Block<'a>> {
    let nodes = doc.nodes();
    let selected = |role| {
        matches!(
            role,
            Role::Paragraph
                | Role::CodeBlock
                | Role::ListItem
                | Role::Definition
                | Role::Row
                | Role::Text
                | Role::Document
        )
    };
    let forbidden = |role| {
        matches!(
            role,
            Role::Landmark {
                landmark: Some(
                    Landmark::Navigation
                        | Landmark::Banner
                        | Landmark::Contentinfo
                        | Landmark::Complementary
                )
            } | Role::Link
        )
    };
    let mut mass = vec![0usize; nodes.len()];
    let mut link_text = vec![0usize; nodes.len()];
    let mut elements = vec![1usize; nodes.len()];
    let mut excluded = Vec::with_capacity(nodes.len());
    for n in nodes {
        excluded.push(
            n.hidden
                || n.marks.has(Marks::SPONSORED)
                || n.marks.has(Marks::GENERATED)
                || n.marks.has(Marks::ANCILLARY)
                || forbidden(n.role)
                || n.parent_index().is_some_and(|p| excluded[p]),
        );
    }
    let mut has_main = false;
    for (i, n) in nodes.iter().enumerate().rev() {
        if n.role == Role::Link && visible(nodes, i) {
            link_text[i] = n.text.chars().filter(|c| c.is_alphanumeric()).count();
        }
        if let Some(p) = n.parent_index() {
            link_text[p] += link_text[i];
        }
        if excluded[i] {
            mass[i] = 0;
            continue;
        }
        has_main |= matches!(
            n.role,
            Role::Document
                | Role::Landmark {
                    landmark: Some(Landmark::Main)
                }
        );
        if selected(n.role) {
            mass[i] += n.text.len();
        }
        if let Some(p) = n.parent_index() {
            mass[p] += mass[i];
            elements[p] += elements[i];
        }
    }
    let dense = nodes
        .iter()
        .enumerate()
        .filter(|(i, n)| {
            n.role == Role::Group && n.parent_index().is_some() && mass[*i] > 0 && !excluded[*i]
        })
        .max_by_key(|(i, _)| (mass[*i] * 64 / (elements[*i] + 1), mass[*i], *i))
        .map(|(i, _)| i);
    let relevant = |s: &str| {
        tokens(s)
            .filter_map(|t| terms.get(&t).copied())
            .fold(0u64, |mask, i| mask | (1 << i))
    };
    let mut headings = BTreeMap::<_, Vec<(u8, usize, u64)>>::new();
    let mut heading_ends = vec![nodes.len(); nodes.len()];
    let mut definitions = BTreeMap::<_, (String, bool)>::new();
    let mut headers = BTreeMap::<_, (String, usize)>::new();
    let mut out = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        if excluded[i] {
            continue;
        }
        let scope = ancestors(nodes, i).find(|&a| {
            matches!(
                nodes[a].role,
                Role::Document
                    | Role::Landmark {
                        landmark: Some(Landmark::Main)
                    }
            )
        });
        if has_main && scope.is_none()
            || !has_main
                && !matches!(n.role, Role::Term | Role::Definition)
                && dense.is_some_and(|d| !ancestors(nodes, i).any(|a| a == d))
        {
            continue;
        }
        let path = headings.entry(scope).or_default();
        if let Role::Heading { level } = n.role {
            let level = level.unwrap_or(1);
            while path.last().is_some_and(|(prior, _, _)| *prior >= level) {
                if let Some((_, closed, _)) = path.pop() {
                    heading_ends[closed] = i;
                }
            }
            path.push((level, i, relevant(&n.text)));
            continue;
        }
        let heading = path.last().map(|(_, h, terms)| (*h, *terms));
        let parent = n.parent_index();
        if n.role == Role::Term {
            let (label, consumed) = definitions.entry(parent).or_default();
            if *consumed {
                label.clear();
                *consumed = false;
            }
            if !label.is_empty() {
                label.push(' ');
            }
            label.push_str(n.text.trim());
            continue;
        } else if n.role != Role::Definition {
            definitions.remove(&parent);
        }
        if !selected(n.role)
            || (link_text[i] > 0
                && link_text[i] >= n.text.chars().filter(|c| c.is_alphanumeric()).count()
                && n.role != Role::Row)
            || matches!(n.role, Role::Text)
                && ancestors(nodes, i).skip(1).any(|a| {
                    selected(nodes[a].role) || matches!(nodes[a].role, Role::Heading { level: _ })
                })
        {
            continue;
        }
        let mut quote_offset = 0;
        let mut text = if n.role == Role::Definition {
            let Some((label, consumed)) = definitions.get_mut(&parent) else {
                continue;
            };
            *consumed = true;
            if label.is_empty() || n.text.trim().is_empty() {
                continue;
            }
            if label.chars().take(MAX_PROSE_CHARS + 1).count() > MAX_PROSE_CHARS {
                continue;
            }
            quote_offset = label.len() + 1;
            Cow::Owned(format!("{label}\n{}", n.text.trim()))
        } else if n.role == Role::Row {
            let cells = nodes[i + 1..]
                .iter()
                .enumerate()
                .take_while(|(_, n)| n.parent_index().is_some_and(|p| p >= i))
                .filter(|(offset, n)| {
                    !excluded[i + 1 + offset]
                        && n.parent_index() == Some(i)
                        && matches!(n.role, Role::Cell | Role::CellHeader)
                })
                .map(|(_, n)| n)
                .collect::<Vec<_>>();
            let table = ancestors(nodes, i).find(|&a| nodes[a].role == Role::Table);
            let row = cells
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>()
                .join("\t");
            if cells.iter().all(|c| c.text.trim().is_empty()) {
                continue;
            }
            if cells.iter().all(|c| c.role == Role::CellHeader) {
                if let Some(table) = table {
                    headers.remove(&table);
                    if row.chars().count() <= MAX_PROSE_CHARS {
                        headers.insert(table, (row, cells.len()));
                    }
                }
                continue;
            }
            if let Some((header, _)) = table
                .and_then(|table| headers.get(&table))
                .filter(|(_, count)| *count == cells.len())
            {
                quote_offset = header.len() + 1;
                Cow::Owned(format!("{header}\n{row}"))
            } else {
                Cow::Owned(row)
            }
        } else {
            Cow::Borrowed(if n.role == Role::CodeBlock {
                n.text.as_str()
            } else {
                n.text.trim()
            })
        };
        let Some(excerpt) = cut(&text, n.role == Role::CodeBlock) else {
            continue;
        };
        if excerpt.trim().is_empty() {
            continue;
        }
        let structured = matches!(n.role, Role::Definition | Role::Row | Role::ListItem);
        if structured && excerpt.len() != text.len() {
            continue;
        }

        let ancillary = |s: &str| {
            matches!(
                s.trim().trim_end_matches(':').to_ascii_lowercase().as_str(),
                "see also"
                    | "comments"
                    | "references"
                    | "citations"
                    | "contents"
                    | "table of contents"
                    | "further reading"
                    | "external links"
            )
        };
        if path.iter().any(|(_, h, _)| ancillary(&nodes[*h].text))
            || [
                "main article:",
                "see also:",
                "for other uses",
                "this article is about",
            ]
            .iter()
            .any(|s| excerpt.to_ascii_lowercase().starts_with(s))
        {
            continue;
        }
        let length = excerpt.len();
        let opening = tokens(excerpt)
            .take(SUBJECT_START_WORDS)
            .filter_map(|t| terms.get(&t))
            .fold(0, |mask, i| mask | (1 << i));
        let matched = relevant(excerpt);
        match &mut text {
            Cow::Borrowed(s) => *s = &s[..length],
            Cow::Owned(s) => s.truncate(length),
        }
        out.push(Block {
            doc: page,
            node: i,
            quote_offset,
            subject_heading: path
                .iter()
                .filter(|(level, _, mask)| *level > 1 && *mask != 0)
                .max_by_key(|(_, _, mask)| mask.count_ones())
                .map(|(_, index, _)| *index),
            path_terms: path
                .iter()
                .filter(|(level, _, _)| *level != 1)
                .fold(0, |m, (_, _, t)| m | t),
            structured,
            terms: matched,
            opening,
            heading_terms: heading.map_or(0, |(_, mask)| mask),
            text,
            scope,
            heading: heading.map(|(i, _)| i),
            heading_end: 0,
            accepted: ancestors(nodes, i).any(|a| nodes[a].marks.has(Marks::ACCEPTED)),
            suggested: ancestors(nodes, i).any(|a| nodes[a].marks.has(Marks::SUGGESTED)),
        });
    }
    for block in &mut out {
        if let Some(heading) = block.heading {
            block.heading_end = heading_ends[heading];
        }
    }
    out
}

fn cut(s: &str, code: bool) -> Option<&str> {
    let end = if code {
        s.floor_char_boundary(MAX_CODE_BYTES)
    } else {
        s.char_indices()
            .nth(MAX_PROSE_CHARS)
            .map_or(s.len(), |(i, _)| i)
    };
    if end == s.len() {
        return Some(s);
    }
    let prefix = &s[..end];
    if code {
        prefix.rsplit_once('\n').map(|(s, _)| s)
    } else {
        prefix
            .rfind(['.', '!', '?', '。', '！', '？'])
            .and_then(|i| Some(&prefix[..i + prefix[i..].chars().next()?.len_utf8()]))
    }
}

fn make(b: &Block<'_>, doc: &Doc) -> Option<Passage> {
    let n = &doc.nodes()[b.node];
    let code = n.role == Role::CodeBlock;
    let text = cut(&b.text, code)?;
    if text.is_empty() {
        return None;
    }
    let heading = b.heading.map(|i| &doc.nodes()[i]);
    let locator = Locator::for_text(
        doc,
        b.node,
        text.get(b.quote_offset..)?,
        heading.and_then(|n| n.selector_id()),
    )?;
    Some(Passage {
        text: text.to_string(),
        code,
        language: n.language.clone(),
        source: PageKey::from_url(doc.page()).ok()?,
        source_title: clip(&doc.head().title, TITLE_BYTES),
        heading: heading.map(|n| clip(&n.text, TITLE_BYTES)),
        locator,
        confidence: Confidence::High,
    })
}

fn paired(intro: &Block<'_>, command: &Block<'_>, doc: &Doc) -> Option<Passage> {
    let mut passage = make(intro, doc)?;
    let command_text = cut(&command.text, true)?;
    passage.text.push('\n');
    passage.text.push_str(command_text);
    let end = Locator::for_text(doc, command.node, command_text, None)?;
    let end_quote = end.quote?;
    let range = passage.locator.quote.as_mut()?;
    range.end = Some(end_quote.end.unwrap_or(end_quote.start));
    range.suffix = end_quote.suffix;
    passage.locator.position = passage
        .locator
        .position
        .zip(end.position)
        .map(|(start, end)| crate::locator::TextPosition {
            start: start.start,
            end: end.end,
        });
    passage.code = true;
    passage
        .language
        .clone_from(&doc.nodes()[command.node].language);
    Some(passage)
}

fn bits(mut mask: u64) -> impl Iterator<Item = usize> {
    std::iter::from_fn(move || {
        if mask == 0 {
            return None;
        }
        let i = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        Some(i)
    })
}

pub fn read_passages(q: &QueryRead, docs: &[&Doc]) -> Vec<Passage> {
    let QueryKind::Passage { kind } = q.kind() else {
        return Vec::new();
    };
    let terms = query_terms(q);
    if terms.is_empty() || terms.len() > MAX_CONTENT_TERMS {
        return Vec::new();
    }
    let terms: BTreeMap<_, _> = terms.into_iter().enumerate().map(|(i, t)| (t, i)).collect();
    let vocabulary = if *kind == PassageKind::Question {
        question::vocabulary(&terms)
    } else {
        terms.clone()
    };
    let blocks: Vec<_> = docs
        .iter()
        .take(MAX_RESULT_PAGES)
        .enumerate()
        .filter(|(_, d)| !challenge(d))
        .flat_map(|(i, d)| blocks(i, d, &vocabulary))
        .collect();
    if *kind == PassageKind::Question {
        if let Some(sides) = question::comparison(q) {
            return question::contrast(sides, docs, &blocks);
        }
    }
    let mut freq = [0usize; MAX_CONTENT_TERMS];
    for b in &blocks {
        for i in bits(b.terms) {
            freq[i] += 1;
        }
    }
    let mut scores = vec![0; blocks.len()];
    let core: Vec<_> = q.diagnostic.iter().flat_map(|s| content_words(s)).collect();
    let distinctive = terms.keys().any(|t| {
        t.ends_with("error")
            || t.ends_with("exception")
            || t.len() == 5
                && t.starts_with('e')
                && t.as_bytes()[1..].iter().all(u8::is_ascii_digit)
    });
    let numbers = terms
        .iter()
        .filter(|(t, _)| t.bytes().all(|b| b.is_ascii_digit()))
        .fold(0u64, |mask, (_, i)| mask | (1 << i));
    for (i, b) in blocks.iter().enumerate() {
        let overlap = (b.terms | b.heading_terms).count_ones() as usize;
        match kind {
            PassageKind::Question => {
                let near = b.opening | b.heading_terms;
                if near & numbers != numbers
                    || near.count_ones() as usize != terms.len()
                    || b.terms == 0 && !(b.accepted && overlap == terms.len())
                    || b.text.trim_end().ends_with('?')
                {
                    continue;
                }
                let weight: usize = bits(b.terms)
                    .map(|i| 1 + (blocks.len() / freq[i]).ilog2() as usize)
                    .sum();
                scores[i] = weight
                    + b.heading_terms.count_ones() as usize * 2
                    + usize::from(b.accepted) * 8
                    + usize::from(b.suggested) * 3;
            }
            PassageKind::Pasted => {
                if b.terms.count_ones() as usize != terms.len() || !distinctive && terms.len() < 2 {
                    continue;
                }
                let candidate: Vec<_> = content_words(&b.text).collect();
                if core.is_empty() || !candidate.windows(core.len()).any(|w| w == core) {
                    continue;
                }
                scores[i] = scores[i].max(1 + terms.len());
                let nodes = docs[b.doc].nodes();
                let issue_parent = b.heading.and_then(|h| nodes[h].parent_index());
                for (distance, next) in blocks[i + 1..]
                    .iter()
                    .take(ERROR_FOLLOWING_BLOCKS)
                    .enumerate()
                {
                    if next.doc != b.doc {
                        break;
                    }
                    let nested_remedy = next.heading.is_some_and(|h| {
                        (b.node..b.heading_end).contains(&h)
                            && issue_parent.is_none_or(|p| ancestors(nodes, h).any(|a| a == p))
                            && tokens(&nodes[h].text).any(|s| {
                                matches!(s.as_str(), "fix" | "solution" | "resolution" | "answer")
                            })
                    });
                    if next.scope != b.scope || !nested_remedy && next.heading != b.heading {
                        break;
                    }
                    if next.accepted || nodes[next.node].role == Role::CodeBlock {
                        scores[i + distance + 1] = scores[i + distance + 1]
                            .max(terms.len() + ERROR_FOLLOWING_BLOCKS + 1 - distance);
                        break;
                    }
                }
            }
        }
    }
    let mut passages = Vec::new();
    for (page, doc) in docs.iter().enumerate().take(MAX_RESULT_PAGES) {
        let local: Vec<_> = blocks.iter().filter(|b| b.doc == page).collect();
        if local.is_empty() {
            continue;
        }
        let article_heading = doc.nodes().iter().enumerate().find(|(i, n)| {
            matches!(n.role, Role::Heading { level: Some(1) })
                && visible(doc.nodes(), *i)
                && !ancestors(doc.nodes(), *i).any(|a| {
                    matches!(
                        doc.nodes()[a].role,
                        Role::Landmark {
                            landmark: Some(Landmark::Banner | Landmark::Navigation)
                        }
                    )
                })
        });
        let titles = std::iter::once(doc.head().title.as_str())
            .chain(article_heading.map(|(_, n)| n.text.as_str()));
        let diagnostic = q.diagnostic.as_deref().and_then(|s| {
            tokens(s)
                .find(|t| {
                    t.len() == 5
                        && t.starts_with('e')
                        && t.as_bytes()[1..].iter().all(u8::is_ascii_digit)
                })
                .or_else(|| {
                    s.split_once(':')
                        .map(|(name, _)| name.to_owned())
                        .filter(|n| n.ends_with("error") || n.ends_with("exception"))
                })
        });
        let canonical_error = diagnostic
            .is_some_and(|code| titles.clone().any(|title| tokens(title).any(|t| t == code)));
        let definition = q.terms().starts_with(&["what".to_owned(), "is".to_owned()])
            || q.terms()
                .last()
                .is_some_and(|w| w.trim_end_matches('?') == "meaning");
        let canonical_definition = definition
            && titles.clone().any(|title| {
                title
                    .split(['|', '—'])
                    .flat_map(|s| s.split(" - "))
                    .next()
                    .is_some_and(|name| {
                        tokens(name)
                            .filter(|t| !STOP.contains(&t.as_str()))
                            .collect::<BTreeSet<_>>()
                            .iter()
                            .eq(terms.keys())
                    })
            });
        let numbered_reference = definition
            && numbers != 0
            && titles.clone().any(|name| {
                let present: BTreeSet<_> = tokens(name).collect();
                terms
                    .iter()
                    .filter(|(_, i)| numbers & (1 << *i) != 0)
                    .all(|(t, _)| present.contains(t))
            });
        let lead = local.iter().copied().find(|b| {
            doc.nodes()[b.node].role == Role::Paragraph
                && (doc.nodes()[b.node].marks.has(Marks::PROSE)
                    || b.opening.count_ones() as usize == terms.len())
                && b.text.chars().any(char::is_alphabetic)
                && !ancestors(doc.nodes(), b.node)
                    .any(|a| matches!(doc.nodes()[a].role, Role::Table | Role::ListItem))
                && (canonical_error
                    || canonical_definition
                    || numbered_reference
                        && (b.opening | b.heading_terms).count_ones() as usize == terms.len())
        });
        let canonical = lead.and_then(|b| {
            if canonical_error {
                let code = local
                    .iter()
                    .copied()
                    .skip_while(|next| next.node <= b.node)
                    .take_while(|next| next.scope == b.scope && next.heading == b.heading)
                    .find(|next| doc.nodes()[next.node].role == Role::CodeBlock)?;
                paired(b, code, doc)
            } else if canonical_definition || numbered_reference {
                make(b, doc)
            } else {
                None
            }
        });
        let best = blocks
            .iter()
            .enumerate()
            .filter(|(i, b)| b.doc == page && scores[*i] > 0)
            .max_by_key(|(i, _)| (scores[*i], std::cmp::Reverse(*i)));
        let question = question::Question::new(q, &vocabulary, terms.len(), doc);
        let targeted = (*kind == PassageKind::Question && !definition)
            .then(|| question.best(&local, doc, &vocabulary))
            .flatten();
        let selected = canonical.or(targeted).or_else(|| {
            let (i, b) = best?;
            if *kind == PassageKind::Question && !question.permits(&b.text) {
                return None;
            }
            if definition && numbers != 0 && !numbered_reference && b.heading_terms == 0 {
                return None;
            }
            if *kind == PassageKind::Question && !definition {
                if matches!(
                    doc.nodes()[b.node].role,
                    Role::Text | Role::ListItem | Role::Document
                ) && !b.text.contains(['.', '!', ':'])
                    && !b.accepted
                {
                    return None;
                }
                let code = |b: &Block<'_>| doc.nodes()[b.node].role == Role::CodeBlock;
                let pair = if code(b) {
                    i.checked_sub(1)
                        .and_then(|i| blocks.get(i))
                        .map(|intro| (intro, b))
                } else {
                    blocks.get(i + 1).map(|next| (b, next))
                };
                if let Some((intro, cmd)) = pair.filter(|(a, c)| {
                    a.doc == page
                        && c.doc == page
                        && a.heading == c.heading
                        && a.scope == c.scope
                        && doc.nodes()[a.node].role == Role::Paragraph
                        && code(c)
                }) {
                    if intro.opening == 0 && intro.heading_terms == 0 {
                        return None;
                    }
                    return paired(intro, cmd, doc);
                }
            }
            make(b, doc)
        });
        if let Some(p) = selected {
            if !passages
                .iter()
                .any(|v: &Passage| v.source.page_match() == p.source.page_match())
            {
                passages.push(p);
            }
        }
        if passages.len() >= MAX_PASSAGES {
            break;
        }
    }
    passages
}

pub(crate) fn featured(q: &QueryRead, f: &Featured, result: &Organic) -> Option<Passage> {
    let terms = query_terms(q);
    let present: BTreeSet<_> = tokens(&f.text).collect();
    if terms.is_empty()
        || !terms.is_subset(&present)
        || !question::TaskQualifiers::new(q.terms(), &result.url).permits(&f.text)
    {
        return None;
    }
    let text = cut(&f.text, false)?.to_owned();
    Some(Passage {
        locator: quote(&text)?,
        text,
        code: false,
        language: None,
        source: f.cites.clone(),
        source_title: result.title.clone(),
        heading: None,
        confidence: Confidence::High,
    })
}

#[cfg(test)]
#[test]
fn featured_excerpts_obey_the_same_task_qualifiers_as_articles() {
    let url = url::Url::parse("https://example.com/guide").unwrap();
    let key = crate::PageKey::from_url(&url).unwrap();
    let result = Organic {
        key: key.clone(),
        url,
        title: "Loom guide".into(),
        snippet: None,
        rank: 1,
        official: None,
        discussion: false,
        sitelinks: Vec::new(),
    };
    for query in ["loom remove all spools", "how to remove all spools?"] {
        let q = crate::read_query(query).unwrap();
        for (text, accepted) in [
            ("Loom can remove all spools without thread.", false),
            ("In Loom, remove all spools before restarting.", true),
        ] {
            assert_eq!(
                featured(
                    &q,
                    &Featured {
                        text: text.into(),
                        cites: key.clone()
                    },
                    &result
                )
                .is_some(),
                accepted
            );
        }
    }
}
