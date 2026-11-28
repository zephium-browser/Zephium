t(Instant::now).elapsed().as_millis()}));
                        Some(index)
                    }).flatten();
                    if public_eval {
                        settings.model_diagnostic = Some(|event| say(format_args!("page-model: event={event:?}")));
                    }
                    let result = composition.run_agent_step(callback, &probe, request, settings).await;
                    if let Some(index) = page_index {
                        let mut diagnostics = page_diagnostics.lock().unwrap_or_else(|e| e.into_inner());
                        diagnostics.pages[index]["finished_ms"] = serde_json::json!(STARTED.get_or_init(Instant::now).elapsed().as_millis());
                    }
                    result
                }
            },
            |_| {},
        );
        let result = if let Some(account) = super::work_account::active() {
            tokio::select! {
                result = running => result,
                _ = account.person(handle, composition, profile, work) => Err(WorkError::Unavailable),
            }
        } else if std::env::var_os("ZEPHIUM_PROBE_PERSON").is_some() {
            tokio::select! {
                result = running => result,
                () = super::work_account::present_human_pages(composition, profile, work) => Err(WorkError::Unavailable),
            }
        } else {
            // Nobody sits at the window: a check stays unopened, as in the
            // app until the person opens it, and its own short wait applies.
            running.await
        };
        let projection = match result {
            Ok(projection) => projection,
            Err(error) => {
                say(format_args!(
                    "lead-run: scenario={} request={} run_failure={error:?}",
                    scenario.name,
                    index + 1
                ));
                failure = Some("lead_run");
                break;
            }
        };
        expected = projection.work.revision;
        let execution = projection.executions.last().ok_or("lead_execution")?;
        run_row(
            scenario.name,
            index + 1,
            execution,
            started.elapsed().as_millis(),
        );
        if let Some(account) = super::work_account::active() {
            account.report(execution);
        }
        if !public_eval && !block_probe && !account_probe && !local_probe {
            page_rows(execution);
            part_rows(execution);
        }
        enrich_media_for_mode(account_probe, local_probe, || media_row(execution)).await;
        if !matches!(
            execution.status,
            WorkExecutionStatus::Completed | WorkExecutionStatus::NeedsReview
        ) {
            failure = failure.or(Some("lead_outcome"));
        }
        state = Some(projection);
    }
    if let Some(person) = person {
        person.abort();
    }
    if let Some(pick) = pick {
        let written = std::fs::read_dir(&pick)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.metadata().is_ok_and(|meta| meta.len() > 0))
                    .count()
            })
            .unwrap_or(0);
        say(format_args!(
            "lead-run: scenario=save files_written={written}"
        ));
        let _ = std::fs::remove_dir_all(&pick);
        if written == 0 {
            failure = failure.or(Some("save_nothing_written"));
        }
    }
    if let Some(folder) = folder.filter(|_| !local_probe) {
        let fixed = std::fs::read_to_string(folder.join("pricing.py")).unwrap_or_default();
        say(format_args!(
            "lead-run: scenario=bug fixed={}",
            fixed.contains("nightly * nights")
        ));
        let _ = std::fs::remove_dir_all(&folder);
    }
    if let Some(host) = &memory_host {
        say(format_args!(
            "work-memory: profile={profile} stage=after stores(own,work)={:?}",
            super::work_site::stores_hold(profile, host)
        ));
        if std::env::var_os("ZEPHIUM_PROBE_WORK_MEMORY_CLEAR").is_some() {
            say(format_args!(
                "work-memory: profile={profile} stage=cleared empty={:?} stores(own,work)={:?}",
                super::work_site::clear_work_memory(profile),
                super::work_site::stores_hold(profile, host)
            ));
        }
    }
    let state = state.ok_or("lead_no_run")?;
    if let Some(fixture) = &block_fixture {
        if let Err(error) = super::work_blocks::verify(handle, profile, fixture, &state).await {
            failure = failure.or(Some(error));
        }
    }
    let fixture_diagnostics = || {
        let diagnostics = local_diagnostics.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::json!({
            "selected_client_calls": diagnostics.calls,
            "truncated": diagnostics.truncated,
            "scope": "selected Work model clients; no browser/search dispatch permitted",
            "attempt_usage": state.executions.last().map(|e| e.attempts.iter().map(|a| a.usage).collect::<Vec<_>>()),
            "step_usage": state.executions.last().map(|e| e.steps.iter().map(|s| s.usage).collect::<Vec<_>>()),
        })
    };
    if let Some(fixture) = local_fixture {
        if !fixture.export(
            &state,
            &model_setup,
            fixture_diagnostics(),
            eval_started.elapsed().as_millis(),
        )? {
            failure = failure.or(Some("local_outcome"));
        }
    } else if let Some(fixture) = heldout_blocks {
        if !fixture
            .export(
                handle,
                profile,
                &state,
                &model_setup,
                fixture_diagnostics(),
                eval_started.elapsed().as_millis(),
            )
            .await?
        {
            failure = failure.or(Some("heldout_blocks_outcome"));
        }
    } else if let Some(path) = export {
        let wall_ms = eval_started.elapsed().as_millis();
        let finished_ms = STARTED.get_or_init(Instant::now).elapsed().as_millis();
        let links = state
            .executions
            .iter()
            .flat_map(|execution| execution.artifacts.iter())
            .flat_map(|artifact| artifact.evidence.iter().cloned())
            .collect();
        let source_evidence =
            collect_eval_sources(public_export_allowed(&state), links, |link| async move {
                let response = handle
                    .work_evidence(profile, work, link)
                    .map_err(|_| ())?
                    .response(profile)
                    .await;
                match response.reply {
                    WorkReplyV1::Evidence { evidence } => Ok(*evidence),
                    _ => Err(()),
                }
            })
            .await;
        export_eval(
            &path,
            &state,
            &model_setup,
            &local_diagnostics,
            wall_ms,
            finished_ms,
            &source_evidence,
        )?;
    } else if !block_probe && !account_probe {
        let directory = std::path::Path::new("target/work-runtime-proof/lead");
        let _ = std::fs::create_dir_all(directory);
        let _ = std::fs::write(
            directory.join(format!("{}.json", scenario.name)),
            serde_json::to_vec_pretty(&state).unwrap_or_default(),
        );
    }
    Ok(super::work_durable::WorkflowResult { state, failure })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn evidence_export_never_reads_private_mode_and_deduplicates_public_links() {
        use zephium_core::work::artifact::*;
        let rows: Vec<WorkEvidencePreviewV1> = serde_json::from_str(include_str!(
            "../../zephium-ipc/fixtures/work-research-evidence-v1.json"
        ))
        .unwrap();
        let preview = rows[0].clone();
        let called = std::sync::atomic::AtomicUsize::new(0);
        let private = collect_eval_sources(false, vec![preview.link.clone()], |_| async {
            called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(preview.clone())
        })
        .await;
        assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(private["state"], "withheld_private_provenance");
        let public = collect_eval_sources(
            true,
            vec![preview.link.clone(), preview.link.clone()],
            |_| async {
                called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(preview.clone())
            },
        )
        .await;
        assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(public["previews"].as_array().unwrap().len(), 1);
        assert_eq!(public["previews"][0]["text"], preview.text);
        assert_eq!(public["previews"][0]["truncated"], preview.truncated);
    }

    #[tokio::test]
    async fn evidence_export_rejects_wrong_links_local_sources_and_excess_text() {
        use zephium_core::work::artifact::*;
        let rows: Vec<WorkEvidencePreviewV1> = serde_json::from_str(include_str!(
            "../../zephium-ipc/fixtures/work-research-evidence-v1.json"
        ))
        .unwrap();
        let original = rows[0].clone();
        let mut wrong = original.clone();
        wrong.link.source_id = wrong.link.source_id.saturating_add(1);
        let mut local = original.clone();
        local.source = WorkEvidenceSourceV1::File {
            path: "/private/never-export".into(),
            name: "private".into(),
            file_kind: zephium_core::work::runtime::WorkFileKindV1::Text,
        };
        local.text = "PRIVATE FIXTURE TEXT".into();
        for refused in [wrong, local] {
            let result = collect_eval_sources(true, vec![original.link.clone()], |_| async {
                Ok(refused.clone())
            })
            .await;
            assert_eq!(result["failed"], 1);
            assert!(result["previews"].as_array().unwrap().is_empty());
            assert!(!result.to_string().contains("PRIVATE FIXTURE TEXT"));
        }
        let mut oversized = original.clone();
        oversized.text = "x".repeat(2 * 1024 * 1024);
        let result = collect_eval_sources(true, vec![original.link.clone()], |_| async {
            Ok(oversized.clone())
        })
        .await;
        assert_eq!(result["limited"], true);
        assert!(result["previews"].as_array().unwrap().is_empty());
    }

    #[test]
    fn image_evidence_is_local_public_only_and_requires_a_settled_call() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("evidence.json");
        let mut diagnostics = EvalDiagnostics::default();
        diagnostics.images.push(EvalInputImage {
            hash: "fixture".into(),
            extension: "png",
            bytes: vec![1, 2, 3],
        });
        diagnostics
            .calls
            .push(serde_json::json!({"state":"settled","input_image_hashes":["fixture"]}));
        assert_eq!(
            export_eval_images(&path, false, &diagnostics).unwrap()["state"],
            "withheld_private_provenance"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        diagnostics.calls[0]["state"] = serde_json::json!("refused");
        assert!(
            export_eval_images(&path, true, &diagnostics).unwrap()["images"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        diagnostics.calls[0]["state"] = serde_json::json!("settled");
        let value = export_eval_images(&path, true, &diagnostics).unwrap();
        assert_eq!(value["images"].as_array().unwrap().len(), 1);
        assert_eq!(
            std::fs::read(directory.path().join("model-input-fixture.png")).unwrap(),
            [1, 2, 3]
        );
    }

    #[test]
    fn public_export_refuses_site_consented_possible_image_disclosure() {
        use zephium_core::work::runtime::WorkPageImageDisclosureV1;
        assert!(public_image_provenance(None));
        assert!(public_image_provenance(Some(
            WorkPageImageDisclosureV1::PublicIsolated
        )));
        assert!(!public_image_provenance(Some(
            WorkPageImageDisclosureV1::SiteConsent
        )));
    }

    #[tokio::test]
    async fn local_and_account_modes_never_invoke_media_candidate_fetch() {
        let candidate = "https://example.com/inert-candidate.png";
        for (account, local) in [(false, true), (true, false), (true, true)] {
            let invoked = std::sync::atomic::AtomicBool::new(false);
            enrich_media_for_mode(account, local, || async {
                assert!(!candidate.is_empty());
                invoked.store(true, std::sync::atomic::Ordering::SeqCst);
            })
            .await;
            assert!(!invoked.load(std::sync::atomic::Ordering::SeqCst));
        }
    }

    #[test]
    fn public_page_diagnostic_is_explicit_and_never_for_accounts() {
        assert!(public_page_diagnostic_enabled(true, false, Some("1")));
        for value in [None, Some(""), Some("true"), Some("1")] {
            assert!(!public_page_diagnostic_enabled(false, false, value));
            assert!(!public_page_diagnostic_enabled(true, true, value));
        }
        assert!(!public_page_diagnostic_enabled(true, false, None));
    }

    #[test]
    fn a_child_writing_more_than_a_mebibyte_completes_and_a_hung_one_is_stopped() {
        let directory = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let mut child = spawn_logged(
            Command::new("/bin/sh").args([
                "-c",
                "head -c 3000000 /dev/zero | tr '\\0' x; echo done >&2",
            ]),
            directory.path(),
        )
        .unwrap();
        let status = loop {
            if let Some(status) = settled(&mut child, started, Duration::from_secs(60)).unwrap() {
                break status;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.is_some_and(|status| status.success()));
        assert_eq!(
            std::fs::metadata(directory.path().join("run.log"))
                .unwrap()
                .len(),
            3_000_000
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("stderr.log")).unwrap(),
            "done\n"
        );
        let started = Instant::now();
        let mut child =
            spawn_logged(Command::new("/bin/sleep").arg("60"), directory.path()).unwrap();
        let status = loop {
            if let Some(status) = settled(&mut child, started, Duration::from_millis(200)).unwrap()
            {
                break status;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.is_none());
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
