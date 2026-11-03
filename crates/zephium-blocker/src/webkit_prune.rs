//! Wildcard block rules that matched nothing in the recorded page corpus (see
//! docs/adblock.md). WebKit gives every rule with `.*` an automaton of its own
//! and runs them all on every load, so these rules carried most of the list's
//! per-load cost while blocking nothing seen. Entries are SHA-256 digests of a
//! rule exactly as it is emitted for WebKit: an upstream edit or a converter
//! change emits a different rule, and that rule is kept.

use std::collections::HashSet;
use std::sync::OnceLock;

use crate::sha256::Sha256;

const SHIPPED: &str = include_str!("webkit_pruned.txt");

#[derive(Debug)]
pub(crate) struct PrunedRules {
    digests: HashSet<[u8; 32]>,
    fingerprint: [u8; 32],
}

impl PrunedRules {
    pub(crate) fn shipped() -> &'static Self {
        static SHIPPED_RULES: OnceLock<PrunedRules> = OnceLock::new();
        SHIPPED_RULES.get_or_init(|| Self::parse(SHIPPED))
    }

    pub(crate) fn none() -> &'static Self {
        static NONE: OnceLock<PrunedRules> = OnceLock::new();
        NONE.get_or_init(|| Self::parse(""))
    }

    /// An unreadable line is skipped, so its rule stays.
    pub(crate) fn parse(text: &str) -> Self {
        let digests = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(decode)
            .collect();
        let mut fingerprint = Sha256::new();
        fingerprint.update(b"zephium-webkit-pruned-rules-v1");
        fingerprint.update(text);
        Self {
            digests,
            fingerprint: fingerprint.finalize(),
        }
    }

    pub(crate) fn contains(&self, emitted_rule: &[u8]) -> bool {
        !self.digests.is_empty() && self.digests.contains(&webkit_rule_digest(emitted_rule))
    }

    pub(crate) const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}

/// The digest a pruned-rule entry holds for one emitted WebKit rule, given as
/// its canonical JSON object.
pub fn webkit_rule_digest(emitted_rule: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(emitted_rule);
    digest.finalize()
}

fn decode(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0; 32];
    for (byte, pair) in digest.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_set_is_sorted_unique_lowercase_digests_only() {
        let entries: Vec<&str> = SHIPPED
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        assert!(!entries.is_empty());
        for entry in &entries {
            assert!(
                entry.len() == 64
                    && entry
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "not a lowercase SHA-256 digest: {entry:?}"
            );
        }
        assert!(
            entries.windows(2).all(|pair| pair[0] < pair[1]),
            "entries must be sorted and unique"
        );
        assert_eq!(PrunedRules::shipped().digests.len(), entries.len());
    }

    #[test]
    fn unreadable_lines_are_skipped() {
        let rules = PrunedRules::parse("# header\nnot-a-digest\n\nab\n");
        assert!(rules.digests.is_empty());
        assert!(!rules.contains(b"{}"));
    }
}
