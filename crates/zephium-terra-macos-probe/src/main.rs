//! Explicit, release-excluded live Terra semantic-action qualification.

#![forbid(unsafe_code)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod acceptance;
#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod assist_bench;
#[cfg(any(all(target_os = "macos", feature = "durable-runtime"), test))]
mod assist_scenarios;
#[cfg(all(target_os = "macos", feature = "decision-eval"))]
mod decision_eval;
#[cfg(all(target_os = "macos", feature = "decision-eval"))]
mod decision_observation;
#[cfg(target_os = "macos")]
mod lead_smoke;
#[cfg(target_os = "macos")]
mod loopback_support;
#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod work_account;
#[cfg(target_os = "macos")]
mod work_actor;
#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod work_app_editors;
#[cfg(target_os = "macos")]
mod work_app_views;
#[cfg(target_os = "macos")]
mod work_application;
#[cfg(target_os = "macos")]
mod work_artifact_cleanup;
#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod work_blocks;
#[cfg(any(target_os = "macos", test))]
mod work_commerce;
#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod work_durable;
#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod work_heldout_blocks;
#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod work_local_fixture;
#[cfg(any(target_os = "macos", test))]
mod work_navigation;
#[cfg(any(target_os = "macos", test))]
mod work_route;
#[cfg(all(target_os = "macos", feature = "durable-runtime"))]
mod work_site;
#[cfg(any(target_os = "macos", test))]
mod work_sites;

#[cfg(not(target_os = "macos"))]
fn main() {
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    #[cfg(feature = "durable-runtime")]
    zephium_app::work_commands::private_guard_main();
    use std::io::Write as _;

    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let result = match arguments.as_slice() {
        #[cfg(feature = "decision-eval")]
        [argument] if argument == "--live-decision-eval" => decision_eval::run(),
        #[cfg(feature = "decision-eval")]
        [argument, case] if argument == "--live-decision-eval-case" => {
            decision_eval::run_case(case, ProbeModel::Luna)
        }
        #[cfg(feature = "decision-eval")]
        [argument] if argument == "--live-decision-eval-terra" => decision_eval::run_terra(),
        #[cfg(feature = "decision-eval")]
        [argument, paths @ ..] if argument == "--search-reuse-replay" && !paths.is_empty() => {
            decision_eval::search_reuse_replay(paths)
        }
        #[cfg(feature = "decision-eval")]
        [argument, effort] if argument == "--live-decision-eval-effort" => {
            decision_eval::run_effort(effort)
        }
        #[cfg(feature = "decision-eval")]
        [argument, case] if argument == "--live-decision-eval-case-terra" => {
            decision_eval::run_case(case, ProbeModel::Terra)
        }
        #[cfg(feature = "decision-eval")]
        [argument, site] if argument == "--record-decision-observation" => {
            decision_observation::run(site)
        }
        #[cfg(feature = "decision-eval")]
        [argument, site, lists]
            if argument == "--record-decision-observation" && lists == "release-lists" =>
        {
            decision_observation::use_release_lists().and_then(|()| decision_observation::run(site))
        }
        [argument] if argument == "--check-provider-keychain" => {
            let started = std::time::Instant::now();
            let result = zephium_agentic::load_macos_probe_openai_credential()
                .map(|_| ())
                .map_err(|_| ProbeFailure::Keychain);
            let _ = writeln!(
                std::io::stderr(),
                "keychain_check available={} elapsed_ms={}",
                result.is_ok(),
                started.elapsed().as_millis()
            );
            result
        }
        [argument] if argument == "--live-lead-smoke" => lead_smoke::run(),
        #[cfg(feature = "durable-runtime")]
        [argument, body, model, thinking, times] if argument == "--live-page-replay" => {
            lead_smoke::replay_page(body, model, thinking, times)
        }
        [argument] if argument == "--live-fixed-click" => run_fixed_click(),
        [argument] if argument == "--live-public-wikipedia-fill" => run_public_wikipedia_fill(),
        [argument] if argument == "--live-two-action" => run_two_action(
            TwoActionScenario::FixedClickFill,
            ProbeModel::Terra,
            ProbeRetention::Stateless,
        ),
        [argument] if argument == "--live-public-wikipedia-form" => run_two_action(
            TwoActionScenario::PublicFillSelect,
            ProbeModel::Terra,
            ProbeRetention::Stateless,
        ),
        [argument] if argument == "--live-public-wikipedia-form-locate" => run_two_action(
            TwoActionScenario::PublicFillLocateSelect,
            ProbeModel::Terra,
            ProbeRetention::Stateless,
        ),
        [argument] if argument == "--live-public-luna-form-inspectable" => run_two_action(
            TwoActionScenario::PublicFillSelect,
            ProbeModel::Luna,
            ProbeRetention::InspectablePublicData,
        ),
        [argument] if argument == "--live-public-luna-form-locate-inspectable" => run_two_action(
            TwoActionScenario::PublicFillLocateSelect,
            ProbeModel::Luna,
            ProbeRetention::InspectablePublicData,
        ),
        [argument] if argument == "--live-public-luna-suite-inspectable" => {
            run_luna_inspectable_suite()
        }
        [argument] if argument == "--live-public-luna-workflow-inspectable" => {
            run_variable_workflow()
        }
        [argument] if argument == "--live-public-luna-work-actor-inspectable" => work_actor::run(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-public-durable-work" => work_durable::run(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-public-coordinated-work" => {
            work_durable::run_coordinated()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-public-cancelled-work" => work_durable::run_cancelled(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-public-work-product" => work_durable::run_product(),
        #[cfg(feature = "durable-runtime")]
        [argument, path, count] if argument == "--replay-agent-turn" => {
            work_durable::replay_agent_turn(path, count, None)
        }
        #[cfg(feature = "durable-runtime")]
        [argument, path, count, effort] if argument == "--replay-agent-turn" => {
            work_durable::replay_agent_turn(path, count, Some(effort))
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-acceptance" => acceptance::run(),
        #[cfg(feature = "durable-runtime")]
        [argument, scenario] if argument == "--live-acceptance-only" => {
            acceptance::run_one(scenario)
        }
        #[cfg(feature = "durable-runtime")]
        [argument, check] if argument == "--loopback-site" => work_site::run(check),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-work" => work_durable::run_agent(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-read-work" => work_durable::run_agent_read(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-human-government-work" => {
            work_durable::run_agent_human_government()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-government-work" => {
            work_durable::run_agent_government()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-files-work" => work_durable::run_agent_files(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-disclosure-work" => {
            work_durable::run_agent_disclosure()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-scroll-work" => work_durable::run_agent_scroll(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-money-node-work" => work_durable::run_money_node(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-money-work" => work_durable::run_agent_money(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-architecture-work" => {
            work_durable::run_agent_architecture()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-engine-chart-work" => {
            work_durable::run_agent_engine_chart()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-explain-mechanism-work" => {
            work_durable::run_agent_explain_mechanism()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-code-review-work" => {
            work_durable::run_agent_code_review()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-explain-rust-work" => {
            work_durable::run_agent_explain_rust()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-concept-comparison-work" => {
            work_durable::run_agent_concept_comparison()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-trip-work" => work_durable::run_agent_trip(),
        #[cfg(feature = "durable-runtime")]
        [argument, scenario] if argument == "--live-lead" => work_durable::run_lead(scenario),
        #[cfg(feature = "durable-runtime")]
        [argument, scenario] if argument == "--assist-bench" => assist_bench::run(scenario),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--assist-bench-list" => {
            for scenario in assist_scenarios::SCENARIOS {
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "{}\t{}\t{}",
                    scenario.name,
                    if scenario.live { "live" } else { "loopback" },
                    scenario.request
                );
            }
            Ok(())
        }
        #[cfg(feature = "durable-runtime")]
        [argument, case, export] if argument == "--live-local-fixture" => {
            work_durable::run_local_fixture(case, export)
        }
        [argument, request, answer] if argument == "--live-lead-request" => {
            work_durable::run_lead_request(request, answer)
        }
        #[cfg(feature = "durable-runtime")]
        [argument, start] if argument == "--live-account-read" => {
            work_durable::run_account_read(start)
        }
        #[cfg(feature = "durable-runtime")]
        [argument, start] if argument == "--live-disposable-slack-send" => {
            work_durable::run_account_send(start)
        }
        #[cfg(feature = "durable-runtime")]
        [argument, start, goal] if argument == "--live-site" => work_durable::run_site(start, goal),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-airbnb-work" => work_durable::run_agent_airbnb(),
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-airbnb-listing-work" => {
            work_durable::run_agent_listing()
        }
        #[cfg(feature = "durable-runtime")]
        [argument, url] if argument == "--live-agent-page-work" => {
            work_durable::run_agent_page(url)
        }
        #[cfg(feature = "durable-runtime")]
        [argument, url] if argument == "--live-agent-listing-page-work" => {
            work_durable::run_agent_listing_page(url)
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-product-details-work" => {
            work_durable::run_agent_details()
        }
        #[cfg(feature = "durable-runtime")]
        [argument] if argument == "--live-agent-collection-work" => {
            work_durable::run_agent_collection()
        }
        [argument] if argument == "--live-public-luna-work-application-inspectable" => {
            work_application::run()
        }
        [argument] if argument == "--live-public-luna-work-extraction-inspectable" => {
            work_application::run_extraction()
        }
        [argument] if argument == "--live-public-luna-work-artifact-inspectable" => {
            work_application::run_artifact()
        }
        [argument] if argument == "--live-public-luna-work-cancellation-inspectable" => {
            work_application::run_cancellation()
        }
        [argument] if argument == "--live-public-luna-work-sequential-inspectable" => {
            work_application::run_sequential()
        }
        [argument] if argument == "--live-public-luna-work-combined-inspectable" => {
            work_application::run_combined()
        }
        [argument] if argument == "--live-public-luna-work-scoped-inspectable" => {
            work_application::run_scoped()
        }
        [argument] if argument == "--live-public-luna-work-review-inspectable" => {
            work_application::run_review()
        }
        [argument] if argument == "--live-public-luna-work-read-inspectable" => {
            work_application::run_read()
        }
        [argument, site] if argument == "--live-public-luna-work-site-inspectable" => site
            .to_str()
            .ok_or(ProbeFailure::Authority)
            .and_then(|site| work_sites::Site::parse(site).map_err(|_| ProbeFailure::Authority))
            .and_then(work_application::run_site),
        [argument, directory] if argument == "--cleanup-public-work-artifact" => {
            work_artifact_cleanup::recover(std::path::Path::new(directory))
        }
        _ => std::process::exit(2),
    };
    if let Err(error) = result {
        if let ProbeFailure::Provider(reason) = error {
            let _ = writeln!(
                std::io::stderr().lock(),
                "browser-workflow-provider: refusal={reason:?}; content=redacted"
            );
        }
        let _ = writeln!(
            std::io::stderr().lock(),
            "macos-terra-agentic-probe: failed; stage={}; action_step={}; tool_kind={}; engine_reason={}; encoding_reason={}; verification_reason={}; wait={}; settle_millis={}; protocol_event={}; content=redacted",
            error.label(),
            error.action_step(),
            error.tool_kind_label(),
            error.engine_reason_label(),
            error.encoding_reason_label(),
            error.verification_reason_label(),
            error.wait_label(),
            error.settle_millis(),
            error.protocol_event_label()
        );
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
type ProbeModel = zephium_agent_controller::AgenticProbeModel;

#[cfg(target_os = "macos")]
type ProbeRetention = zephium_agent_controller::AgenticProbeRetention;

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum ProbeFailure {
    Runtime,
    SuiteLaunch,
    SuiteChild,
    Keychain,
    Authority,
    Provider(zephium_agent_controller::TerraProbeProviderError),
    Proposal {
        step: u8,
        error: Option<zephium_agent_controller::TerraProbeActionBridgeError>,
    },
    Engine(&'static str),
    Verification,
    PostFirstActionVerification {
        error: zephium_agent_controller::TerraProbeActionBridgeError,
        wait: zephium_agentic::SemanticWaitCondition,
        settle_millis: u32,
    },
    PostSecondActionVerification {
        error: zephium_agent_controller::TerraProbeActionBridgeError,
        wait: zephium_agentic::SemanticWaitCondition,
        settle_millis: u32,
    },
    Metrics,
    Output,
}

#[cfg(target_os = "macos")]
impl ProbeFailure {
    const fn label(self) -> &'static str {
        use zephium_agent_controller::TerraProbeProviderError;
        use zephium_agentic::{AgentProviderFailureClass, AgentProviderProtocolError};

        match self {
            Self::Provider(TerraProbeProviderError::Read(_)) => "semantic_read_authority",
            Self::Provider(zephium_agent_controller::TerraProbeProviderError::Extraction(_)) => {
                "extraction_output"
            }
            Self::Runtime => "runtime",
            Self::SuiteLaunch => "suite_launch",
            Self::SuiteChild => "suite_child",
            Self::Keychain => "keychain",
            Self::Authority => "authority",
            Self::Provider(TerraProbeProviderError::Authority) => "provider_authority",
            Self::Provider(TerraProbeProviderError::Account(_)) => {
                "controller_account_reattestation"
            }
            Self::Provider(TerraProbeProviderError::Navigation(_)) => {
                "controller_navigation_refused"
            }
            Self::Provider(TerraProbeProviderError::Cancelled) => "controller_cancelled",
            Self::Provider(TerraProbeProviderError::ActionPending) => "controller_action_pending",
            Self::Provider(TerraProbeProviderError::ActionLimit) => "controller_action_limit",
            Self::Provider(TerraProbeProviderError::Action(_)) => "controller_action_refused",
            Self::Provider(TerraProbeProviderError::ActionRejected(_)) => {
                "controller_action_rejected"
            }
            Self::Provider(TerraProbeProviderError::ActionUnverified) => {
                "controller_action_unverified"
            }
            Self::Provider(TerraProbeProviderError::UnsupportedTool(_)) => {
                "controller_tool_unsupported"
            }
            Self::Provider(TerraProbeProviderError::Catalog) => "provider_catalog",
            Self::Provider(TerraProbeProviderError::Clock) => "provider_clock",
            Self::Provider(TerraProbeProviderError::Deadline) => "provider_deadline",
            Self::Provider(TerraProbeProviderError::InitialEncoding(_)) => {
                "provider_initial_encoding"
            }
            Self::Provider(TerraProbeProviderError::ReadEncoding(_)) => "provider_read_encoding",
            Self::Provider(TerraProbeProviderError::DiffEncoding(_)) => "provider_diff_encoding",
            Self::Provider(TerraProbeProviderError::LocateTool) => "provider_locate_tool",
            Self::Provider(TerraProbeProviderError::Locate) => "provider_locate",
            Self::Provider(TerraProbeProviderError::LocateEncoding(_)) => {
                "provider_locate_encoding"
            }
            Self::Provider(TerraProbeProviderError::Transport) => "provider_transport",
            Self::Provider(TerraProbeProviderError::Journal) => "product_journal",
            Self::Provider(TerraProbeProviderError::PreDispatchTerminal(_)) => {
                "provider_pre_dispatch_terminal"
            }
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Authentication,
            )) => "provider_count_authentication",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::InvalidRequest,
            )) => "provider_count_invalid_request",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Permission,
            )) => "provider_count_permission",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::NotFound,
            )) => "provider_count_not_found",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Conflict,
            )) => "provider_count_conflict",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::RateLimited,
            )) => "provider_count_rate_limited",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Overloaded,
            )) => "provider_count_overloaded",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Timeout,
            )) => "provider_count_timeout",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Transport,
            )) => "provider_count_transport",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Protocol,
            )) => "provider_count_protocol",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Integration,
            )) => "provider_count_integration",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Provider,
            )) => "provider_count_provider",
            Self::Provider(TerraProbeProviderError::CountTerminal(
                AgentProviderFailureClass::Cancelled,
            )) => "provider_count_cancelled",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Authentication,
            )) => "provider_model_authentication",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::InvalidRequest,
            )) => "provider_model_invalid_request",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Permission,
            )) => "provider_model_permission",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::NotFound,
            )) => "provider_model_not_found",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Conflict,
            )) => "provider_model_conflict",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::RateLimited,
            )) => "provider_model_rate_limited",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Overloaded,
            )) => "provider_model_overloaded",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Timeout,
            )) => "provider_model_timeout",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Transport,
            )) => "provider_model_transport",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Protocol,
            )) => "provider_model_protocol",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Integration,
            )) => "provider_model_integration",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Provider,
            )) => "provider_model_provider",
            Self::Provider(TerraProbeProviderError::ModelTerminal(
                AgentProviderFailureClass::Cancelled,
            )) => "provider_model_cancelled",
            Self::Provider(TerraProbeProviderError::ModelProtocol(
                AgentProviderProtocolError::Framing,
                _,
            )) => "provider_model_protocol_framing",
            Self::Provider(TerraProbeProviderError::ModelProtocol(
                AgentProviderProtocolError::Limit,
                _,
            )) => "provider_model_protocol_limit",
            Self::Provider(TerraProbeProviderError::ModelProtocol(
                AgentProviderProtocolError::Event,
                _,
            )) => "provider_model_protocol_event",
            Self::Provider(TerraProbeProviderError::ModelProtocol(
                AgentProviderProtocolError::Sequence,
                _,
            )) => "provider_model_protocol_sequence",
            Self::Provider(TerraProbeProviderError::ModelProtocol(
                AgentProviderProtocolError::UnsupportedOutput,
                _,
            )) => "provider_model_protocol_unsupported_output",
            Self::Provider(TerraProbeProviderError::ModelProtocol(
                AgentProviderProtocolError::ToolCall
                | AgentProviderProtocolError::ToolContract(_, _),
                _,
            )) => "provider_model_protocol_tool_call",
            Self::Provider(TerraProbeProviderError::ModelProtocol(
                AgentProviderProtocolError::Usage,
                _,
            )) => "provider_model_protocol_usage",
            Self::Provider(TerraProbeProviderError::ModelProtocol(
                AgentProviderProtocolError::Terminal,
                _,
            )) => "provider_model_protocol_terminal",
            Self::Provider(TerraProbeProviderError::Settlement) => "provider_settlement",
            Self::Provider(TerraProbeProviderError::Proposal) => "provider_proposal",
            Self::Provider(TerraProbeProviderError::Continuation) => "provider_continuation",
            Self::Provider(TerraProbeProviderError::TurnLimit) => "provider_turn_limit",
            Self::Provider(TerraProbeProviderError::NoExtractionEvidence) => {
                "provider_no_extraction_evidence"
            }
            Self::Provider(TerraProbeProviderError::RequestPolicy(_)) => "provider_request_policy",
            Self::Provider(TerraProbeProviderError::RequestContract(_)) => {
                "provider_request_contract"
            }
            Self::Provider(TerraProbeProviderError::RequestEncoding) => "provider_request_encoding",
            Self::Provider(TerraProbeProviderError::ActionProposalLoop) => {
                "provider_action_proposal_loop"
            }
            Self::Provider(TerraProbeProviderError::NoProgress) => "provider_no_progress",
            Self::Proposal { .. } => "proposal_contract",
            Self::Engine(_) => "native_engine",
            Self::Verification => "fresh_snapshot_verification",
            Self::PostFirstActionVerification { .. } => "post_first_action_verification",
            Self::PostSecondActionVerification { .. } => "post_second_action_verification",
            Self::Metrics => "metrics",
            Self::Output => "output",
        }
    }

    const fn engine_reason_label(self) -> &'static str {
        match self {
            Self::Engine(reason) => reason,
            _ => "not_applicable",
        }
    }

    const fn action_step(self) -> u8 {
        match self {
            Self::Proposal { step, .. } => step,
            Self::PostFirstActionVerification { .. } => 1,
            Self::PostSecondActionVerification { .. } => 2,
            _ => 0,
        }
    }

    const fn verification_reason_label(self) -> &'static str {
        use zephium_agent_controller::TerraProbeActionBridgeError;
        let error = match self {
            Self::Proposal {
                error: Some(error), ..
            }
            | Self::PostFirstActionVerification { error, .. }
            | Self::PostSecondActionVerification { error, .. } => error,
            _ => return "not_applicable",
        };
        match error {
            TerraProbeActionBridgeError::UnexpectedTool(_) => "unexpected_tool",
            TerraProbeActionBridgeError::ActionCount => "action_count",
            TerraProbeActionBridgeError::StateUpdate => "state_update",
            TerraProbeActionBridgeError::Qualification(error) => error.diagnostic_code(),
        }
    }

    const fn tool_kind_label(self) -> &'static str {
        use zephium_agent_controller::TerraProbeActionBridgeError;
        use zephium_agentic::AgentBrowserToolKind;

        let Self::Proposal {
            error: Some(TerraProbeActionBridgeError::UnexpectedTool(kind)),
            ..
        } = self
        else {
            return "not_applicable";
        };
        match kind {
            AgentBrowserToolKind::Navigate => "navigate",
            AgentBrowserToolKind::Back => "back",
            AgentBrowserToolKind::Forward => "forward",
            AgentBrowserToolKind::Reload => "reload",
            AgentBrowserToolKind::Snapshot => "snapshot",
            AgentBrowserToolKind::Locate => "locate",
            AgentBrowserToolKind::Act => "act",
            AgentBrowserToolKind::FillForm => "fill_form",
            AgentBrowserToolKind::Wait => "wait",
            AgentBrowserToolKind::Read => "read",
            AgentBrowserToolKind::Extract => "extract",
            AgentBrowserToolKind::Screenshot => "screenshot",
            AgentBrowserToolKind::ShowForHuman => "show_for_human",
            AgentBrowserToolKind::ResumeAfterHuman => "resume_after_human",
        }
    }

    const fn wait_label(self) -> &'static str {
        use zephium_agentic::SemanticWaitCondition;

        let wait = match self {
            Self::PostFirstActionVerification { wait, .. }
            | Self::PostSecondActionVerification { wait, .. } => wait,
            _ => return "not_applicable",
        };
        match wait {
            SemanticWaitCondition::Immediate => "immediate",
            SemanticWaitCondition::NavigationCommitted => "navigation_committed",
            SemanticWaitCondition::DocumentReady => "document_ready",
            SemanticWaitCondition::TargetState { .. } => "target_state",
            SemanticWaitCondition::UrlChanged => "url_changed",
            SemanticWaitCondition::TitleChanged => "title_changed",
            SemanticWaitCondition::Dialog(_) => "dialog",
            SemanticWaitCondition::SemanticChange => "semantic_change",
            SemanticWaitCondition::MutationQuiet(_) => "mutation_quiet",
            SemanticWaitCondition::ScrollPositionChanged => "scroll_position_changed",
        }
    }

    const fn settle_millis(self) -> u32 {
        match self {
            Self::PostFirstActionVerification { settle_millis, .. }
            | Self::PostSecondActionVerification { settle_millis, .. } => settle_millis,
            _ => 0,
        }
    }

    const fn encoding_reason_label(self) -> &'static str {
        use zephium_agent_controller::TerraProbeProviderError;
        use zephium_agentic::SemanticModelEncodingError;

        let error = match self {
            Self::Provider(TerraProbeProviderError::InitialEncoding(error))
            | Self::Provider(TerraProbeProviderError::ReadEncoding(error))
            | Self::Provider(TerraProbeProviderError::DiffEncoding(error))
            | Self::Provider(TerraProbeProviderError::LocateEncoding(error)) => error,
            _ => return "not_applicable",
        };
        match error {
            SemanticModelEncodingError::Budget => "budget",
            SemanticModelEncodingError::OutputLimit => "output_limit",
            SemanticModelEncodingError::Invariant => "invariant",
            SemanticModelEncodingError::TokenCounter(_) => "token_counter",
            SemanticModelEncodingError::TokenQuality => "token_quality",
            SemanticModelEncodingError::TokenizerRevisionMismatch => "tokenizer_revision",
            SemanticModelEncodingError::TokenLimit => "token_limit",
        }
    }

    const fn protocol_event_label(self) -> &'static str {
        use zephium_agent_controller::TerraProbeProviderError;
        use zephium_agentic::AgentProviderProtocolEvent;

        let Self::Provider(TerraProbeProviderError::ModelProtocol(_, event)) = self else {
            return "not_applicable";
        };
        match event {
            Some(AgentProviderProtocolEvent::OpenAiCreated) => "openai_created",
            Some(AgentProviderProtocolEvent::OpenAiInProgress) => "openai_in_progress",
            Some(AgentProviderProtocolEvent::OpenAiOutputItemAdded) => "openai_output_item_added",
            Some(AgentProviderProtocolEvent::OpenAiOutputItemDone) => "openai_output_item_done",
            Some(AgentProviderProtocolEvent::OpenAiContentPart) => "openai_content_part",
            Some(AgentProviderProtocolEvent::OpenAiText) => "openai_text",
            Some(AgentProviderProtocolEvent::OpenAiToolArgumentsDelta) => {
                "openai_tool_arguments_delta"
            }
            Some(AgentProviderProtocolEvent::OpenAiToolArgumentsDone) => {
                "openai_tool_arguments_done"
            }
            Some(AgentProviderProtocolEvent::OpenAiTerminal) => "openai_terminal",
            Some(AgentProviderProtocolEvent::OpenAiError) => "openai_error",
            Some(AgentProviderProtocolEvent::OpenAiDone) => "openai_done",
            Some(AgentProviderProtocolEvent::OpenAiUnknown) => "openai_unknown",
            None => "unavailable",
        }
    }
}

#[cfg(target_os = "macos")]
const fn action_backend_label(
    backend: zephium_agentic::SemanticActionExecutionBackend,
) -> &'static str {
    match backend {
        zephium_agentic::SemanticActionExecutionBackend::FixedSemanticRecipe => {
            "fixed_semantic_recipe"
        }
        zephium_agentic::SemanticActionExecutionBackend::PageWorldCompatibilityFill => {
            "page_world_compatibility_fill"
        }
        zephium_agentic::SemanticActionExecutionBackend::EngineNativeInput => "engine_native_input",
        zephium_agentic::SemanticActionExecutionBackend::InProcessAccessibility => {
            "in_process_accessibility"
        }
    }
}

#[cfg(target_os = "macos")]
fn run_fixed_click() -> Result<(), ProbeFailure> {
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zephium_agent_controller::{
        run_initial_terra_probe, TerraControllerClock, TerraControllerRunInput,
        TerraControllerTurnInput, TerraProbeActionBridge,
    };
    use zephium_agent_provider_transport::{
        load_macos_probe_openai_credential, AgentProviderTransportConfig,
    };
    use zephium_agentic::{AgentPolicyInstant, SemanticActionExecutionInstant};

    struct Clock(AgentPolicyInstant);
    impl TerraControllerClock for Clock {
        fn now(
            &self,
        ) -> Result<AgentPolicyInstant, zephium_agent_controller::TerraControllerClockError>
        {
            Ok(self.0)
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| ProbeFailure::Runtime)?;
    let mut credential =
        Some(load_macos_probe_openai_credential().map_err(|_| ProbeFailure::Keychain)?);
    let mut bridge = None;
    let mut metrics = None;
    let mut callback_failure = None;
    let started = Instant::now();
    let terminal_result = zephium_engine::run_macos_agentic_semantic_model_click_probe(
        |observation, authority| {
            let (manifest, lease, account, request, frame, invocation, generation, now) =
                authority.into_parts();
            let turn = TerraControllerTurnInput::try_new(
                account,
                request,
                frame,
                invocation,
                generation,
                "Return exactly one act action: click the control named Primary semantic action; effect=local_write; wait=target_state expanded true; verification=target_state expanded true; use a bounded settle budget."
                    .to_owned(),
            )
            .map_err(|_| {
                callback_failure = Some(ProbeFailure::Authority);
            })?;
            let ids = probe_controller_ids().map_err(|_| {
                callback_failure = Some(ProbeFailure::Authority);
            })?;
            let input = TerraControllerRunInput::try_new(
                manifest,
                lease,
                turn,
                ids,
                Arc::new(Clock(now)),
                Instant::now()
                    .checked_add(Duration::from_secs(120))
                    .ok_or_else(|| {
                        callback_failure = Some(ProbeFailure::Authority);
                    })?,
            )
            .map_err(|_| {
                callback_failure = Some(ProbeFailure::Authority);
            })?;
            let key = credential.take().ok_or_else(|| {
                callback_failure = Some(ProbeFailure::Keychain);
            })?;
            let provider_started = Instant::now();
            let turn = runtime
                .block_on(run_initial_terra_probe(
                    input,
                    AgentProviderTransportConfig::STANDARD,
                    key,
                    observation,
                ))
                .map_err(|error| {
                    callback_failure = Some(ProbeFailure::Provider(error));
                })?;
            let provider_elapsed = provider_started.elapsed();
            let receipt = turn.receipt();
            let input = turn.input();
            let mut action = TerraProbeActionBridge::try_prepare(
                turn.into_tool_turn(),
                observation,
                1,
                1,
                SemanticActionExecutionInstant::from_millis(10_000),
            )
            .map_err(|error| {
                callback_failure = Some(ProbeFailure::Proposal {
                    step: 1,
                    error: Some(error),
                });
            })?;
            let request = action.take_native_request().map_err(|error| {
                callback_failure = Some(ProbeFailure::Proposal {
                    step: 1,
                    error: Some(error),
                });
            })?;
            bridge = Some(action);
            metrics = Some((receipt, input, provider_elapsed));
            Ok(request)
        },
    );
    let terminal =
        terminal_result.map_err(|error| callback_failure.unwrap_or(ProbeFailure::Engine(error)))?;
    let (settlement, snapshot, observed_at) = terminal.into_parts();
    let report = bridge
        .ok_or(ProbeFailure::Proposal {
            step: 1,
            error: None,
        })?
        .settle_and_verify(settlement, &snapshot, observed_at)
        .map_err(|_| ProbeFailure::Verification)?;
    let _ = report.applied();
    let (receipt, input, provider_elapsed) = metrics.ok_or(ProbeFailure::Metrics)?;
    let input = input.metrics();
    let structured = input
        .structured_input_tokens()
        .map_or(0, |count| count.tokens());
    let semantic = input
        .semantic_payload_tokens()
        .map_or(0, |count| count.tokens());
    let provider_elapsed = provider_elapsed.as_millis();
    let elapsed = started.elapsed().as_millis();
    use std::io::Write as _;
    let mut output = std::io::stdout().lock();
    writeln!(
        output,
        "macos-terra-agentic-probe: passed; model=gpt-5.6-terra; tool=act-click; effect=local_write; verified=true; input_tokens={}; output_tokens={}; total_tokens={}; structured_tokens={structured}; semantic_tokens={semantic}; request_bytes={}; semantic_bytes={}; charged_micro_usd={}; provider_elapsed_ms={provider_elapsed}; elapsed_ms={elapsed}; content=redacted",
        receipt.input_tokens(), receipt.output_tokens(), receipt.input_tokens().saturating_add(receipt.output_tokens()), input.serialized_request_bytes(), input.semantic().disclosed_bytes(), receipt.cost_micro_usd(),
    ).map_err(|_| ProbeFailure::Output)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn probe_controller_ids() -> Result<zephium_agent_controller::TerraControllerIds, ProbeFailure> {
    use zephium_agent_controller::TerraControllerIds;
    use zephium_agentic::{
        AgentAuditDeliveryId, AgentAuditEventId, AgentModelCallId, AgentSupervisorAttemptId,
        AgentSupervisorCancellationId, AgentSupervisorId,
    };

    TerraControllerIds::try_new(
        AgentSupervisorId::new(1).ok_or(ProbeFailure::Authority)?,
        AgentSupervisorAttemptId::new(1).ok_or(ProbeFailure::Authority)?,
        AgentSupervisorCancellationId::new(1).ok_or(ProbeFailure::Authority)?,
        AgentModelCallId::new(1).ok_or(ProbeFailure::Authority)?,
        [
            AgentAuditEventId::new(1).ok_or(ProbeFailure::Authority)?,
            AgentAuditEventId::new(2).ok_or(ProbeFailure::Authority)?,
            AgentAuditEventId::new(3).ok_or(ProbeFailure::Authority)?,
            AgentAuditEventId::new(4).ok_or(ProbeFailure::Authority)?,
        ],
        AgentAuditDeliveryId::new(1).ok_or(ProbeFailure::Authority)?,
    )
    .map_err(|_| ProbeFailure::Authority)
}

#[cfg(target_os = "macos")]
fn run_public_wikipedia_fill() -> Result<(), ProbeFailure> {
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zephium_agent_controller::{
        run_initial_terra_probe, TerraControllerClock, TerraControllerRunInput,
        TerraControllerTurnInput, TerraProbeActionBridge,
    };
    use zephium_agent_provider_transport::{
        load_macos_probe_openai_credential, AgentProviderTransportConfig,
    };
    use zephium_agentic::{AgentPolicyInstant, SemanticActionExecutionInstant};

    struct Clock(AgentPolicyInstant);
    impl TerraControllerClock for Clock {
        fn now(
            &self,
        ) -> Result<AgentPolicyInstant, zephium_agent_controller::TerraControllerClockError>
        {
            Ok(self.0)
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| ProbeFailure::Runtime)?;
    let mut credential =
        Some(load_macos_probe_openai_credential().map_err(|_| ProbeFailure::Keychain)?);
    let mut bridge = None;
    let mut metrics = None;
    let mut callback_failure = None;
    let started = Instant::now();
    let terminal_result = zephium_engine::run_macos_agentic_semantic_model_public_fill_probe(
        |observation, authority| {
            let (manifest, lease, account, request, frame, invocation, generation, now) =
                authority.into_parts();
            let turn = TerraControllerTurnInput::try_new(
                account,
                request,
                frame,
                invocation,
                generation,
                "On the current Wikipedia public discovery page, fill its search input with exactly Zephium browser. Return exactly one act action and do not submit or navigate. Use effect=local_write, wait=mutation_quiet 100 ms, verification=target_value_matches_input, and settle_budget=1000 ms."
                    .to_owned(),
            )
            .map_err(|_| {
                callback_failure = Some(ProbeFailure::Authority);
            })?;
            let input = TerraControllerRunInput::try_new(
                manifest,
                lease,
                turn,
                probe_controller_ids().map_err(|_| {
                    callback_failure = Some(ProbeFailure::Authority);
                })?,
                Arc::new(Clock(now)),
                Instant::now()
                    .checked_add(Duration::from_secs(120))
                    .ok_or_else(|| {
                        callback_failure = Some(ProbeFailure::Authority);
                    })?,
            )
            .map_err(|_| {
                callback_failure = Some(ProbeFailure::Authority);
            })?;
            let key = credential.take().ok_or_else(|| {
                callback_failure = Some(ProbeFailure::Keychain);
            })?;
            let provider_started = Instant::now();
            let turn = runtime
                .block_on(run_initial_terra_probe(
                    input,
                    AgentProviderTransportConfig::STANDARD,
                    key,
                    observation,
                ))
                .map_err(|error| {
                    callback_failure = Some(ProbeFailure::Provider(error));
                })?;
            let provider_elapsed = provider_started.elapsed();
            let receipt = turn.receipt();
            let input = turn.input();
            let mut action = TerraProbeActionBridge::try_prepare(
                turn.into_tool_turn(),
                observation,
                1,
                1,
                SemanticActionExecutionInstant::from_millis(10_000),
            )
            .map_err(|error| {
                callback_failure = Some(ProbeFailure::Proposal {
                    step: 1,
                    error: Some(error),
                });
            })?;
            let request = action.take_native_request().map_err(|error| {
                callback_failure = Some(ProbeFailure::Proposal {
                    step: 1,
                    error: Some(error),
                });
            })?;
            bridge = Some(action);
            metrics = Some((receipt, input, provider_elapsed));
            Ok(request)
        },
    );
    let terminal =
        terminal_result.map_err(|error| callback_failure.unwrap_or(ProbeFailure::Engine(error)))?;
    let (settlement, snapshot, observed_at) = terminal.into_parts();
    let action = bridge.ok_or(ProbeFailure::Proposal {
        step: 1,
        error: None,
    })?;
    let wait = action.wait();
    let settle_millis = action.settle_millis();
    let report = action
        .settle_and_verify(settlement, &snapshot, observed_at)
        .map_err(|error| ProbeFailure::PostFirstActionVerification {
            error,
            wait,
            settle_millis,
        })?;
    let backend = action_backend_label(report.applied().backend());
    let (receipt, input, provider_elapsed) = metrics.ok_or(ProbeFailure::Metrics)?;
    let input = input.metrics();
    let provider_elapsed = provider_elapsed.as_millis();
    let elapsed = started.elapsed().as_millis();
    use std::io::Write as _;
    writeln!(
        std::io::stdout().lock(),
        "macos-terra-agentic-probe: passed; workflow=public-wikipedia-search-fill; model=gpt-5.6-terra; turns=1; verified_actions=1; backend={backend}; profile=ephemeral; extensions=absent; presentation=hidden; focus_theft=false; input_tokens={}; output_tokens={}; total_tokens={}; request_bytes={}; semantic_bytes={}; charged_micro_usd={}; provider_elapsed_ms={provider_elapsed}; elapsed_ms={elapsed}; content=redacted",
        receipt.input_tokens(),
        receipt.output_tokens(),
        receipt
            .input_tokens()
            .saturating_add(receipt.output_tokens()),
        input.serialized_request_bytes(),
        input.semantic().disclosed_bytes(),
        receipt.cost_micro_usd(),
    )
    .map_err(|_| ProbeFailure::Output)?;
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum TwoActionScenario {
    FixedClickFill,
    PublicFillSelect,
    PublicFillLocateSelect,
}

#[cfg(target_os = "macos")]
impl TwoActionScenario {
    const fn objective(self) -> &'static str {
        match self {
            Self::FixedClickFill => {
                "Complete exactly two local actions in order. First click Primary semantic action with target_state expanded true verification. After the verified tool result, fill Semantic fill text with Zephium model continuation using target_value_matches_input verification. Return exactly one act action per turn and use bounded settle budgets."
            }
            Self::PublicFillSelect => {
                "On the current Wikipedia public discovery page, complete exactly two local actions in order without submitting or navigating. First fill its search input with exactly Zephium browser using wait=mutation_quiet 100 ms, verification=target_value_matches_input, and settle_budget=1000 ms. After the verified tool result, select the Deutsch option in the language selector using wait=mutation_quiet 100 ms, verification=target_selection_matches_option, and settle_budget=1000 ms. Return exactly one act action per turn with effect=local_write."
            }
            Self::PublicFillLocateSelect => {
                "On the current Wikipedia public discovery page, complete exactly two local actions in order without submitting or navigating. First fill its search input with exactly Zephium browser using wait=mutation_quiet 100 ms, verification=target_value_matches_input, and settle_budget=1000 ms. After the verified tool result, call locate exactly once with query Deutsch and scope all before selecting the returned exact option ref in the language selector using wait=mutation_quiet 100 ms, verification=target_selection_matches_option, and settle_budget=1000 ms. Return exactly one act action per action turn with effect=local_write."
            }
        }
    }

    const fn engine_scenario(self) -> zephium_engine::MacosAgenticSemanticTwoActionScenario {
        match self {
            Self::FixedClickFill => {
                zephium_engine::MacosAgenticSemanticTwoActionScenario::FixedClickFill
            }
            Self::PublicFillSelect => {
                zephium_engine::MacosAgenticSemanticTwoActionScenario::PublicFillSelect
            }
            Self::PublicFillLocateSelect => {
                zephium_engine::MacosAgenticSemanticTwoActionScenario::PublicFillSelect
            }
        }
    }

    const fn workflow_label(self) -> &'static str {
        match self {
            Self::FixedClickFill => "verified-click-then-fill",
            Self::PublicFillSelect => "public-wikipedia-fill-then-select",
            Self::PublicFillLocateSelect => "public-wikipedia-fill-locate-then-select",
        }
    }

    const fn accepts_model_turns(self, turns: usize) -> bool {
        match self {
            Self::FixedClickFill => turns == 2,
            Self::PublicFillSelect => turns == 2 || turns == 3,
            Self::PublicFillLocateSelect => turns == 3,
        }
    }

    const fn permits_locate(self) -> bool {
        matches!(self, Self::PublicFillSelect | Self::PublicFillLocateSelect)
    }
}

#[cfg(target_os = "macos")]
fn run_luna_inspectable_suite() -> Result<(), ProbeFailure> {
    // AppKit has process-global launch and activation state. Each qualification
    // run therefore gets a fresh process, matching how the standalone probe is
    // invoked while avoiding false failures from attempting to relaunch the
    // same NSApplication during a repeatability sweep.
    let executable = std::env::current_exe().map_err(|_| ProbeFailure::SuiteLaunch)?;
    for argument in [
        "--live-public-luna-form-inspectable",
        "--live-public-luna-form-inspectable",
        "--live-public-luna-form-inspectable",
        "--live-public-luna-form-locate-inspectable",
        "--live-public-luna-form-locate-inspectable",
        "--live-public-luna-form-locate-inspectable",
    ] {
        let status = std::process::Command::new(&executable)
            .arg(argument)
            .status()
            .map_err(|_| ProbeFailure::SuiteLaunch)?;
        if !status.success() {
            return Err(ProbeFailure::SuiteChild);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct PreparedSearchProgress {
    prepared: bool,
}

#[cfg(target_os = "macos")]
impl PreparedSearchProgress {
    fn observe(&mut self, initial: bool, final_query: bool, language: bool) -> bool {
        self.prepared |= initial && language;
        self.prepared && final_query && language
    }
}

#[cfg(all(test, target_os = "macos"))]
mod workflow_tests {
    use super::PreparedSearchProgress;

    #[test]
    fn preparation_accepts_either_order_but_requires_the_refinement_after_both() {
        for first in [(true, false, false), (false, false, true)] {
            let mut progress = PreparedSearchProgress::default();
            assert!(!progress.observe(first.0, first.1, first.2));
            assert!(!progress.observe(true, false, true));
            assert!(progress.observe(false, true, true));
        }
    }

    #[test]
    fn partial_milestones_and_early_refinement_cannot_certify_completion() {
        let mut progress = PreparedSearchProgress::default();
        for observation in [
            (true, false, false),
            (false, true, false),
            (false, true, true),
            (false, false, false),
            (false, true, true),
        ] {
            assert!(!progress.observe(observation.0, observation.1, observation.2));
        }
        assert!(!progress.observe(true, false, true));
        assert!(!progress.observe(false, true, false));
        assert!(!progress.observe(false, false, true));
        assert!(progress.observe(false, true, true));
    }
}

#[cfg(target_os = "macos")]
fn run_variable_workflow() -> Result<(), ProbeFailure> {
    use std::cell::{Cell, RefCell};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use zephium_agent_controller::{
        AgentBrowserModel, AgentBrowserRetention, AgentBrowserSession, TerraControllerClock,
        TerraControllerRunInput, TerraControllerTurnInput,
    };
    use zephium_agent_provider_transport::{
        load_macos_probe_openai_credential, AgentProviderTransportConfig,
    };
    use zephium_agentic::{
        AgentEffectAssessment, AgentEffectSettlement, AgentPolicyInstant,
        SemanticActionExecutionInstant, SemanticActionKind, SemanticEffectClass,
        SemanticEffectProofKind, SemanticObservation, SemanticRole, SemanticState,
        SemanticValueSummary,
    };

    struct Clock {
        started: Instant,
        base: AgentPolicyInstant,
    }
    impl TerraControllerClock for Clock {
        fn now(
            &self,
        ) -> Result<AgentPolicyInstant, zephium_agent_controller::TerraControllerClockError>
        {
            let elapsed = u64::try_from(self.started.elapsed().as_millis())
                .map_err(|_| zephium_agent_controller::TerraControllerClockError::Invalid)?;
            let millis = self
                .base
                .millis()
                .checked_add(elapsed)
                .ok_or(zephium_agent_controller::TerraControllerClockError::Invalid)?;
            Ok(AgentPolicyInstant::from_millis(millis))
        }
    }
    fn state(observation: &SemanticObservation) -> (bool, bool, bool) {
        let mut initial = false;
        let mut final_query = false;
        let mut language = false;
        for node in observation.frames().iter().flat_map(|frame| frame.nodes()) {
            if matches!(node.role(), SemanticRole::Searchbox | SemanticRole::Textbox) {
                if let Some(SemanticValueSummary::Text(value)) = node.value() {
                    let preview = value.preview();
                    initial |= !preview.truncated()
                        && preview.source_bytes() == "Zephium browser".len()
                        && preview.text() == "Zephium browser";
                    final_query |= !preview.truncated()
                        && preview.source_bytes() == "Zephium open source browser".len()
                        && preview.text() == "Zephium open source browser";
                }
            }
            language |= node.role() == SemanticRole::Option
                && node.name().is_some_and(|name| name.as_str() == "Deutsch")
                && node.states().contains(SemanticState::Selected);
        }
        (initial, final_query, language)
    }
    struct Metric {
        model: zephium_agentic::AgentModelCallReceipt,
        input: zephium_agentic::AgentProviderInputMetricReceipt,
        elapsed: Duration,
        wall_elapsed: Duration,
    }
    struct State {
        session: Option<AgentBrowserSession>,
        metrics: Vec<Metric>,
        verified: u8,
        terminal: Option<zephium_agent_controller::AgentBrowserSessionTerminal>,
        progress: PreparedSearchProgress,
        complete: bool,
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| ProbeFailure::Runtime)?;
    let workflow = RefCell::new(State {
        session: None,
        metrics: Vec::with_capacity(8),
        verified: 0,
        terminal: None,
        progress: PreparedSearchProgress::default(),
        complete: false,
    });
    let failure = Cell::new(None);
    let started = Instant::now();
    let result = zephium_engine::run_macos_agentic_semantic_model_workflow_probe(
        |observation, authority, automation| {
            let run = || -> Result<_, ProbeFailure> {
                let (manifest, lease, account, request, frame, invocation, generation, now) =
                    authority.into_parts();
                let clock = Arc::new(Clock {
                    started: Instant::now(),
                    base: now,
                });
                let objective = "Prepare a public Wikipedia search without submitting or navigating. Fill the search with exactly Zephium browser and choose Deutsch in the search language selector, in either order. Once both are verified, refine the search text to exactly Zephium open source browser. Use one local_write act action per turn. Locate option references when needed. For each action use mutation_quiet=100 ms, settle_budget=1000 ms, and exact value or exact selected-option verification. Do not click links or submit. The host checks the exact milestones and stops when the final prepared search is verified.";
                let turn = TerraControllerTurnInput::try_new(
                    account,
                    request,
                    frame,
                    invocation,
                    generation,
                    objective.to_owned(),
                )
                .map_err(|_| ProbeFailure::Authority)?;
                let input = TerraControllerRunInput::try_new_for_model(
                    manifest,
                    lease,
                    turn,
                    probe_controller_ids()?,
                    clock,
                    Instant::now()
                        .checked_add(Duration::from_secs(120))
                        .ok_or(ProbeFailure::Authority)?,
                    AgentBrowserModel::Luna,
                )
                .map_err(|_| ProbeFailure::Authority)?;
                let credential =
                    load_macos_probe_openai_credential().map_err(|_| ProbeFailure::Keychain)?;
                let mut workflow = workflow
                    .try_borrow_mut()
                    .map_err(|_| ProbeFailure::Authority)?;
                workflow.session = Some(
                    AgentBrowserSession::try_new(
                        input,
                        AgentProviderTransportConfig::STANDARD,
                        credential,
                        AgentBrowserModel::Luna,
                        AgentBrowserRetention::InspectablePublicData,
                    )
                    .map_err(ProbeFailure::Provider)?,
                );
                let State {
                    session, metrics, ..
                } = &mut *workflow;
                let session = session.as_mut().ok_or(ProbeFailure::Authority)?;
                let turn = runtime
                    .block_on(session.start_initial(observation))
                    .map_err(ProbeFailure::Provider)?;
                let frames = observation
                    .frames()
                    .iter()
                    .map(|frame| frame.frame().clone())
                    .collect::<Vec<_>>();
                let proposal = runtime
                    .block_on(session.next_action(
                        turn,
                        observation,
                        &frames,
                        |model, input, elapsed| {
                            metrics.push(Metric {
                                model,
                                input,
                                elapsed,
                                wall_elapsed: started.elapsed(),
                            });
                        },
                    ))
                    .map_err(ProbeFailure::Provider)?;
                if !matches!(
                    proposal.action().kind(),
                    SemanticActionKind::Fill | SemanticActionKind::Select
                ) {
                    return Err(ProbeFailure::Authority);
                }
                let assessment = AgentEffectAssessment::new(
                    proposal.action(),
                    proposal.action().frame().origin().clone(),
                    SemanticEffectClass::LocalWrite,
                );
                let native = session
                    .authorize_action(
                        proposal,
                        &assessment,
                        automation,
                        SemanticActionExecutionInstant::from_millis(10_000),
                    )
                    .map_err(ProbeFailure::Provider)?;
                writeln!(std::io::stdout().lock(), "browser-workflow-action: step=1; kind={:?}; state=authorized; content=redacted", native.kind()).map_err(|_| ProbeFailure::Output)?;
                Ok(native)
            };
            run().map_err(|error| failure.set(Some(error)))
        },
        |baseline, native, current, observed_at, automation| {
            let run = || -> Result<_, ProbeFailure> {
                let mut workflow = workflow
                    .try_borrow_mut()
                    .map_err(|_| ProbeFailure::Authority)?;
                let (receipt, transition) = workflow
                    .session
                    .as_mut()
                    .ok_or(ProbeFailure::Authority)?
                    .settle_action(native, baseline, current, observed_at)
                    .map_err(ProbeFailure::Provider)?;
                if !matches!(
                    receipt.settlement(),
                    AgentEffectSettlement::Verified(
                        SemanticEffectProofKind::ExactTargetValue
                            | SemanticEffectProofKind::ExactSelection
                    )
                ) {
                    return Err(ProbeFailure::Verification);
                }
                workflow.verified = workflow
                    .verified
                    .checked_add(1)
                    .ok_or(ProbeFailure::Metrics)?;
                let (initial, final_query, language) = state(current);
                writeln!(std::io::stdout().lock(), "browser-workflow-effect: step={}; state=verified; initial_query={}; final_query={}; language_selected={}; content=redacted",
                    workflow.verified, initial, final_query, language).map_err(|_| ProbeFailure::Output)?;
                if workflow.progress.observe(initial, final_query, language) {
                    drop(transition);
                    let session = workflow.session.take().ok_or(ProbeFailure::Authority)?;
                    match session.try_finish() {
                        Ok(terminal) => workflow.terminal = Some(terminal),
                        Err(refusal) => {
                            let error = refusal.error();
                            workflow.session = Some(refusal.into_session());
                            return Err(ProbeFailure::Provider(error));
                        }
                    }
                    workflow.complete = true;
                    return Ok(None);
                }
                let turn = runtime
                    .block_on(
                        workflow
                            .session
                            .as_mut()
                            .ok_or(ProbeFailure::Authority)?
                            .continue_after_verified_action(transition),
                    )
                    .map_err(ProbeFailure::Provider)?;
                let frames = current
                    .frames()
                    .iter()
                    .map(|frame| frame.frame().clone())
                    .collect::<Vec<_>>();
                let State {
                    session, metrics, ..
                } = &mut *workflow;
                let session = session.as_mut().ok_or(ProbeFailure::Authority)?;
                let proposal = runtime
                    .block_on(session.next_action(
                        turn,
                        current,
                        &frames,
                        |model, input, elapsed| {
                            metrics.push(Metric {
                                model,
                                input,
                                elapsed,
                                wall_elapsed: started.elapsed(),
                            });
                        },
                    ))
                    .map_err(ProbeFailure::Provider)?;
                if !matches!(
                    proposal.action().kind(),
                    SemanticActionKind::Fill | SemanticActionKind::Select
                ) {
                    return Err(ProbeFailure::Authority);
                }
                let assessment = AgentEffectAssessment::new(
                    proposal.action(),
                    proposal.action().frame().origin().clone(),
                    SemanticEffectClass::LocalWrite,
                );
                let native = session
                    .authorize_action(
                        proposal,
                        &assessment,
                        automation,
                        SemanticActionExecutionInstant::from_millis(10_000),
                    )
                    .map_err(ProbeFailure::Provider)?;
                writeln!(
                    std::io::stdout().lock(),
                    "browser-workflow-action: kind={:?}; state=authorized; content=redacted",
                    native.kind()
                )
                .map_err(|_| ProbeFailure::Output)?;
                Ok(Some(native))
            };
            run().map_err(|error| failure.set(Some(error)))
        },
    );
    let mut workflow = workflow.into_inner();
    use std::io::Write as _;
    for (ordinal, metric) in workflow.metrics.iter().enumerate() {
        let metrics = metric.input.metrics();
        writeln!(std::io::stdout().lock(), "browser-workflow-turn: turn={}; input_tokens={}; output_tokens={}; request_bytes={}; semantic_bytes={}; provider_elapsed_ms={}; wall_elapsed_ms={}; charged_micro_usd={}; content=redacted",
            ordinal + 1, metric.model.input_tokens(), metric.model.output_tokens(), metrics.serialized_request_bytes(), metrics.semantic().disclosed_bytes(), metric.elapsed.as_millis(), metric.wall_elapsed.as_millis(), metric.model.cost_micro_usd()).map_err(|_| ProbeFailure::Output)?;
    }
    // Even a refused workflow retains its session until native teardown returned.
    let recovery = workflow.session.take().map(AgentBrowserSession::try_finish);
    if let Err(error) = result {
        writeln!(std::io::stdout().lock(), "browser-workflow: refused; turns={}; verified_actions={}; provider_policy_drain={}; elapsed_ms={}; content=redacted",
            workflow.metrics.len(), workflow.verified,
            if recovery.as_ref().is_none_or(Result::is_ok) { "drained" } else { "recovery_required" },
            started.elapsed().as_millis()).map_err(|_| ProbeFailure::Output)?;
        return Err(failure.get().unwrap_or(ProbeFailure::Engine(error)));
    }
    if !workflow.complete
        || workflow.terminal.is_none()
        || workflow.verified < 3
        || workflow.metrics.len() > 8
    {
        return Err(ProbeFailure::Metrics);
    }
    let terminal = workflow.terminal.as_ref().ok_or(ProbeFailure::Metrics)?;
    if terminal.model_receipts().len() != workflow.metrics.len() {
        return Err(ProbeFailure::Metrics);
    }
    writeln!(std::io::stdout().lock(), "browser-workflow: passed; model=gpt-5.6-luna; provider_storage=retained-public-probe; turns={}; verified_actions={}; real_policy=true; focus_theft=0; background_scheduling=cpu_throttled; native_teardown=drained; elapsed_ms={}; content=redacted",
        workflow.metrics.len(), workflow.verified, started.elapsed().as_millis()).map_err(|_| ProbeFailure::Output)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_two_action(
    scenario: TwoActionScenario,
    model: ProbeModel,
    retention: ProbeRetention,
) -> Result<(), ProbeFailure> {
    use std::cell::{Cell, RefCell};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zephium_agent_controller::{
        TerraControllerClock, TerraControllerRunInput, TerraControllerTurnInput,
        TerraProbeActionBridge, TerraProbeSession,
    };
    use zephium_agent_provider_transport::{
        load_macos_probe_openai_credential, AgentProviderCredential, AgentProviderTransportConfig,
    };
    use zephium_agentic::{
        AgentModelCallReceipt, AgentPolicyInstant, AgentProviderInputMetricReceipt,
        SemanticActionExecutionInstant,
    };

    struct Clock(AgentPolicyInstant);
    impl TerraControllerClock for Clock {
        fn now(
            &self,
        ) -> Result<AgentPolicyInstant, zephium_agent_controller::TerraControllerClockError>
        {
            Ok(self.0)
        }
    }

    struct TurnMetric {
        receipt: AgentModelCallReceipt,
        input: AgentProviderInputMetricReceipt,
        provider_elapsed: Duration,
    }

    struct WorkflowState {
        credential: Option<AgentProviderCredential>,
        session: Option<TerraProbeSession>,
        bridge: Option<TerraProbeActionBridge>,
        metrics: Vec<TurnMetric>,
        backends: Vec<zephium_agentic::SemanticActionExecutionBackend>,
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| ProbeFailure::Runtime)?;
    let state = RefCell::new(WorkflowState {
        credential: Some(load_macos_probe_openai_credential().map_err(|_| ProbeFailure::Keychain)?),
        session: None,
        bridge: None,
        metrics: Vec::with_capacity(3),
        backends: Vec::with_capacity(2),
    });
    let callback_failure = Cell::new(None);
    let started = Instant::now();
    let result = zephium_engine::run_macos_agentic_semantic_model_two_action_probe(
        scenario.engine_scenario(),
        |observation, authority| {
            let mut state = state.try_borrow_mut().map_err(|_| {
                callback_failure.set(Some(ProbeFailure::Authority));
            })?;
            let (manifest, lease, account, request, frame, invocation, generation, now) =
                authority.into_parts();
            let turn = TerraControllerTurnInput::try_new(
                account,
                request,
                frame,
                invocation,
                generation,
                scenario.objective().to_owned(),
            )
            .map_err(|_| {
                callback_failure.set(Some(ProbeFailure::Authority));
            })?;
            let input = TerraControllerRunInput::try_new_for_probe_model(
                manifest,
                lease,
                turn,
                probe_controller_ids().map_err(|_| {
                    callback_failure.set(Some(ProbeFailure::Authority));
                })?,
                Arc::new(Clock(now)),
                Instant::now()
                    .checked_add(Duration::from_secs(120))
                    .ok_or_else(|| {
                        callback_failure.set(Some(ProbeFailure::Authority));
                    })?,
                model,
            )
            .map_err(|_| {
                callback_failure.set(Some(ProbeFailure::Authority));
            })?;
            let credential = state.credential.take().ok_or_else(|| {
                callback_failure.set(Some(ProbeFailure::Keychain));
            })?;
            let provider_started = Instant::now();
            let (session, provider_turn) = runtime
                .block_on(TerraProbeSession::start_with_model(
                    input,
                    AgentProviderTransportConfig::STANDARD,
                    credential,
                    observation,
                    model,
                    retention,
                ))
                .map_err(|error| {
                    callback_failure.set(Some(ProbeFailure::Provider(error)));
                })?;
            let provider_elapsed = provider_started.elapsed();
            let receipt = provider_turn.receipt();
            let input = provider_turn.input();
            let mut bridge = TerraProbeActionBridge::try_prepare(
                provider_turn.into_tool_turn(),
                observation,
                1,
                1,
                SemanticActionExecutionInstant::from_millis(10_000),
            )
            .map_err(|error| {
                callback_failure.set(Some(ProbeFailure::Proposal {
                    step: 1,
                    error: Some(error),
                }));
            })?;
            let request = bridge.take_native_request().map_err(|error| {
                callback_failure.set(Some(ProbeFailure::Proposal {
                    step: 1,
                    error: Some(error),
                }));
            })?;
            state.session = Some(session);
            state.bridge = Some(bridge);
            state.metrics.push(TurnMetric {
                receipt,
                input,
                provider_elapsed,
            });
            Ok(request)
        },
        |baseline, settlement, current, observed_at| {
            let mut state = state.try_borrow_mut().map_err(|_| {
                callback_failure.set(Some(ProbeFailure::Authority));
            })?;
            let bridge = state.bridge.take().ok_or_else(|| {
                callback_failure.set(Some(ProbeFailure::Proposal {
                    step: 1,
                    error: None,
                }));
            })?;
            let wait = bridge.wait();
            let settle_millis = bridge.settle_millis();
            let (report, transition) = bridge
                .settle_for_continuation(settlement, baseline, current, observed_at)
                .map_err(|error| {
                    callback_failure.set(Some(ProbeFailure::PostFirstActionVerification {
                        error,
                        wait,
                        settle_millis,
                    }));
                })?;
            state.backends.push(report.applied().backend());
            let provider_started = Instant::now();
            let provider_turn = runtime
                .block_on(
                    state
                        .session
                        .as_mut()
                        .ok_or_else(|| {
                            callback_failure.set(Some(ProbeFailure::Authority));
                        })?
                        .continue_after_verified_action(transition),
                )
                .map_err(|error| {
                    callback_failure.set(Some(ProbeFailure::Provider(error)));
                })?;
            let provider_elapsed = provider_started.elapsed();
            let receipt = provider_turn.receipt();
            let input = provider_turn.input();
            let tool_turn = provider_turn.into_tool_turn();
            state.metrics.push(TurnMetric {
                receipt,
                input,
                provider_elapsed,
            });
            let tool_turn = if scenario.permits_locate()
                && tool_turn.proposal().kind() == zephium_agentic::AgentBrowserToolKind::Locate
            {
                let provider_started = Instant::now();
                let provider_turn = runtime
                    .block_on(
                        state
                            .session
                            .as_mut()
                            .ok_or_else(|| {
                                callback_failure.set(Some(ProbeFailure::Authority));
                            })?
                            .continue_after_locate(tool_turn, current, 1),
                    )
                    .map_err(|error| {
                        callback_failure.set(Some(ProbeFailure::Provider(error)));
                    })?;
                let provider_elapsed = provider_started.elapsed();
                let receipt = provider_turn.receipt();
                let input = provider_turn.input();
                state.metrics.push(TurnMetric {
                    receipt,
                    input,
                    provider_elapsed,
                });
                provider_turn.into_tool_turn()
            } else {
                tool_turn
            };
            let mut bridge = TerraProbeActionBridge::try_prepare(
                tool_turn,
                current,
                2,
                2,
                SemanticActionExecutionInstant::from_millis(10_000),
            )
            .map_err(|error| {
                callback_failure.set(Some(ProbeFailure::Proposal {
                    step: 2,
                    error: Some(error),
                }));
            })?;
            let request = bridge.take_native_request().map_err(|error| {
                callback_failure.set(Some(ProbeFailure::Proposal {
                    step: 2,
                    error: Some(error),
                }));
            })?;
            state.bridge = Some(bridge);
            Ok(request)
        },
        |_baseline, settlement, current, observed_at| {
            let mut state = state.try_borrow_mut().map_err(|_| {
                callback_failure.set(Some(ProbeFailure::Authority));
            })?;
            let snapshot = current.frames().first().ok_or_else(|| {
                callback_failure.set(Some(ProbeFailure::Verification));
            })?;
            let bridge = state.bridge.take().ok_or_else(|| {
                callback_failure.set(Some(ProbeFailure::Proposal {
                    step: 2,
                    error: None,
                }));
            })?;
            let wait = bridge.wait();
            let settle_millis = bridge.settle_millis();
            let report = bridge
                .settle_and_verify(settlement, snapshot, observed_at)
                .map_err(|error| {
                    callback_failure.set(Some(ProbeFailure::PostSecondActionVerification {
                        error,
                        wait,
                        settle_millis,
                    }));
                })?;
            state.backends.push(report.applied().backend());
            state
                .session
                .take()
                .ok_or_else(|| {
                    callback_failure.set(Some(ProbeFailure::Authority));
                })?
                .finish()
                .map_err(|error| {
                    callback_failure.set(Some(ProbeFailure::Provider(error)));
                })?;
            Ok(())
        },
    );
    result.map_err(|error| {
        callback_failure
            .get()
            .unwrap_or(ProbeFailure::Engine(error))
    })?;
    let state = state.into_inner();
    if state.credential.is_some()
        || state.session.is_some()
        || state.bridge.is_some()
        || !scenario.accepts_model_turns(state.metrics.len())
        || state.backends.len() != 2
    {
        return Err(ProbeFailure::Metrics);
    }
    let [first_backend, second_backend] = state.backends.as_slice() else {
        return Err(ProbeFailure::Metrics);
    };
    let first_backend = action_backend_label(*first_backend);
    let second_backend = action_backend_label(*second_backend);
    let model_turns = state.metrics.len();
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut charged_micro_usd = 0_u64;
    let mut request_bytes = 0_u64;
    let mut semantic_bytes = 0_u64;
    let mut provider_elapsed_ms = 0_u128;
    let mut turn_input_tokens = Vec::with_capacity(model_turns);
    let mut turn_cached_input_tokens = Vec::with_capacity(model_turns);
    let mut turn_output_tokens = Vec::with_capacity(model_turns);
    let mut turn_reasoning_output_tokens = Vec::with_capacity(model_turns);
    let mut turn_request_bytes = Vec::with_capacity(model_turns);
    let mut turn_semantic_bytes = Vec::with_capacity(model_turns);
    let mut turn_semantic_token_measurements = Vec::with_capacity(model_turns);
    let mut turn_structured_input_measurements = Vec::with_capacity(model_turns);
    let mut turn_provider_elapsed_ms = Vec::with_capacity(model_turns);
    for metric in state.metrics {
        let attribution = metric.receipt.pricing_attribution();
        turn_input_tokens.push(metric.receipt.input_tokens());
        turn_cached_input_tokens.push(
            attribution
                .map(|attribution| attribution.cached_input_tokens())
                .unwrap_or(0),
        );
        turn_output_tokens.push(metric.receipt.output_tokens());
        turn_reasoning_output_tokens.push(
            attribution
                .map(|attribution| attribution.reasoning_output_tokens())
                .unwrap_or(0),
        );
        input_tokens = input_tokens
            .checked_add(metric.receipt.input_tokens())
            .ok_or(ProbeFailure::Metrics)?;
        output_tokens = output_tokens
            .checked_add(metric.receipt.output_tokens())
            .ok_or(ProbeFailure::Metrics)?;
        charged_micro_usd = charged_micro_usd
            .checked_add(metric.receipt.cost_micro_usd())
            .ok_or(ProbeFailure::Metrics)?;
        let input = metric.input.metrics();
        turn_request_bytes.push(input.serialized_request_bytes());
        turn_semantic_bytes.push(input.semantic().disclosed_bytes());
        turn_semantic_token_measurements.push(input.semantic_payload_tokens());
        turn_structured_input_measurements.push(input.structured_input_tokens());
        turn_provider_elapsed_ms.push(metric.provider_elapsed.as_millis());
        request_bytes = request_bytes
            .checked_add(u64::from(input.serialized_request_bytes()))
            .ok_or(ProbeFailure::Metrics)?;
        semantic_bytes = semantic_bytes
            .checked_add(u64::from(input.semantic().disclosed_bytes()))
            .ok_or(ProbeFailure::Metrics)?;
        provider_elapsed_ms = provider_elapsed_ms
            .checked_add(metric.provider_elapsed.as_millis())
            .ok_or(ProbeFailure::Metrics)?;
    }
    let total_tokens = input_tokens
        .checked_add(output_tokens)
        .ok_or(ProbeFailure::Metrics)?;
    let elapsed_ms = started.elapsed().as_millis();
    let workflow = scenario.workflow_label();
    let model_revision = model.revision();
    let provider_storage = match retention {
        ProbeRetention::Stateless => "stateless",
        ProbeRetention::InspectablePublicData => "retained-public-probe",
    };
    use std::io::Write as _;
    writeln!(
        std::io::stdout().lock(),
        "macos-terra-agentic-probe: passed; workflow={workflow}; model={model_revision}; provider_storage={provider_storage}; turns={model_turns}; verified_actions=2; first_backend={first_backend}; second_backend={second_backend}; input_tokens={input_tokens}; output_tokens={output_tokens}; total_tokens={total_tokens}; request_bytes={request_bytes}; semantic_bytes={semantic_bytes}; charged_micro_usd={charged_micro_usd}; provider_elapsed_ms={provider_elapsed_ms}; elapsed_ms={elapsed_ms}; turn_input_tokens={turn_input_tokens:?}; turn_cached_input_tokens={turn_cached_input_tokens:?}; turn_output_tokens={turn_output_tokens:?}; turn_reasoning_output_tokens={turn_reasoning_output_tokens:?}; turn_request_bytes={turn_request_bytes:?}; turn_semantic_bytes={turn_semantic_bytes:?}; turn_semantic_token_measurements={turn_semantic_token_measurements:?}; turn_structured_input_measurements={turn_structured_input_measurements:?}; turn_provider_elapsed_ms={turn_provider_elapsed_ms:?}; content=redacted",
    )
    .map_err(|_| ProbeFailure::Output)?;
    Ok(())
}
