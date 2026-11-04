//! Zephium's own filter rules, compiled after every catalog's lists where
//! requests carry their page. They are reviewed code shipped with the app,
//! never fetched.

use crate::sha256::Sha256;
use crate::{CompileTarget, FilterSource, SourceFormat, SourceId};

/// Reserved: no catalog source may use it.
pub(crate) const ID: &str = "zephium";
const RULES: &str = include_str!("zephium_rules.txt");

pub(crate) fn source() -> FilterSource {
    FilterSource::new(
        SourceId::new(ID).expect("the built-in source ID is valid"),
        SourceFormat::Standard,
        RULES.to_owned(),
    )
}

/// Whether these rules compile for `target`. They are per-site exceptions,
/// so they need requests that carry their page: WebKit's always do, and the
/// runtime matcher's do when it judges them against the view's main document.
/// A runtime matcher without that would apply them on every site.
pub(crate) const fn applies_to(target: CompileTarget) -> bool {
    match target {
        CompileTarget::WebKit => true,
        CompileTarget::Runtime => cfg!(feature = "runtime-exact"),
    }
}

/// Physical lines and candidate rules, as the compiler counts them.
pub(crate) fn line_counts() -> (usize, usize) {
    crate::compiler::line_counts(SourceFormat::Standard, RULES)
}

pub(crate) const fn byte_len() -> usize {
    RULES.len()
}

/// Changes whenever the rules do, so compiled artifacts made without them are
/// not reused.
pub(crate) fn fingerprint() -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"zephium-builtin-rules-v1");
    digest.update(RULES);
    digest.finalize()
}
