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
/// and the runtime matcher sees requests without their page on Windows, where
/// a site-scoped exception would apply on every site.
pub(crate) const fn applies_to(target: CompileTarget) -> bool {
    matches!(target, CompileTarget::WebKit)
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
