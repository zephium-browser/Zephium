//! Public search on the person's own search engine: the run opens its
//! results page in the agent browser, as if they had typed the query in the
//! address bar, and Rust reads the results off the page. No model call and
//! no search fee. A page that cannot be used (a challenge, a consent wall,
//! no results) falls back to a provider's search a few times per run.

use std::sync::atomic::{AtomicU8, Ordering};

use zephium_core::search::SearchEngine;
use zephium_core::work::context::WorkContextBody;
use zephium_core::work::runtime::{WorkExecutionLimits, WorkUsage, WorkUsageAccounting};
use zephium_core::work::search::*;
use zephium_core::work::WorkError;

/// Provider searches one run may fall back to when a results page cannot
/// be used. Each costs a model call and a search fee.
pub const BROWSER_SEARCH_FALLBACKS: u8 = 3;
const TITLE_BYTES: usize = 512;

/// What one browser search costs: a step, no tokens and no money.
const BROWSER_SEARCH_USAGE: WorkUsage = WorkUsage {
    model_tokens: 0,
    cost_micro_usd: 0,
    operations: 1,
    accounting: WorkUsageAccounting::Exact,
};

/// A query parameter an engine reads, and its value.
type Param = Option<(&'static str, &'static str)>;
/// The lead's freshness, as it appends it to a query, and each engine's
/// own parameter for it.
const FRESHNESS: [(&str, [Param; 4]); 4] = [
    (
        " (past day)",
        [
            Some(("tbs", "qdr:d")),
            Some(("df", "d")),
            Some(("filters", "ex1:\"ez1\"")),
            Some(("tf", "pd")),
        ],
    ),
    (
        " (past week)",
        [
            Some(("tbs", "qdr:w")),
            Some(("df", "w")),
            Some(("filters", "ex1:\"ez2\"")),
            Some(("tf", "pw")),
        ],
    ),
    (
        " (past month)",
        [
            Some(("tbs", "qdr:m")),
            Some(("df", "m")),
            Some(("filters", "ex1:\"ez3\"")),
            Some(("tf", "pm")),
        ],
    ),
    (
        " (past year)",
        [
            Some(("tbs", "qdr:y")),
            Some(("df", "y")),
            None,
            Some(("tf", "py")),
        ],
    ),
];

/// Search on the person's engine, with a capped provider fallback.
pub struct BrowserSearch {
    engine: SearchEngine,
    template: String,
    /// A private Kagi session token. Never included in a diagnostic.
    token: String,
    fallback: Option<(Box<dyn WorkPublicSearchProvider>, WorkSearchFallbackV1)>,
    fallbacks: AtomicU8,
}

impl BrowserSearch {
    /// `template` is the person's custom engine address with
    /// `{searchTerms}`; it is refused for a custom engine without a valid one.
    pub fn try_new(
        engine: SearchEngine,
        template: &str,
        token: &str,
    ) -> Result<Self, WorkError> {
        if engine == SearchEngine::Custom && !zephium_core::search::valid_template(template) {
            return Err(WorkError::Unavailable);
        }
        Ok(Self {
            engine,
            template: template.to_owned(),
            token: token.trim().to_owned(),
            fallback: None,
            fallbacks: AtomicU8::new(0),
        })
    }

    /// The provider search a page that cannot be used falls back to.
    pub fn with_fallback(
        mut self,
        search: Box<dyn WorkPublicSearchProvider>,
        grant: WorkSearchFallbackV1,
    ) -> Result<Self, WorkError> {
        grant.validate()?;
        self.fallback = Some((search, grant));
        Ok(self)
    }

    fn mine(&self, scope: &WorkPublicSearchScope) -> bool {
        scope.validate().is_ok()
            && scope.provider == WorkSearchProvider::Browser
            && scope.model == self.engine.id()
    }

    /// The results page for a query, its freshness as the engine's own
    /// parameter. The navigation policy's sensitive-input guard decides
    /// whether the query may leave at all.
    fn page(&self, query: &str) -> Option<url::Url> {
        let (query, fresh) = FRESHNESS
            .iter()
            .find_map(|(suffix, params)| {
                let base = query.strip_suffix(suffix)?;
                let slot = match self.engine {
                    SearchEngine::Google => 0,
                    SearchEngine::DuckDuckGo => 1,
                    SearchEngine::Bing => 2,
                    SearchEngine::Brave => 3,
                    SearchEngine::Kagi | SearchEngine::Custom => return Some((base, None)),
                };
                Some((base, params[slot]))
            })
            .unwrap_or((query, None));
        let mut url = self
            .engine
            .configured_search(query, &self.template, &self.token)?;
        if let Some((key, value)) = fresh {
            url.query_pairs_mut().append_pair(key, value);
        }
        Some(url)
    }

    /// The fallback search, while the run has some left; nothing is sent
    /// past the cap.
    async fn fall_back(
        &self,
        scope: &WorkPublicSearchScope,
        limits: WorkExecutionLimits,
    ) -> Result<WorkPublicSearchResult, WorkPublicSearchError> {
        let unavailable = WorkPublicSearchError::NotDispatched(WorkError::Unavailable);
        let Some((search, grant)) = &self.fallback else {
            return Err(unavailable);
        };
        if self
            .fallbacks
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                (used < BROWSER_SEARCH_FALLBACKS).then_some(used + 1)
            })
            .is_err()
        {
            return Err(unavailable);
        }
        let scope = WorkPublicSearchScope {
            provider: grant.provider,
            model: grant.model.clone(),
            query: scope.query.clone(),
        };
        let result = search.search(&scope, &[], limits).await;
        // A search that was never sent leaves the run's fallbacks as they were.
        if matches!(result, Err(WorkPublicSearchError::NotDispatched(_))) {
            self.fallbacks.fetch_sub(1, Ordering::SeqCst);
        }
        result
    }

    fn evidence(&self, hits: Vec<WorkSearchHit>) -> Option<WorkProviderSearchEvidenceV1> {
        let mut citations: Vec<WorkProviderSearchCitation> = Vec::new();
        for hit in hits {
            if citations.len() >= BROWSER_SEARCH_MAX_RESULTS {
                break;
            }
            if citations.iter().any(|known| known.url == hit.url) {
                continue;
            }
            let title = clip(&one_line(&hit.title), TITLE_BYTES);
            if title.is_empty() {
                continue;
            }
            citations.push(WorkProviderSearchCitation {
                url: hit.url,
                title,
                start_index: 0,
                end_index: 0,
                snippet: hit
                    .snippet
                    .map(|snippet| clip(&one_line(&snippet), BROWSER_SEARCH_SNIPPET_BYTES))
                    .filter(|snippet| !snippet.is_empty()),
            });
        }
        let evidence = WorkProviderSearchEvidenceV1 {
            version: 1,
            provider: WorkSearchProvider::Browser,
            model: self.engine.id().to_owned(),
            response_model: self.engine.id().to_owned(),
            response_id: format!("zb_{}", ulid::Ulid::new()),
            search_call_id: format!("zq_{}", ulid::Ulid::new()),
            answer: String::new(),
            citations,
            actual_input_tokens: 0,
            actual_output_tokens: 0,
        };
        evidence.validate().is_ok().then_some(evidence)
    }
}

impl WorkPublicSearchProvider for BrowserSearch {
    fn grant(&self) -> Option<(WorkSearchProvider, String)> {
        Some((WorkSearchProvider::Browser, self.engine.id().to_owned()))
    }

    fn fallback(&self) -> Option<WorkSearchFallbackV1> {
        self.fallback.as_ref().map(|(_, grant)| grant.clone())
    }

    fn minimum_reservation(
        &self,
        scope: &WorkPublicSearchScope,
        _context: &[WorkContextBody],
    ) -> Option<WorkUsage> {
        self.mine(scope).then_some(BROWSER_SEARCH_USAGE)
    }

    fn results_page(&self, scope: &WorkPublicSearchScope) -> Option<WorkResultsPage> {
        if !self.mine(scope) {
            return None;
        }
        Some(WorkResultsPage {
            engine: self.engine,
            url: self.page(&scope.query)?,
        })
    }

    fn search_page<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        page: Option<WorkSearchPage>,
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            if !self.mine(scope) || limits.validate().is_err() {
                return Err(WorkPublicSearchError::NotDispatched(WorkError::Invalid));
            }
            if let Some(WorkSearchPage::Results(hits)) = page {
                if let Some(evidence) = self.evidence(hits) {
                    if !BROWSER_SEARCH_USAGE.within(limits) {
                        return Err(WorkPublicSearchError::NotDispatched(WorkError::Capacity));
                    }
                    return Ok(WorkPublicSearchResult {
                        evidence,
                        usage: BROWSER_SEARCH_USAGE,
                    });
                }
            }
            self.fall_back(scope, limits).await
        })
    }

    /// Without a page: a query the engine may not receive is refused, and
    /// one whose page could not be opened falls back.
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        _context: &'a [WorkContextBody],
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        if self.mine(scope) && self.page(&scope.query).is_none() {
            return Box::pin(async {
                Err(WorkPublicSearchError::NotDispatched(WorkError::Invalid))
            });
        }
        self.search_page(scope, None, limits)
    }
}

fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn clip(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_owned()
}

#[cfg(test)]
mod tests;
