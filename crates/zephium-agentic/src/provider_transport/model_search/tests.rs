use super::*;

type Script = Box<
    dyn Fn() -> (
            Vec<WorkModelEvent>,
            Result<WorkModelOutcome, WorkModelError>,
        ) + Send
        + Sync,
>;

struct Scripted {
    script: Script,
    requests: Mutex<Vec<WorkModelRequest>>,
}

impl WorkModelClient for Scripted {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            let (streamed, result) = (self.script)();
            for event in streamed {
                events(event);
            }
            result
        })
    }
}

fn entry(native_search: bool, price: Option<WorkModelPrice>) -> WorkModelEntry {
    WorkModelEntry {
        id: "anthropic/claude-haiku-4-5-20251001".into(),
        model: WorkModelRef {
            provider: WorkModelProvider::Anthropic,
            wire: WorkModelWire::AnthropicMessages,
            model: "claude-haiku-4-5-20251001".into(),
        },
        display_name: "Claude Haiku 4.5".into(),
        roles: vec![WorkModelRole::Light],
        recommended: true,
        context_window: 200_000,
        max_output: 64_000,
        supports: WorkModelSupports {
            tools: true,
            vision: true,
            prompt_cache: true,
            reasoning: true,
            native_search,
        },
        price,
    }
}

fn haiku_price() -> WorkModelPrice {
    WorkModelPrice {
        input: 1_000_000,
        cached_input: 100_000,
        output: 5_000_000,
    }
}

fn outcome(cost: Option<u64>) -> WorkModelOutcome {
    WorkModelOutcome {
        stop: WorkModelStop::EndTurn,
        usage: WorkModelUsage {
            input_tokens: 20_000,
            output_tokens: 300,
            cost_micros: cost,
            ..Default::default()
        },
        assistant: Vec::new(),
    }
}

fn search(script: Script, price: Option<WorkModelPrice>) -> (ModelNativeSearch, Arc<Scripted>) {
    let client = Arc::new(Scripted {
        script,
        requests: Mutex::new(Vec::new()),
    });
    let search = ModelNativeSearch::try_new(entry(true, price), client.clone()).unwrap();
    (search, client)
}

fn scope(query: &str) -> WorkPublicSearchScope {
    WorkPublicSearchScope {
        provider: WorkSearchProvider::Native,
        model: "anthropic/claude-haiku-4-5-20251001".into(),
        query: query.into(),
    }
}

fn limits() -> WorkExecutionLimits {
    WorkExecutionLimits {
        model_tokens: 400_000,
        cost_micro_usd: 1_000_000,
        operations: 4,
        timeout_seconds: 60,
        max_workers: 1,
    }
}

fn hit(url: &str, title: &str, snippet: &str) -> WorkModelSearchHit {
    WorkModelSearchHit {
        url: url.into(),
        title: title.into(),
        snippet: snippet.into(),
    }
}

#[test]
fn only_a_model_that_can_search_is_accepted() {
    let client: Arc<dyn WorkModelClient> = Arc::new(Scripted {
        script: Box::new(|| (Vec::new(), Ok(outcome(None)))),
        requests: Mutex::new(Vec::new()),
    });
    assert!(ModelNativeSearch::try_new(entry(false, None), client).is_err());
}

#[tokio::test]
async fn an_answer_with_sources_becomes_native_evidence_bound_to_the_model() {
    let (search, client) = search(
        Box::new(|| {
            (
                vec![
                    WorkModelEvent::Text("Lisbon averages 2,800 hours of sun a year, ".into()),
                    WorkModelEvent::Text("more than most European capitals.".into()),
                    WorkModelEvent::Search {
                        query: "lisbon sunshine".into(),
                        hits: vec![
                            hit(
                                "https://climate.example/lisbon?utm_source=chatgpt.com",
                                "Lisbon climate",
                                "2,800 hours of sun a year",
                            ),
                            hit("https://other.example/", "", "unrelated passage"),
                            hit(
                                "https://climate.example/lisbon?utm_source=chatgpt.com",
                                "dupe",
                                "",
                            ),
                            hit("file:///etc/passwd", "local", ""),
                        ],
                    },
                ],
                Ok(outcome(Some(21_500))),
            )
        }),
        Some(haiku_price()),
    );
    assert_eq!(
        search.grant(),
        Some((
            WorkSearchProvider::Native,
            "anthropic/claude-haiku-4-5-20251001".into()
        ))
    );
    let result = search
        .search(&scope("How sunny is Lisbon?"), &[], limits())
        .await
        .unwrap();
    let evidence = result.evidence;
    assert!(evidence.validate().is_ok());
    assert!(evidence.response_id.starts_with("zr_"));
    assert!(evidence.search_call_id.starts_with("zs_"));
    assert_eq!(evidence.citations.len(), 2);
    assert_eq!(evidence.citations[0].url, "https://climate.example/lisbon");
    let quoted: String = evidence
        .answer
        .chars()
        .skip(evidence.citations[0].start_index as usize)
        .take((evidence.citations[0].end_index - evidence.citations[0].start_index) as usize)
        .collect();
    assert_eq!(quoted, "2,800 hours of sun a year");
    // A source the answer never quotes supports the whole answer, under its host's name.
    assert_eq!(evidence.citations[1].title, "other.example");
    assert_eq!(evidence.citations[1].start_index, 0);
    assert_eq!(result.usage.cost_micro_usd, 21_500);
    assert_eq!(result.usage.accounting, WorkUsageAccounting::Exact);

    let request = client.requests.lock().unwrap().pop().unwrap();
    assert!(request.native_search);
    assert!(request.tools.is_empty());
    assert_eq!(request.max_output_tokens, OUTPUT_TOKENS);
}

#[tokio::test]
async fn refusals_before_and_after_dispatch_stay_distinct() {
    let refused = |error: WorkModelError| -> Script { Box::new(move || (Vec::new(), Err(error))) };
    let (missing, _) = search(refused(WorkModelError::MissingKey), None);
    assert_eq!(
        missing.search(&scope("query"), &[], limits()).await.err(),
        Some(WorkPublicSearchError::NotDispatched(WorkError::Unavailable))
    );
    let (lost, _) = search(refused(WorkModelError::Network), None);
    assert!(matches!(
        lost.search(&scope("query"), &[], limits()).await.err(),
        Some(WorkPublicSearchError::Rejected(_))
    ));
    // No sources means no evidence, and the spend still counts.
    let (bare, _) = search(
        Box::new(|| {
            (
                vec![WorkModelEvent::Text("An answer.".into())],
                Ok(outcome(None)),
            )
        }),
        None,
    );
    match bare.search(&scope("query"), &[], limits()).await.err() {
        Some(WorkPublicSearchError::Rejected(usage)) => {
            assert!(usage.cost_micro_usd > 0);
            assert_eq!(
                usage.accounting,
                WorkUsageAccounting::ConservativeReservation
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn nothing_is_sent_past_the_budget_for_a_secret_or_for_another_model() {
    let (search, client) = search(
        Box::new(|| (Vec::new(), Ok(outcome(None)))),
        Some(haiku_price()),
    );
    let tight = WorkExecutionLimits {
        cost_micro_usd: 1_000,
        ..limits()
    };
    assert_eq!(
        search.search(&scope("query"), &[], tight).await.err(),
        Some(WorkPublicSearchError::NotDispatched(WorkError::Capacity))
    );
    let mut other = scope("query");
    other.model = "openai/gpt-6-luna".into();
    assert_eq!(
        search.search(&other, &[], limits()).await.err(),
        Some(WorkPublicSearchError::NotDispatched(WorkError::Invalid))
    );
    assert!(search
        .search(
            &scope("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"),
            &[],
            limits()
        )
        .await
        .is_err());
    assert!(client.requests.lock().unwrap().is_empty());
    let reserved = search.minimum_reservation(&scope("query"), &[]).unwrap();
    assert!(reserved.model_tokens > SEARCH_CONTEXT_TOKENS);
    assert!(reserved.cost_micro_usd > 0);
}

#[test]
fn local_native_search_never_invents_a_flat_fee_or_advertised_charge() {
    let mut model = entry(
        true,
        Some(WorkModelPrice {
            input: 0,
            cached_input: 0,
            output: 0,
        }),
    );
    model.model.provider = WorkModelProvider::Compatible;
    let provider = ModelNativeSearch::try_new(
        model,
        Arc::new(Scripted {
            script: Box::new(|| panic!("no provider call")),
            requests: Mutex::new(vec![]),
        }),
    )
    .unwrap();
    let reservation = provider.reservation(100).unwrap();
    assert_eq!(reservation.cost_micro_usd, 0);
    assert!(reservation.model_tokens > 0);
    let usage = provider.settle(
        WorkModelUsage {
            input_tokens: 100,
            output_tokens: 10,
            cost_micros: Some(9000),
            ..Default::default()
        },
        reservation,
    );
    assert_eq!(usage.cost_micro_usd, 0);
    assert_eq!(usage.model_tokens, 110);
    assert_eq!(usage.operations, 1);
}
