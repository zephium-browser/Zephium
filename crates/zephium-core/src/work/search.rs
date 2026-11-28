gine", &self.engine)
            .finish_non_exhaustive()
    }
}

/// What a results page showed, read by Rust from the page itself.
#[derive(Clone, PartialEq, Eq)]
pub enum WorkSearchPage {
    /// The engine's results in its order, without ads or its own links.
    Results(Vec<WorkSearchHit>),
    /// The page stood in the way of its results.
    Wall(WorkSearchWall),
}
impl std::fmt::Debug for WorkSearchPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Results(hits) => write!(f, "Results({})", hits.len()),
            Self::Wall(wall) => write!(f, "Wall({wall:?})"),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct WorkSearchHit {
    pub title: String,
    /// The destination itself, never the engine's tracking redirect.
    pub url: String,
    pub snippet: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkSearchWall {
    /// A CAPTCHA, a "sorry" or an unusual-traffic page.
    Challenge,
    /// A consent page that must be answered before any results.
    Consent,
    /// The engine found nothing for the query.
    NoResults,
    /// The page loaded but showed no result this reader recognises.
    Unreadable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkPublicSearchError {
    NotDispatched(WorkError),
    Rejected(WorkUsage),
    OutcomeUnknown,
}
pub type WorkPublicSearchFuture<'a> = Pin<
    Box<dyn Future<Output = Result<WorkPublicSearchResult, WorkPublicSearchError>> + Send + 'a>,
>;
/// Advisory source priority; IDs are one-based indices into the unchanged citations.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkPublicSearchRanking {
    pub preferred: Vec<u16>,
    pub usage: WorkUsage,
}
impl WorkPublicSearchRanking {
    pub fn validate(&self, evidence: &WorkProviderSearchEvidenceV1) -> Result<(), WorkError> {
        let mut seen = std::collections::BTreeSet::new();
        if self.preferred.len() > evidence.citations.len()
            || self.preferred.iter().any(|id| {
                *id == 0 || usize::from(*id) > evidence.citations.len() || !seen.insert(*id)
            })
            || (!self.preferred.is_empty() && self.usage.operations == 0)
        {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
}
/// A reuse decision and what deciding it cost.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkPublicSearchReuse {
    pub answers: bool,
    pub usage: WorkUsage,
}
pub type WorkPublicSearchReuseFuture<'a> =
    Pin<Box<dyn Future<Output = Result<WorkPublicSearchReuse, WorkPublicSearchError>> + Send + 'a>>;
pub type WorkPublicSearchRankingFuture<'a> = Pin<
    Box<dyn Future<Output = Result<WorkPublicSearchRanking, WorkPublicSearchError>> + Send + 'a>,
>;

pub trait WorkPublicSearchProvider: Send + Sync {
    /// The provider and model a run's grant records for this backend's
    /// searches; `None` keeps what the grant already names.
    fn grant(&self) -> Option<(WorkSearchProvider, String)> {
        None
    }

    /// The other search this backend may answer with, whose evidence the
    /// grant then also admits.
    fn fallback(&self) -> Option<WorkSearchFallbackV1> {
        None
    }

    /// The results page a browser search opens for this scope; None for a
    /// search a provider runs itself.
    fn results_page(&self, _scope: &WorkPublicSearchScope) -> Option<WorkResultsPage> {
        None
    }

    /// A browser search over what its results page showed, `None` when the
    /// page could not be opened. Others search as usual.
    fn search_page<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        _page: Option<WorkSearchPage>,
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        self.search(scope, &[], limits)
    }

    /// Pure scheduling hint, with no dispatch or disclosure. Unknown providers
    /// may omit it; every search still enforces its supplied limits independently.
    fn minimum_reservation(
        &self,
        _scope: &WorkPublicSearchScope,
        _context: &[super::context::WorkContextBody],
    ) -> Option<WorkUsage> {
        None
    }

    /// Optional decisions over already admitted public search evidence. The caller
    /// supplies only the original step's remaining budget and absolute deadline.
    fn rerank<'a>(
        &'a self,
        _scope: &'a WorkPublicSearchScope,
        _evidence: &'a WorkProviderSearchEvidenceV1,
        _limits: WorkExecutionLimits,
        _deadline: std::time::Instant,
    ) -> WorkPublicSearchRankingFuture<'a> {
        Box::pin(async { Ok(WorkPublicSearchRanking::default()) })
    }

    /// Optional: whether an earlier search, made for `earlier`, already
    /// answers this scope's query, so its evidence can stand for a new one.
    /// Deciding may spend a little of `limits`; without it nothing is reused.
    fn reuse<'a>(
        &'a self,
        _scope: &'a WorkPublicSearchScope,
        _earlier: &'a str,
        _evidence: &'a WorkProviderSearchEvidenceV1,
        _limits: WorkExecutionLimits,
        _deadline: std::time::Instant,
    ) -> WorkPublicSearchReuseFuture<'a> {
        Box::pin(async { Ok(WorkPublicSearchReuse::default()) })
    }

    /// Optional: whether what a part's searches found already answers its
    /// goal (`scope.query`), so it finishes instead of searching again.
    /// `found` is those searches' own answers; `answers` true means enough.
    /// Without it, the part decides for itself.
    fn enough<'a>(
        &'a self,
        _scope: &'a WorkPublicSearchScope,
        _found: &'a str,
        _limits: WorkExecutionLimits,
        _deadline: std::time::Instant,
    ) -> WorkPublicSearchReuseFuture<'a> {
        Box::pin(async { Ok(WorkPublicSearchReuse::default()) })
    }

    /// `context` carries only Rust-admitted public bodies for this attempt.
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        context: &'a [super::context::WorkContextBody],
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_query_limit_counts_unicode_scalars_with_bounded_utf8() {
        for character in ['a', '—', '界', '😀'] {
            let query = character.to_string().repeat(PUBLIC_SEARCH_MAX_QUERY_CHARS);
            assert!(query.len() <= PUBLIC_SEARCH_MAX_QUERY_BYTES);
            let mut scope = WorkPublicSearchScope {
                provider: WorkSearchProvider::OpenAi,
                model: PUBLIC_SEARCH_MODEL.into(),
                query,
            };
            assert!(scope.validate().is_ok());
            scope.query.push(character);
            assert_eq!(scope.validate(), Err(WorkError::Invalid));
        }
        for query in ["", " ", "public\nquery", "public\0query"] {
            assert_eq!(validate_public_search_query(query), Err(WorkError::Invalid));
        }
    }
    #[test]
    fn search_scope_pins_model_and_does_not_expose_query_in_debug() {
        let mut scope = WorkPublicSearchScope {
            provider: WorkSearchProvider::OpenAi,
            model: PUBLIC_SEARCH_MODEL.into(),
            query: "public query".into(),
        };
        assert!(scope.validate().is_ok());
        assert!(!format!("{scope:?}").contains("public query"));
        for model in ["gpt-4.1-mini", "gpt-5.6-luna", "gpt-6-luna"] {
            scope.model = model.into();
            assert!(scope.validate().is_ok());
        }
        scope.model = "unadmitted-model".into();
        assert!(scope.validate().is_err());
    }
    #[test]
    fn provider_evidence_preserves_character_offsets_and_refuses_bad_sources() {
        let mut evidence = WorkProviderSearchEvidenceV1 {
            version: 1,
            provider: WorkSearchProvider::OpenAi,
            model: PUBLIC_SEARCH_MODEL.into(),
            response_model: PUBLIC_SEARCH_MODEL.into(),
            response_id: "resp_test".into(),
            search_call_id: "ws_test".into(),
            answer: "é source".into(),
            citations: vec![WorkProviderSearchCitation {
                url: "https://example.com".into(),
                title: "Source".into(),
                start_index: 2,
                end_index: 8,
                snippet: None,
            }],
            actual_input_tokens: 100,
            actual_output_tokens: 10,
        };
        assert!(evidence.validate().is_ok());
        // Evidence stored under the earlier model still validates; a
        // response from another model never does.
        evidence.model = "gpt-5.6-luna".into();
        evidence.response_model = "gpt-5.6-luna".into();
        assert!(evidence.validate().is_ok());
        evidence.response_model = PUBLIC_SEARCH_MODEL.into();
        assert!(evidence.validate().is_err());
        evidence.model = PUBLIC_SEARCH_MODEL.into();
        assert!(evidence.validate().is_ok());
        assert_eq!(evidence.citation_excerpt(0).unwrap(), "é source");
        assert_eq!(evidence.citation_excerpt(1), Err(WorkError::NotFound));
        for control in ['\r', '\0', '\u{7f}'] {
            evidence.answer = format!("é source{control}");
            assert!(evidence.validate().is_err());
        }
        evidence.answer = "é source\n\t".into();
        assert!(evidence.validate().is_ok());
        evidence.answer = "é source".into();
        evidence.citations[0].end_index = 9;
        assert!(evidence.validate().is_err());
        evidence.citations[0].end_index = 8;
        evidence.citations[0].url = "file:///private".into();
        assert!(evidence.validate().is_err());
        evidence.answer = "é".repeat(2000) + "source" + &"界".repeat(2000);
        evidence.citations[0].start_index = 2000;
        evidence.citations[0].end_index = 2006;
        let excerpt = evidence.citation_excerpt(0).unwrap();
        assert!(excerpt.contains("source"));
        assert_eq!(excerpt.chars().count(), 768);
        assert!(excerpt.len() < evidence.answer.len());
    }
    #[test]
    fn browser_search_carries_ranked_results_and_no_answer() {
        let result = |url: &str, snippet: Option<&str>| WorkProviderSearchCitation {
            url: url.into(),
            title: "Result".into(),
            start_index: 0,
            end_index: 0,
            snippet: snippet.map(str::to_owned),
        };
        let mut evidence = WorkProviderSearchEvidenceV1 {
            version: 1,
            provider: WorkSearchProvider::Browser,
            model: "duckduckgo".into(),
            response_model: "duckduckgo".into(),
            response_id: "zb_01J".into(),
            search_call_id: "zq_01J".into(),
            answer: String::new(),
            citations: vec![
                result("https://example.com/a", Some("The engine's own text")),
                result("https://example.org/b", None),
            ],
            actual_input_tokens: 0,
            actual_output_tokens: 0,
        };
        assert!(evidence.validate().is_ok());
        assert_eq!(
            evidence.citation_excerpt(0).unwrap(),
            "The engine's own text"
        );
        assert_eq!(evidence.citation_excerpt(1).unwrap(), "Result");
        let valid = evidence.clone();
        let refused: [fn(&mut WorkProviderSearchEvidenceV1); 9] = [
            |e| e.answer = "An answer the engine never gave".into(),
            |e| e.citations.clear(),
            |e| e.actual_input_tokens = 1,
            |e| e.model = "yahoo".into(),
            |e| e.response_id = "zr_01J".into(),
            |e| e.citations[1].url = e.citations[0].url.clone(),
            |e| e.citations[0].end_index = 4,
            |e| e.citations[0].snippet = Some("line\rbreak".into()),
            |e| e.citations[1].url = "javascript:alert(1)".into(),
        ];
        for refuse in refused {
            evidence = valid.clone();
            refuse(&mut evidence);
            assert!(evidence.validate().is_err());
        }
        // A provider answer never carries a browser result's snippet.
        let mut native = valid;
        native.provider = WorkSearchProvider::Native;
        native.model = "anthropic/claude-haiku-4-5-20251001".into();
        native.response_model = native.model.clone();
        native.response_id = "zr_01J".into();
        native.search_call_id = "zs_01J".into();
        native.answer = "A source".into();
        native.actual_input_tokens = 10;
        native.citations.truncate(1);
        native.citations[0].end_index = 8;
        assert!(native.validate().is_err());
        native.citations[0].snippet = None;
        assert!(native.validate().is_ok());
        for engine in BROWSER_SEARCH_ENGINES {
            assert!(supported_search_model(WorkSearchProvider::Browser, engine));
        }
        let fallback = WorkSearchFallbackV1 {
            provider: WorkSearchProvider::Browser,
            model: "google".into(),
        };
        assert!(fallback.validate().is_err());
    }

    #[test]
    fn native_search_binds_the_chosen_model_and_rust_minted_ids() {
        let scope = WorkPublicSearchScope {
            provider: WorkSearchProvider::Native,
            model: "anthropic/claude-haiku-4-5-20251001".into(),
            query: "public query".into(),
        };
        assert!(scope.validate().is_ok());
        for model in ["", "has space", "quote\"d"] {
            assert!(!supported_search_model(WorkSearchProvider::Native, model));
        }
        assert!(!supported_search_model(
            WorkSearchProvider::OpenAi,
            "anthropic/claude"
        ));
        let mut evidence = WorkProviderSearchEvidenceV1 {
            version: 1,
            provider: WorkSearchProvider::Native,
            model: scope.model.clone(),
            response_model: scope.model.clone(),
            response_id: "zr_01J".into(),
            search_call_id: "zs_01J".into(),
            answer: "A source".into(),
            citations: vec![WorkProviderSearchCitation {
                url: "https://example.com".into(),
                title: "Source".into(),
                start_index: 0,
                end_index: 8,
                snippet: None,
            }],
            actual_input_tokens: 100,
            actual_output_tokens: 10,
        };
        assert!(evidence.validate().is_ok());
        // Native evidence never passes as an OpenAI receipt, nor the reverse.
        evidence.response_id = "resp_01J".into();
        assert!(evidence.validate().is_err());
        evidence.response_id = "zr_01J".into();
        evidence.response_model = "zephium/other".into();
        assert!(evidence.validate().is_err());
        evidence.response_model = scope.model.clone();
        evidence.provider = WorkSearchProvider::OpenAi;
        assert!(evidence.validate().is_err());
    }
}
