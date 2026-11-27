use super::*;
use crate::work_agent::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use zephium_core::work::{agent::*, search::*, synthesis::*};

struct Turns(AtomicUsize);
impl WorkAgentTurnProvider for Turns {
    fn turn<'a>(
        &'a self,
        _: &'a WorkAgentTurnDisclosure,
        _: WorkSynthesisTrace,
    ) -> WorkAgentTurnFuture<'a> {
        Box::pin(async move {
            let turn = self.0.fetch_add(1, Ordering::SeqCst);
            assert!(turn < 2);
            Ok(WorkAgentTurnResult {
                output: WorkAgentTurnOutput {
                    say: None,
                    artifacts: vec![],
                    fetch: if turn == 0 {
                        ['a', 'b', 'c', 'd']
                            .into_iter()
                            .map(|key| WorkAgentFetch::Read {
                                url: format!("https://example.test/{key}"),
                                collection: None,
                            })
                            .collect()
                    } else {
                        vec![]
                    },
                    ask: None,
                    finish: turn > 0,
                    followups: vec![],
                    malformed: 0,
                },
                usage: WorkUsage::default(),
            })
        })
    }
}
struct NoSearch;
impl WorkPublicSearchProvider for NoSearch {
    fn search<'a>(
        &'a self,
        _: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        _: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        panic!("exact source URLs need no search");
    }
}

#[tokio::test]
async fn work_parallel_reads_partition_retries_and_drain_unknown_outcomes() {
    for (unknown, slow, slow_fails, turn_limit) in [
        (false, false, false, false),
        (true, false, false, false),
        (false, true, false, false),
        (false, true, true, false),
        (false, false, false, true),
        (false, false, true, true),
    ] {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let create = handle.work_document(WorkIntent::Create {
            objective: "Read https://example.test/a https://example.test/b https://example.test/c https://example.test/d and compare".into(),
        }).unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let provider = Turns(AtomicUsize::new(0));
        let service = WorkAgentService::new(handle);
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::<WorkAgentBrowseRequest>::new()));
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            drive(
                &mut shell,
                &queue,
                service.run(
                    profile,
                    WorkCommandV1 {
                        version: 1,
                        work,
                        expected_revision: WorkRevision::INITIAL,
                        command: WorkCommandId::generate(),
                        intent: WorkRuntimeIntent::BeginAgent {
                            grant: WorkAgentGrantV1 {
                                no_usage_charge: false,
                                draft: None,
                                lead: None,
                                provider: WorkSearchProvider::OpenAi,
                                model: PUBLIC_SEARCH_MODEL.into(),
                                max_turns: 8,
                                max_steps: 24,
                                browse_hops: 1,
                                folders: vec![],
                                accounts: Vec::new(),
                                private: false,
                                skill: None,
                                search_fallback: None,
                            },
                            limits: WorkExecutionLimits {
                                model_tokens: 100_000,
                                cost_micro_usd: 100_000,
                                operations: 32,
                                timeout_seconds: 30,
                                max_workers: 4,
                            },
                        },
                    },
                    None,
                    WorkAgentProviders {
                        turn: &provider,
                        search: &NoSearch,
                    },
                    |probe, request| {
                        let ordinal = {
                            let mut requests = requests.lock().unwrap();
                            let ordinal = requests.len();
                            requests.push(request.clone());
                            ordinal
                        };
                        if ordinal == 3 {
                            assert_eq!(completed.load(Ordering::SeqCst), 3);
                        }
                        let barrier = barrier.clone();
                        let active = active.clone();
                        let peak = peak.clone();
                        let completed = completed.clone();
                        async move {
                            assert!(probe.admit_read_page(WorkStepId::generate()).await.is_err());
                            let _admission = probe.admit_read_page(request.id).await.unwrap();
                            peak.fetch_max(
                                active.fetch_add(1, Ordering::SeqCst) + 1,
                                Ordering::SeqCst,
                            );
                            if ordinal < 3 {
                                barrier.wait().await;
                            }
                            if ordinal > 0 {
                                tokio::time::sleep(Duration::from_millis(30)).await;
                            }
                            active.fetch_sub(1, Ordering::SeqCst);
                            completed.fetch_add(1, Ordering::SeqCst);
                            if ordinal == 0 && unknown {
                                return Err(WorkError::OutcomeUnknown);
                            }
                            let first = ordinal == 0;
                            let retry_failed = ordinal == 3 && slow_fails;
                            Ok(WorkBrowserOutcome {
                                status: if first || retry_failed {
                                    WorkStepStatus::Failed
                                } else {
                                    WorkStepStatus::Succeeded
                                },
                                usage: Some(if first {
                                    WorkUsage {
                                        model_tokens: 11,
                                        cost_micro_usd: 7,
                                        operations: 2,
                                        accounting: WorkUsageAccounting::ConservativeReservation,
                                    }
                                } else {
                                    WorkUsage {
                                        model_tokens: 3,
                                        cost_micro_usd: 2,
                                        operations: 1,
                                        accounting: WorkUsageAccounting::Exact,
                                    }
                                }),
                                intervention: None,
                                measurements: ((first || retry_failed) && turn_limit).then(|| {
                                    WorkStepMeasurementsV1 {
                                        outcome: Some(WorkPageOutcomeV1::TooManySteps),
                                        ..Default::default()
                                    }
                                }),
                                // The first page's human check was taken and continued.
                                helped: first && !slow && !turn_limit,
                                note: if first && turn_limit {
                                    None
                                } else if first {
                                    Some(
                                        if slow {
                                            read_note::CONSTRUCTION_TIMEOUT
                                        } else {
                                            read_note::HUMAN_CHECK
                                        }
                                        .into(),
                                    )
                                } else {
                                    retry_failed.then(|| read_note::SLOW_SITE.into())
                                },
                                artifacts: if first || retry_failed {
                                    vec![]
                                } else {
                                    vec![WorkArtifactDraft {
                                        output: request.output,
                                        title: "Page result".into(),
                                        evidence: vec![WorkEvidenceLink {
                                            extraction_id: WorkArtifactId::from(100),
                                            source_id: 1,
                                        }],
                                        data: WorkArtifactDataV1::Document {
                                            paragraphs: vec!["Page facts".into()],
                                            formatted: None,
                                        },
                                    }]
                                },
                                held_back: false,
                                committed: Vec::new(),
                                rerun: false,
                                search: None,
                                redirect: None,
                            })
                        }
                    },
                    |_| {},
                ),
            ),
        )
        .await
        .expect("three reads must overlap")
        .unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), 3);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(
            completed.load(Ordering::SeqCst),
            if unknown { 3 } else { 5 }
        );
        let requests = requests.lock().unwrap();
        let initial = &requests[..3];
        assert_eq!(
            initial
                .iter()
                .map(|request| request.limits.model_tokens)
                .sum::<u32>(),
            100_000
        );
        assert_eq!(
            initial
                .iter()
                .map(|request| request.limits.cost_micro_usd)
                .sum::<u32>(),
            100_000
        );
        assert_eq!(
            initial
                .iter()
                .map(|request| request.limits.operations)
                .sum::<u32>(),
            31
        );
        assert!(requests
            .iter()
            .all(|request| request.limits.max_workers == 1));
        let execution = &result.executions[0];
        assert!(execution
            .steps
            .iter()
            .all(|step| step.status != WorkStepStatus::Running));
        let reads: Vec<_> = execution
            .steps
            .iter()
            .filter(|step| matches!(step.kind, WorkStepKindV1::Read { .. }))
            .collect();
        assert_eq!(reads.len(), if unknown { 3 } else { 4 });
        if unknown {
            assert_eq!(requests.len(), 3);
            assert_eq!(execution.status, WorkExecutionStatus::Interrupted);
            assert_eq!(
                execution.attempts[0].status,
                WorkAttemptStatus::OutcomeUnknown
            );
            assert_eq!(execution.attempts[0].usage, None);
            assert_eq!(
                reads
                    .iter()
                    .filter(|step| step.status == WorkStepStatus::Succeeded)
                    .count(),
                2
            );
        } else {
            assert_eq!(execution.status, WorkExecutionStatus::NeedsReview);
            let retry = &requests[3];
            assert_eq!(retry.id, initial[0].id);
            assert!(initial.iter().all(|request| request.construction_attempt
                == zephium_agentic::WorkBrowserConstructionAttempt::Initial));
            assert_eq!(
                retry.construction_attempt,
                if slow {
                    zephium_agentic::WorkBrowserConstructionAttempt::SlowPageRetry
                } else {
                    zephium_agentic::WorkBrowserConstructionAttempt::Initial
                }
            );
            assert_eq!(
                retry.limits.model_tokens + 11,
                initial[0].limits.model_tokens
            );
            assert_eq!(
                retry.limits.cost_micro_usd + 7,
                initial[0].limits.cost_micro_usd
            );
            assert_eq!(retry.limits.operations + 2, initial[0].limits.operations);
            let retried = reads.iter().find(|step| step.id == retry.id).unwrap();
            if slow_fails {
                assert_eq!(retried.status, WorkStepStatus::Failed);
                assert_eq!(retried.note.as_deref(), Some(read_note::SLOW_SITE));
            }
            assert_eq!(
                retried.usage,
                Some(WorkUsage {
                    model_tokens: 14,
                    cost_micro_usd: 9,
                    operations: 3,
                    accounting: WorkUsageAccounting::ConservativeReservation
                })
            );
        }
    }
}

struct SearchTurns(AtomicUsize);
impl WorkAgentTurnProvider for SearchTurns {
    fn turn<'a>(
        &'a self,
        _: &'a WorkAgentTurnDisclosure,
        _: WorkSynthesisTrace,
    ) -> WorkAgentTurnFuture<'a> {
        Box::pin(async move {
            let turn = self.0.fetch_add(1, Ordering::SeqCst);
            assert!(turn < 4);
            Ok(WorkAgentTurnResult {
                output: WorkAgentTurnOutput {
                    say: None,
                    artifacts: vec![],
                    fetch: if turn == 0 {
                        (0..4)
                            .map(|index| WorkAgentFetch::Search {
                                query: format!("public source {index}"),
                            })
                            .collect()
                    } else {
                        vec![]
                    },
                    ask: None,
                    finish: turn > 0,
                    followups: vec![],
                    malformed: 0,
                },
                usage: WorkUsage::default(),
            })
        })
    }
}
struct BudgetedSearch {
    width: usize,
    requests: Mutex<Vec<WorkExecutionLimits>>,
    barrier: tokio::sync::Barrier,
    completed: AtomicUsize,
}
impl WorkPublicSearchProvider for BudgetedSearch {
    fn minimum_reservation(
        &self,
        _: &WorkPublicSearchScope,
        _: &[zephium_core::work::context::WorkContextBody],
    ) -> Option<WorkUsage> {
        (self.width == 1).then_some(WorkUsage {
            model_tokens: 60_000,
            cost_micro_usd: 1,
            operations: 1,
            accounting: WorkUsageAccounting::ConservativeReservation,
        })
    }
    fn search<'a>(
        &'a self,
        _: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            let ordinal = {
                let mut requests = self.requests.lock().unwrap();
                let ordinal = requests.len();
                requests.push(limits);
                ordinal
            };
            assert!(self.completed.load(Ordering::SeqCst) >= ordinal / self.width * self.width);
            self.barrier.wait().await;
            self.completed.fetch_add(1, Ordering::SeqCst);
            Err(WorkPublicSearchError::Rejected(WorkUsage {
                model_tokens: 10,
                cost_micro_usd: 20,
                operations: 1,
                accounting: WorkUsageAccounting::Exact,
            }))
        })
    }
}

#[tokio::test]
async fn work_parallel_searches_share_and_recalculate_the_remaining_grant() {
    for width in [2, 1] {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let create = handle
            .work_document(WorkIntent::Create {
                objective: "Compare four public sources".into(),
            })
            .unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let turns = SearchTurns(AtomicUsize::new(0));
        let search = BudgetedSearch {
            width,
            requests: Mutex::new(vec![]),
            barrier: tokio::sync::Barrier::new(width),
            completed: AtomicUsize::new(0),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            drive(
                &mut shell,
                &queue,
                WorkAgentService::new(handle).run(
                    profile,
                    WorkCommandV1 {
                        version: 1,
                        work,
                        expected_revision: WorkRevision::INITIAL,
                        command: WorkCommandId::generate(),
                        intent: WorkRuntimeIntent::BeginAgent {
                            grant: WorkAgentGrantV1 {
                                no_usage_charge: false,
                                draft: None,
                                lead: None,
                                provider: WorkSearchProvider::OpenAi,
                                model: PUBLIC_SEARCH_MODEL.into(),
                                max_turns: 8,
                                max_steps: 24,
                                browse_hops: 1,
                                folders: vec![],
                                accounts: Vec::new(),
                                private: false,
                                skill: None,
                                search_fallback: None,
                            },
                            limits: WorkExecutionLimits {
                                model_tokens: 100_000,
                                cost_micro_usd: 100_000,
                                operations: 32,
                                timeout_seconds: 30,
                                max_workers: 2,
                            },
                        },
                    },
                    None,
                    WorkAgentProviders {
                        turn: &turns,
                        search: &search,
                    },
                    |_, _| async { panic!("search fixture never reads a page") },
                    |_| {},
                ),
            ),
        )
        .await
        .expect("both searches in each batch must overlap")
        .unwrap();
        assert_eq!(search.completed.load(Ordering::SeqCst), 4);
        let requests = search.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for (index, batch) in requests.chunks(width).enumerate() {
            assert_eq!(
                batch.iter().map(|limits| limits.model_tokens).sum::<u32>(),
                100_000 - index as u32 * width as u32 * 10
            );
            assert_eq!(
                batch
                    .iter()
                    .map(|limits| limits.cost_micro_usd)
                    .sum::<u32>(),
                100_000 - index as u32 * width as u32 * 20
            );
            assert_eq!(
                batch.iter().map(|limits| limits.operations).sum::<u32>(),
                31 - index as u32 * width as u32
            );
            assert!(batch
                .iter()
                .all(|limits| limits.max_workers == 1 && limits.timeout_seconds == 30));
        }
        assert!(result.executions[0]
            .steps
            .iter()
            .all(|step| step.status != WorkStepStatus::Running));
    }
}

struct RankedSearch {
    mode: u8,
    turns: AtomicUsize,
    rankings: AtomicUsize,
}
impl WorkPublicSearchProvider for RankedSearch {
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        _: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            Ok(WorkPublicSearchResult {
                evidence: WorkProviderSearchEvidenceV1 {
                    version: 1,
                    provider: scope.provider,
                    model: scope.model.clone(),
                    response_model: PUBLIC_SEARCH_MODEL.into(),
                    response_id: "resp_ranking".into(),
                    search_call_id: "ws_ranking".into(),
                    answer: "One two three public sources.".into(),
                    citations: (1..=3)
                        .map(|id| WorkProviderSearchCitation {
                            url: format!("https://example.test/{id}"),
                            title: id.to_string(),
                            start_index: id * 2,
                            end_index: id * 2 + 1,
                            snippet: None,
                        })
                        .collect(),
                    actual_input_tokens: 100,
                    actual_output_tokens: 10,
                },
                usage: WorkUsage {
                    model_tokens: 110,
                    cost_micro_usd: 7,
                    operations: 1,
                    accounting: WorkUsageAccounting::Exact,
                },
            })
        })
    }
    fn rerank<'a>(
        &'a self,
        _: &'a WorkPublicSearchScope,
        evidence: &'a WorkProviderSearchEvidenceV1,
        limits: WorkExecutionLimits,
        deadline: std::time::Instant,
    ) -> WorkPublicSearchRankingFuture<'a> {
        Box::pin(async move {
            self.rankings.fetch_add(1, Ordering::SeqCst);
            assert!(deadline > std::time::Instant::now());
            assert_eq!(limits.model_tokens, 100_000 - 110);
            assert_eq!(limits.cost_micro_usd, 100_000 - 7);
            assert_eq!(limits.operations, 30);
            assert_eq!(evidence.citations[0].title, "1");
            let usage = WorkUsage {
                model_tokens: 10,
                cost_micro_usd: 3,
                operations: 1,
                accounting: WorkUsageAccounting::Exact,
            };
            match self.mode {
                2 => Err(WorkPublicSearchError::Rejected(usage)),
                3 => Err(WorkPublicSearchError::OutcomeUnknown),
                4 => Ok(WorkPublicSearchRanking {
                    preferred: vec![],
                    usage: WorkUsage {
                        operations: 0,
                        ..usage
                    },
                }),
                _ => Ok(WorkPublicSearchRanking {
                    preferred: if self.mode == 1 { vec![4] } else { vec![3, 1] },
                    usage,
                }),
            }
        })
    }
}
impl WorkAgentTurnProvider for RankedSearch {
    fn turn<'a>(
        &'a self,
        disclosure: &'a WorkAgentTurnDisclosure,
        _: WorkSynthesisTrace,
    ) -> WorkAgentTurnFuture<'a> {
        Box::pin(async move {
            let turn = self.turns.fetch_add(1, Ordering::SeqCst);
            assert!(turn < 4);
            if turn > 0 {
                let expected = if self.mode == 0 {
                    vec!["3", "1", "2"]
                } else {
                    vec!["1", "2", "3"]
                };
                assert_eq!(
                    disclosure
                        .context()
                        .sources
                        .iter()
                        .map(|source| source.title.as_str())
                        .collect::<Vec<_>>(),
                    expected
                );
            }
            Ok(WorkAgentTurnResult {
                output: WorkAgentTurnOutput {
                    say: None,
                    artifacts: vec![],
                    fetch: if turn == 0 {
                        vec![WorkAgentFetch::Search {
                            query: "public sources".into(),
                        }]
                    } else {
                        vec![]
                    },
                    ask: None,
                    finish: turn > 0,
                    followups: vec![],
                    malformed: 0,
                },
                usage: WorkUsage::default(),
            })
        })
    }
}

#[tokio::test]
async fn work_search_ranking_preserves_sources_and_accounts_for_refused_or_unknown_calls() {
    for mode in 0..5 {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let create = handle
            .work_document(WorkIntent::Create {
                objective: "Compare public sources".into(),
            })
            .unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let provider = RankedSearch {
            mode,
            turns: AtomicUsize::new(0),
            rankings: AtomicUsize::new(0),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            drive(
                &mut shell,
                &queue,
                WorkAgentService::new(handle).run(
                    profile,
                    WorkCommandV1 {
                        version: 1,
                        work,
                        expected_revision: WorkRevision::INITIAL,
                        command: WorkCommandId::generate(),
                        intent: WorkRuntimeIntent::BeginAgent {
                            grant: WorkAgentGrantV1 {
                                no_usage_charge: false,
                                draft: None,
                                lead: None,
                                provider: WorkSearchProvider::OpenAi,
                                model: PUBLIC_SEARCH_MODEL.into(),
                                max_turns: 8,
                                max_steps: 24,
                                browse_hops: 1,
                                folders: vec![],
                                accounts: Vec::new(),
                                private: false,
                                skill: None,
                                search_fallback: None,
                            },
                            limits: WorkExecutionLimits {
                                model_tokens: 100_000,
                                cost_micro_usd: 100_000,
                                operations: 32,
                                timeout_seconds: 30,
                                max_workers: 1,
                            },
                        },
                    },
                    None,
                    WorkAgentProviders {
                        turn: &provider,
                        search: &provider,
                    },
                    |_, _| async { panic!("ranking fixture never reads a page") },
                    |_| {},
                ),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(provider.rankings.load(Ordering::SeqCst), 1);
        let execution = &result.executions[0];
        let search = execution
            .steps
            .iter()
            .find(|step| matches!(step.kind, WorkStepKindV1::Search { .. }))
            .unwrap();
        if mode >= 3 {
            assert_eq!(search.status, WorkStepStatus::OutcomeUnknown);
            assert!(search.usage.is_none());
            assert_eq!(provider.turns.load(Ordering::SeqCst), 1);
            assert_eq!(execution.status, WorkExecutionStatus::Interrupted);
        } else {
            assert_eq!(search.status, WorkStepStatus::Succeeded);
            assert_eq!(search.usage.unwrap().model_tokens, 120);
            assert_eq!(search.usage.unwrap().cost_micro_usd, 10);
            assert_eq!(search.usage.unwrap().operations, 2);
            let record = execution
                .provider_evidence
                .iter()
                .find(|record| Some(record.id) == search.evidence)
                .unwrap();
            assert_eq!(
                record
                    .evidence
                    .citations
                    .iter()
                    .map(|source| source.title.as_str())
                    .collect::<Vec<_>>(),
                ["1", "2", "3"]
            );
            assert_eq!(record.evidence.actual_input_tokens, 100);
            assert_eq!(record.evidence.actual_output_tokens, 10);
        }
    }
}
