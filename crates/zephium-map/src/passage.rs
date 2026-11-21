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
const ERROR_FOLLOWING_BLOCKS: usize = 3;
const STOP: &[&str] = &[
    "a", "an", "the", "how", "do", "does", "i", "to", "is", "are", "was", "be", "can", "in", "of",
    "on", "and", "or", "my", "what", "where", "when", "why", "who", "for", "with", "it", "at",
    "from", "error", "please", "that", "this", "by",
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
        q.terms().iter().flat_map(|t| content_words(t)).collect()
    }
}
struct Block<'a> {
    doc: usize,
    text: Cow<'a, str>,
    node: usize,
    heading: Option<usize>,
    scope: Option<usize>,
    accepted: bool,
    suggested: bool,
    terms: u64,
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
    let mut elements = vec![1usize; nodes.len()];
    let mut excluded = Vec::with_capacity(nodes.len());
    for n in nodes {
        excluded.push(
            n.hidden
                || n.marks.has(Marks::SPONSORED)
                || n.marks.has(Marks::GENERATED)
                || forbidden(n.role)
                || n.parent_index().is_some_and(|p| excluded[p]),
        );
    }
    let mut has_main = false;
    for (i, n) in nodes.iter().enumerate().rev() {
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
    let mut headings = BTreeMap::new();
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
            || !has_main && dense.is_some_and(|d| !ancestors(nodes, i).any(|a| a == d))
        {
            continue;
        }
        if matches!(n.role, Role::Heading { level: _ }) {
            headings.insert(scope, (i, relevant(&n.text)));
            continue;
        }
        if !selected(n.role)
            || matches!(n.role, Role::Text)
                && ancestors(nodes, i).skip(1).any(|a| {
                    selected(nodes[a].role) || matches!(nodes[a].role, Role::Heading { level: _ })
                })
        {
            continue;
        }
        let text = if n.role == Role::Row {
            let cells = nodes[i + 1..]
                .iter()
                .enumerate()
                .take_while(|(_, n)| n.parent_index().is_some_and(|p| p >= i))
                .filter(|(offset, n)| {
                    visible(nodes, i + 1 + offset)
                        && n.parent_index() == Some(i)
                        && matches!(n.role, Role::Cell | Role::CellHeader)
                })
                .map(|(_, n)| n.text.as_str())
                .collect::<Vec<_>>()
                .join("\t");
            Cow::Owned(cells)
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
        let heading = headings.get(&scope).copied();

        out.push(Block {
            doc: page,
            node: i,
            terms: relevant(excerpt),
            heading_terms: heading.map_or(0, |(_, mask)| mask),
            text,
            scope,
            heading: heading.map(|(i, _)| i),
            accepted: ancestors(nodes, i).any(|a| nodes[a].marks.has(Marks::ACCEPTED)),
            suggested: ancestors(nodes, i).any(|a| nodes[a].marks.has(Marks::SUGGESTED)),
        });
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
        text,
        heading.and_then(|n| n.element_id.as_deref()),
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
    let blocks: Vec<_> = docs
        .iter()
        .take(MAX_RESULT_PAGES)
        .enumerate()
        .filter(|(_, d)| !challenge(d))
        .flat_map(|(i, d)| blocks(i, d, &terms))
        .collect();
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
    for (i, b) in blocks.iter().enumerate() {
        let overlap = (b.terms | b.heading_terms).count_ones() as usize;
        match kind {
            PassageKind::Question => {
                if b.terms == 0 && !(b.accepted && overlap == terms.len())
                    || overlap * 3 < terms.len() * 2
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
                for (distance, next) in blocks[i + 1..]
                    .iter()
                    .take(ERROR_FOLLOWING_BLOCKS)
                    .enumerate()
                {
                    if next.doc != b.doc {
                        break;
                    }
                    let remedy = next.heading.is_some_and(|h| {
                        tokens(&docs[next.doc].nodes()[h].text).any(|s| {
                            matches!(s.as_str(), "fix" | "solution" | "resolution" | "answer")
                        })
                    });
                    if !next.accepted
                        && !remedy
                        && (next.scope != b.scope || next.heading != b.heading)
                    {
                        break;
                    }
                    if next.accepted || docs[next.doc].nodes()[next.node].role == Role::CodeBlock {
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
        let best = blocks
            .iter()
            .enumerate()
            .filter(|(i, b)| b.doc == page && scores[*i] > 0)
            .max_by_key(|(i, _)| (scores[*i], std::cmp::Reverse(*i)));
        if let Some(p) = best.and_then(|(_, b)| make(b, doc)) {
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
    if terms.is_empty() || !terms.is_subset(&present) {
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
