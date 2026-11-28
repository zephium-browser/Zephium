use super::*;
use std::sync::atomic::AtomicUsize;

const NATIVE: &str = "anthropic/claude-haiku-4-5-20251001";

/// The provider search a browser search falls back to: counts what it is
/// asked and answers like a model's own search.
#[derive(Default)]
struct Native {
    asked: AtomicUsize,
    unsent: bool,
}

impl WorkPublicSearchProvider for Native {
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        _: &'a [WorkContextBody],
        _: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            self.asked.fetch_add(1, Ordering::SeqCst);
            assert_eq!(scope.provider, WorkSearchProvider::Native);
            assert_eq!(scope.model, NATIVE);
            if self.unsent {
                return Err(WorkPublicSearchError::NotDispatched(WorkError::Capacity));
            }
            Ok(WorkPublicSearchResult {
                evidence: WorkProviderSearchEvidenceV1 {
                    version: 1,
                    provider: WorkSearchProvider::Native,
                    model: NATIVE.into(),
                    response_model: NATIVE.into(),
                    response_id: "zr_1".into(),
                    search_call_id: "zs_1".into(),
                    answer: "Tokio is a runtime.".into(),
                    citations: vec![WorkProviderSearchCitation {
                        url: "https://tokio.rs/".into(),
                        title: "Tokio".into(),
                        start_index: 0,
                        end_index: 5,
                        snippet: None,
                    }],
                    actual_input_tokens: 9_000,
                    actual_output_tokens: 200,
                },
                usage: WorkUsage {
                    model_tokens: 9_200,
                    cost_micro_usd: 14_500,
                    operations: 1,
                    accounting: WorkUsageAccounting::Exact,
                },
            })
        })
    }
}

/// A shared fallback, so a test can count what the backend asked of it.
struct Shared(std::sync::Arc<Native>);
impl WorkPublicSearchProvider for Shared {
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        context: &'a [WorkContextBody],
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        self.0.search(scope, context, limits)
    }
}

fn backend(engine: SearchEngine, native: &std::sync::Arc<Native>) -> BrowserSearch {
    BrowserSearch::try_new(engine, "", "")
        .unwrap()
        .with_fallback(
            Box::new(Shared(native.clone())),
            WorkSearchFallbackV1 {
                provider: WorkSearchProvider::Native,
                model: NATIVE.into(),
            },
        )
        .unwrap()
}

fn scope(engine: SearchEngine, query: &str) -> WorkPublicSearchScope {
    WorkPublicSearchScope {
        provider: WorkSearchProvider::Browser,
        model: engine.id().into(),
        query: query.into(),
    }
}

const LIMITS: WorkExecutionLimits = WorkExecutionLimits {
    model_tokens: 200_000,
    cost_micro_usd: 100_000,
    operations: 4,
    timeout_seconds: 60,
    max_workers: 1,
};

fn results() -> WorkSearchPage {
    WorkSearchPage::Results(vec![
        WorkSearchHit {
            title: "Tokio - An asynchronous Rust runtime".into(),
            url: "https://tokio.rs/".into(),
            snippet: Some("Tokio is an event-driven,\nnon-blocking I/O platform.".into()),
        },
        WorkSearchHit {
            title: "async-std".into(),
            url: "https://async.rs/".into(),
            snippet: None,
        },
    ])
}

#[tokio::test]
async fn a_results_page_is_the_search_at_no_cost() {
    let native = std::sync::Arc::new(Native::default());
    let search = backend(SearchEngine::DuckDuckGo, &native);
    let scope = scope(SearchEngine::DuckDuckGo, "rust async runtime");
    assert_eq!(
        search.grant(),
        Some((WorkSearchProvider::Browser, "duckduckgo".into()))
    );
    assert_eq!(
        search.fallback().map(|fallback| fallback.model),
        Some(NATIVE.to_owned())
    );
    let reserved = search.minimum_reservation(&scope, &[]).unwrap();
    assert_eq!((reserved.model_tokens, reserved.cost_micro_usd), (0, 0));
    let page = search.results_page(&scope).unwrap();
    assert_eq!(page.engine, SearchEngine::DuckDuckGo);
    assert_eq!(
        page.url.as_str(),
        "https://duckduckgo.com/?q=rust+async+runtime"
    );
    let found = search
        .search_page(&scope, Some(results()), LIMITS)
        .await
        .unwrap();
    assert_eq!(found.usage.model_tokens, 0);
    assert_eq!(found.usage.cost_micro_usd, 0);
    assert_eq!(found.usage.accounting, WorkUsageAccounting::Exact);
    let evidence = found.evidence;
    assert!(evidence.validate().is_ok());
    assert_eq!(evidence.provider, WorkSearchProvider::Browser);
    assert_eq!(evidence.model, "duckduckgo");
    assert!(evidence.answer.is_empty());
    assert_eq!(evidence.citations.len(), 2);
    assert_eq!(
        evidence.citations[0].snippet.as_deref(),
        Some("Tokio is an event-driven, non-blocking I/O platform.")
    );
    assert_eq!(native.asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_wall_falls_back_until_the_runs_fallbacks_are_spent() {
    let native = std::sync::Arc::new(Native::default());
    let search = backend(SearchEngine::Google, &native);
    let scope = scope(SearchEngine::Google, "rust async runtime");
    for (round, page) in [
        Some(WorkSearchPage::Wall(WorkSearchWall::Challenge)),
        Some(WorkSearchPage::Wall(WorkSearchWall::Consent)),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let found = search.search_page(&scope, page, LIMITS).await.unwrap();
        assert_eq!(found.evidence.provider, WorkSearchProvider::Native);
        assert!(found.usage.cost_micro_usd > 0);
        assert_eq!(native.asked.load(Ordering::SeqCst), round + 1);
    }
    // Past the cap the search says it is unavailable and sends nothing.
    let spent = search
        .search_page(
            &scope,
            Some(WorkSearchPage::Wall(WorkSearchWall::NoResults)),
            LIMITS,
        )
        .await;
    assert!(matches!(
        spent,
        Err(WorkPublicSearchError::NotDispatched(WorkError::Unavailable))
    ));
    assert_eq!(native.asked.load(Ordering::SeqCst), 3);
    // Results still come from the page.
    let found = search
        .search_page(&scope, Some(results()), LIMITS)
        .await
        .unwrap();
    assert_eq!(found.evidence.provider, WorkSearchProvider::Browser);
}

#[tokio::test]
async fn a_fallback_that_was_never_sent_keeps_the_runs_fallbacks() {
    let native = std::sync::Arc::new(Native {
        unsent: true,
        ..Native::default()
    });
    let search = backend(SearchEngine::Bing, &native);
    let scope = scope(SearchEngine::Bing, "rust async runtime");
    for _ in 0..5 {
        let refused = search.search_page(&scope, None, LIMITS).await;
        assert!(matches!(
            refused,
            Err(WorkPublicSearchError::NotDispatched(WorkError::Capacity))
        ));
    }
    assert_eq!(native.asked.load(Ordering::SeqCst), 5);
    assert_eq!(search.fallbacks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn without_a_fallback_a_wall_is_reported_unavailable() {
    let search = BrowserSearch::try_new(SearchEngine::Brave, "", "").unwrap();
    let scope = scope(SearchEngine::Brave, "rust async runtime");
    assert_eq!(search.fallback(), None);
    assert!(matches!(
        search
            .search_page(
                &scope,
                Some(WorkSearchPage::Wall(WorkSearchWall::Challenge)),
                LIMITS
            )
            .await,
        Err(WorkPublicSearchError::NotDispatched(WorkError::Unavailable))
    ));
}

#[tokio::test]
async fn a_query_the_engine_may_not_receive_is_never_sent_anywhere() {
    let native = std::sync::Arc::new(Native::default());
    let search = backend(SearchEngine::DuckDuckGo, &native);
    let scope = scope(SearchEngine::DuckDuckGo, "/Users/me/private/notes.txt");
    assert!(search.results_page(&scope).is_none());
    assert!(matches!(
        search.search(&scope, &[], LIMITS).await,
        Err(WorkPublicSearchError::NotDispatched(WorkError::Invalid))
    ));
    assert_eq!(native.asked.load(Ordering::SeqCst), 0);
    // Another engine's scope is not this backend's to answer.
    let other = super::tests::scope(SearchEngine::Google, "rust");
    assert!(search.results_page(&other).is_none());
}

#[test]
fn kagi_session_token_goes_only_to_kagi() {
    let search = BrowserSearch::try_new(SearchEngine::Kagi, "", "secret-token").unwrap();
    let url = search
        .results_page(&scope(SearchEngine::Kagi, "rust"))
        .unwrap()
        .url;
    assert_eq!(url.host_str(), Some("kagi.com"));
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "token")
            .map(|(_, value)| value.into_owned()),
        Some("secret-token".to_owned())
    );
    let other = BrowserSearch::try_new(SearchEngine::Google, "", "secret-token").unwrap();
    let url = other
        .results_page(&scope(SearchEngine::Google, "rust"))
        .unwrap()
        .url;
    assert!(url.query_pairs().all(|(key, _)| key != "token"));
}

#[test]
fn freshness_becomes_each_engines_own_parameter() {
    let url = |engine, query: &str| {
        BrowserSearch::try_new(engine, "https://search.example.net/?q={searchTerms}", "")
            .unwrap()
            .results_page(&scope(engine, query))
            .unwrap()
            .url
            .to_string()
    };
    assert_eq!(
        url(SearchEngine::Google, "rust release (past week)"),
        "https://www.google.com/search?q=rust+release&tbs=qdr%3Aw"
    );
    assert_eq!(
        url(SearchEngine::DuckDuckGo, "rust release (past day)"),
        "https://duckduckgo.com/?q=rust+release&df=d"
    );
    assert_eq!(
        url(SearchEngine::Bing, "rust release (past month)"),
        "https://www.bing.com/search?q=rust+release&filters=ex1%3A%22ez3%22"
    );
    assert_eq!(
        url(SearchEngine::Bing, "rust release (past year)"),
        "https://www.bing.com/search?q=rust+release"
    );
    assert_eq!(
        url(SearchEngine::Brave, "rust release (past year)"),
        "https://search.brave.com/search?q=rust+release&tf=py"
    );
    assert_eq!(
        url(SearchEngine::Custom, "rust release (past week)"),
        "https://search.example.net/?q=rust+release"
    );
    assert!(BrowserSearch::try_new(SearchEngine::Custom, "", "").is_err());
}
