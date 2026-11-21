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
