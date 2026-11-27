//! Read-only document style policy supplied by the bounded blocker compiler.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::ContentRuleDigest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentStyleFailure {
    InvalidDocument,
    ResourceLimit,
    Unavailable,
}

/// Public-list policy only: no profile preferences, private-session data or
/// personal element selections may be retained here. Instances are shared by
/// every profile using the exact same source artifact.
pub trait DocumentStyleProvider: Send + Sync + std::fmt::Debug {
    fn fingerprint(&self) -> ContentRuleDigest;
    /// Prepared off the UI thread. Generic selectors are a lookup table, not
    /// a stylesheet: the document installs only selectors for observed tokens.
    fn document_plan(&self, document_url: &str) -> Result<DocumentStylePlan, DocumentStyleFailure>;
    /// Match untrusted, bounded document tokens on the style worker. The page
    /// receives only selectors, never the shared generic lookup table.
    fn generic_selectors(
        &self,
        document_url: &str,
        tokens: &[String],
    ) -> Result<Vec<String>, DocumentStyleFailure>;
    /// The scriptlet program every frame of the profile runs at document start
    /// in the page's main world, when any host has scriptlets.
    fn scriptlet_program(&self) -> Option<Arc<str>> {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentStylePlan {
    pub css: Arc<str>,
    pub generic_index: Arc<str>,
    /// Computed once by the immutable policy, shared with every document.
    pub generic_index_digest: ContentRuleDigest,
    pub exceptions: Arc<str>,
}

impl DocumentStylePlan {
    pub fn empty() -> Self {
        Self {
            css: Arc::from(""),
            generic_index: Arc::from("[]"),
            generic_index_digest: ContentRuleDigest::from_bytes(Sha256::digest(b"[]").into()),
            exceptions: Arc::from("[]"),
        }
    }

    /// Exact subscription content identity without rehashing the shared index.
    pub fn fingerprint(&self) -> ContentRuleDigest {
        let mut hash = Sha256::new();
        hash.update(b"zephium-document-style-v2\0");
        hash.update((self.css.len() as u64).to_le_bytes());
        hash.update(self.css.as_bytes());
        hash.update(self.generic_index_digest.as_bytes());
        hash.update((self.exceptions.len() as u64).to_le_bytes());
        hash.update(self.exceptions.as_bytes());
        ContentRuleDigest::from_bytes(hash.finalize().into())
    }
}
