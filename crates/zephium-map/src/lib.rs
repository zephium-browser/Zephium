#![forbid(unsafe_code)]

pub mod doc;
pub mod key;
pub mod query;
pub mod results;

pub use doc::{Doc, DocBuilder, DocError, DocLimits, Node};
pub use key::{Locator, PageKey, SiteKey};
pub use query::{read_query, QueryKind, QueryRead, SectionRole};
pub use results::{read_results, FirstLayer, ResultsRead, Wall};

pub(crate) fn flatten(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub mod compare;
mod linked;
pub mod passage;
pub mod robots;
pub mod session;
pub mod site;
pub use passage::Passage;
pub use session::{Config, Effect, Input, MapEvent, Session};
pub use site::{Entry, Place, PlaceNotes, Source};

fn ancestors(nodes: &[Node], at: usize) -> impl Iterator<Item = usize> + '_ {
    std::iter::successors(Some(at), |&i| nodes[i].parent)
}
fn visible(nodes: &[Node], at: usize) -> bool {
    ancestors(nodes, at).all(|i| {
        !nodes[i].hidden
            && !nodes[i].marks.has(doc::Marks::SPONSORED)
            && !nodes[i].marks.has(doc::Marks::GENERATED)
    })
}
