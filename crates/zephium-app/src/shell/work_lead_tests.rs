 call(
                    "read",
                    "web_fetch",
                    json!({"url":"https://records.example.com/one","records":{"title":"Offers","max_items":2,"columns":[
                        {"name":"price","value":{"kind":"text"},"required":false},
                        {"name":"stock","value":{"kind":"text"},"required":false},
                        {"name":"url","value":{"kind":"url"},"required":true}
                    ]}}),
                ),
                1 => {
                    assert!(seen.contains("record r1:"), "{seen}");
                    match self.0 {
                        0 | 3 => final_call(),
                        1 => call(
                            "rewrite",
                            "create",
                            json!({"kind":"sheet","title":"Offers",
                            "data":{"columns":[{"kind":"text","label":"Name"},{"kind":"text","label":"Price"}],
                            "rows":[{"cells":["First offer","FREE"]}]}}),
                        ),
                        2 => call(
                            "unknown",
                            "finish",
                            json!({"reply":{"headline":"Lost table","text":"The table is complete."},
                            "sheet":{"rows":["silently lost"]},"say":"Done","title":"Wrong result"}),
                        ),
                        5 => call(
                            "long-point",
                            "finish",
                            json!({"reply":{"headline":"Lost table","text":"Both offers were read.","points":["x".repeat(141)]},
                                "records":[{"source":"r2"},{"source":"r1"}],"layout":"picks","records_title":"Offers",
                                "reply_sources":["s1"],"say":"Done","title":"Offers"}),
                        ),
                        4 => call(
                            "unknown-source",
                            "finish",
                            json!({
                                "reply":{"headline":"Lost table","text":"Unsupported evidence."},
                                "reply_sources":["s999"],"say":"Done","title":"Wrong result"
                            }),
                        ),
                        _ => unreachable!(),
                    }
                }
                2 if self.0 != 0 => {
                    assert!(
                        seen.contains(if self.0 == 5 {
                            "140"
                        } else if self.0 == 1 {
                            "Placed sheet"
                        } else if self.0 == 4 {
                            "is not a source of this run"
                        } else {
                            "finish accepts only"
                        }),
                        "{seen}"
                    );
                    final_call()
                }
                _ => panic!("native records must settle within one repair call: {turn}: {seen}"),
            };
            if let WorkModelPart::ToolCall(tool) = &output {
                events(WorkModelEvent::ToolCall(tool.clone()));
            }
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 1000,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant: vec![output],
            })
        })
    }
}

#[tokio::test]
async fn native_records_publish_with_reply_and_finish_in_one_final_model_turn() {
    native_record_finish_fixture(0).await;
}
#[tokio::test]
async fn native_records_refuse_rewritten_values_and_repair_in_one_call() {
    native_record_finish_fixture(1).await;
}
#[tokio::test]
async fn native_records_reject_lost_finish_objects_before_early_reply_and_repair_once() {
    native_record_finish_fixture(2).await;
}
#[tokio::test]
async fn cited_reply_and_records_finish_in_one_call_with_original_evidence() {
    native_record_finish_fixture(3).await;
}
#[tokio::test]
async fn unknown_finish_reply_source_refuses_before_early_publication() {
    native_record_finish_fixture(4).await;
}
#[tokio::test]
async fn overlong_final_reply_point_is_clipped_without_a_repair_call() {
    native_record_finish_fixture(5).await;
}
async fn native_record_finish_fixture(mode: u8) {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Read https://records.example.com/one and compare the two offers".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(NativeRecordFinish(mode)),
        ..model(Arc::new(Script {
            calls: Mutex::new(vec![]),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let done = drive(&mut shell,&queue,WorkLeadService::new(handle).run(profile,command(work,WorkRevision::INITIAL),None,models,&Search,|_,request|async move {
        let WorkStepKindV1::Read { collection: Some(collection), .. } = &request.step else { panic!("native records must be requested on the first read") };
        assert_eq!(collection.max_items, 2);
        assert_eq!(collection.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["price", "stock", "url"]);
        let source = WorkEvidenceLink { extraction_id:WorkArtifactId::from(940),source_id:1 };
        Ok(outcome(None,vec![WorkArtifactDraft {
            output:request.output,title:"Original records".into(), evidence:vec![source],
            data:serde_json::from_value(json!({"kind":"comparison_matrix",
                "subjects":[{"name":"First offer","homepage":"https://records.example.com/item?id=1&variant=blue"},{"name":"Second offer","homepage":"https://records.example.com/item?id=2&variant=red"}],
                "criteria":[{"name":"price","kind":{"kind":"text"}},{"name":"stock","kind":{"kind":"text"}},{"name":"url","kind":{"kind":"text"}}],
                "cells":[[{"value":{"kind":"text","text":"129,99 zł"},"evidence":[0]},{"value":{"kind":"text","text":"In stock"},"evidence":[0]},{"value":{"kind":"text","text":"https://records.example.com/item?id=1&variant=blue"},"evidence":[0]}],
                         [{"value":{"kind":"text","text":"€32.50 / item"},"evidence":[0]},{"value":{"kind":"text","text":"Out of stock"},"evidence":[0]},{"value":{"kind":"text","text":"https://records.example.com/item?id=2&variant=red"},"evidence":[0]}]]
            })).unwrap()
        }]))
    },|_|{})).await.unwrap();
    let execution = &done.executions[0];
    assert!(matches!(
        execution.status,
        WorkExecutionStatus::NeedsReview | WorkExecutionStatus::Completed
    ));
    let picks: Vec<_> = execution
        .artifacts
        .iter()
        .filter_map(|artifact| match &artifact.data {
            WorkArtifactDataV1::Picks { items, .. } => Some(items),
            _ => None,
        })
        .collect();
    // Rows naming records stand as those records: the page's price, never
    // the model's.
    let stored = serde_json::to_string(&execution.artifacts).unwrap();
    assert!(
        !stored.contains("FREE"),
        "a model rewrite must never reach storage"
    );
    assert_eq!(
        stored.contains("\"sheet\""),
        mode == 1,
        "only a sheet of records may stand"
    );
    if mode == 5 {
        // An overlong point is clipped between words; the finish needs no repair.
        assert!(execution
            .artifacts
            .iter()
            .any(|a| matches!(&a.data, WorkArtifactDataV1::Reply {points,..}
            if points.first().is_some_and(|p| p.chars().count() == 140 && p.ends_with('…')))));
    } else {
        assert!(!execution.artifacts.iter().any(|a| matches!(&a.data, WorkArtifactDataV1::Reply {headline,..} if headline == "Lost table")),
            "invalid finish fields must refuse before early publication");
    }
    assert_eq!(picks.len(), 1);
    assert_eq!(picks[0][0].name, "Second offer");
    assert_eq!(picks[0][0].price.as_ref().unwrap().display, "€32.50 / item");
    assert_eq!(
        picks[0][1].url.as_deref(),
        Some("https://records.example.com/item?id=1&variant=blue")
    );
    assert_eq!(picks[0][1].facts[0].value, "In stock");
    assert_eq!(
        execution
            .artifacts
            .iter()
            .filter(|a| matches!(a.data, WorkArtifactDataV1::Reply { .. }))
            .count(),
        1
    );
    if mode >= 3 {
        let reply = execution
            .artifacts
            .iter()
            .find(|a| matches!(a.data, WorkArtifactDataV1::Reply { .. }))
            .unwrap();
        assert_eq!(
            reply.evidence,
            vec![WorkEvidenceLink {
                extraction_id: WorkArtifactId::from(940),
                source_id: 1
            }]
        );
        assert!(!reply.general_knowledge);
    }
}

struct FailedCatalogThenDetail;
impl WorkModelClient for FailedCatalogThenDetail {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let lead = request
                .tools
                .iter()
                .any(|tool| matches!(tool.name.as_str(), "start_part" | "continue_work"));
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            let assistant = if !lead {
                vec![call(
                    "part-end",
                    "finish",
                    json!({"summary":"Catalog unavailable","digest":"No catalog rows were returned.","found":false}),
                )]
            } else {
                match turn {
                    0 => vec![call(
                        "catalog",
                        "start_part",
                        json!({"helper":"browser","title":"Books","goal":"Find books and return their prices and availability","service":"shop.example.com","records":["price","availability","url"]}),
                    )],
                    1 => vec![call(
                        "detail",
                        "web_fetch",
                        json!({"url":"https://shop.example.com/book"}),
                    )],
                    2 => {
                        assert!(results(&request).contains("record r1:"));
                        vec![
                            call(
                                "reply",
                                "create",
                                json!({"kind":"reply","title":"Book found","data":{"headline":"Book found","text":"The detail page supplied price and availability."}}),
                            ),
                            call(
                                "pick",
                                "create",
                                json!({"kind":"picks","title":"Book","data":{"records":[{"source":"r1"}]}}),
                            ),
                            call(
                                "done",
                                "finish",
                                json!({"say":"The original record is preserved.","title":"Book availability"}),
                            ),
                        ]
                    }
                    _ => panic!(
                        "unexpected additional lead turn {turn}: {}",
                        results(&request)
                    ),
                }
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                assistant,
                usage: WorkModelUsage {
                    input_tokens: 100,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
            })
        })
    }
}

#[tokio::test]
async fn a_failed_browser_catalog_keeps_stock_in_the_leads_fallback_detail_read() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create=handle.work_document(WorkIntent::Create{objective:"Find an available book at https://shop.example.com/catalog with its price and availability".into()}).unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(FailedCatalogThenDetail),
        ..model(Arc::new(Script {
            calls: Mutex::new(vec![]),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let done=drive(&mut shell,&queue,WorkLeadService::new(handle).run(profile,command(work,WorkRevision::INITIAL),None,models,&Search,|_,request|async move{
        let WorkStepKindV1::Read{url,collection,..}=&request.step else {panic!()};
        if url.ends_with("/catalog") {return Err(WorkError::Unavailable);}
        assert!(url.ends_with("/book"));
        let collection=collection.as_ref().expect("fallback must retain requested native records");
        assert!(collection.columns.iter().any(|column|column.name=="availability"));
        let source=WorkEvidenceLink{extraction_id:WorkArtifactId::from(950),source_id:1};
        Ok(outcome(None,vec![WorkArtifactDraft{output:request.output,title:"Book metadata".into(),evidence:vec![source],data:serde_json::from_value(json!({
            "kind":"comparison_matrix","subjects":[{"name":"Book Alpha","homepage":"https://shop.example.com/book"}],
            "criteria":[{"name":"price","kind":{"kind":"text"}},{"name":"availability","kind":{"kind":"text"}}],
            "cells":[[{"value":{"kind":"text","text":"14.99 PLN"},"evidence":[0]},{"value":{"kind":"text","text":"InStock"},"evidence":[0]}]]
        })).unwrap()}]))
    },|_|{})).await.unwrap();
    let execution = &done.executions[0];
    assert!(matches!(
        execution.status,
        WorkExecutionStatus::Completed | WorkExecutionStatus::NeedsReview
    ));
    let pick = execution
        .artifacts
        .iter()
        .find_map(|artifact| match &artifact.data {
            WorkArtifactDataV1::Picks { items, .. } => items.first(),
            _ => None,
        })
        .unwrap();
    assert_eq!(pick.price.as_ref().unwrap().display, "14.99 PLN");
    assert_eq!(pick.facts[0].value, "InStock");
}
