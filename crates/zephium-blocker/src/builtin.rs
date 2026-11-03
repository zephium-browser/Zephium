//! Zephium's own filter rules, compiled after every catalog's lists. They are
//! reviewed code shipped with the app, never fetched.

use crate::sha256::Sha256;
use crate::{FilterSource, SourceFormat, SourceId};

const ID: &str = "zephium";
const RULES: &str = include_str!("zephium_rules.txt");

pub(crate) fn source() -> FilterSource {
    FilterSource::new(
        SourceId::new(ID).expect("the built-in source ID is valid"),
        SourceFormat::Standard,
        RULES.to_owned(),
    )
}

/// Changes whenever the rules do, so compiled artifacts made without them are
/// not reused.
pub(crate) fn fingerprint() -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"zephium-builtin-rules-v1");
    digest.update(RULES);
    digest.finalize()
}
