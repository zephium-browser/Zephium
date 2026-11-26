use super::*;
use serde_json::{json, Value};
use std::io::{Read, Write};

struct DecisionTask(SemanticExtractionSchema, bool);
impl AgentWorkTask for DecisionTask {
    fn allows_actions_before_extraction(&self) -> bool {
        true
    }
    fn extraction_schema(&self) -> Option<&SemanticExtractionSchema> {
        Some(&self.0)
    }
    fn evaluate(
        &mut self,
        observation: &SemanticObservation,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure> {
        Ok(
            if !self.1
                && observation.frames()[0].nodes()[1]
                    .states()
                    .contains(SemanticState::Expanded)
            {
                AgentWorkTaskProgress::ReadyForExtraction
            } else {
                AgentWorkTaskProgress::Continue
            },
        )
    }
    fn model_action_operations(
        &self,
        node: &SemanticNode,
        _: &SemanticObservation,
    ) -> Result<SemanticOperations, AgentWorkFailure> {
        Ok(
            if node.role()
                == if self.1 {
                    SemanticRole::Document
                } else {
                    SemanticRole::Button
                }
            {
                node.operations()
            } else {
                SemanticOperations::NONE
            },
        )
    }
    fn decision_action_recipe(
        &self,
        operation: &DecisionOperation,
        _: &SemanticObservation,
    ) -> Result<Option<SemanticActionProposal>, AgentWorkFailure> {
        let (intent, verification) = match operation {
            DecisionOperation::Click(target) if !self.1 => (
                SemanticActionIntent::Click { target: *target },
                SemanticVerification::TargetState {
                    state: SemanticState::Expanded,
                    present: true,
                },
            ),
            DecisionOperation::Scroll(target) if self.1 => (
                SemanticActionIntent::Scroll {
                    target: *target,
                    direction: SemanticScrollDirection::Down,
                    amount: SemanticScrollAmount::HalfPage,
                },
                SemanticVerification::ScrollPositionChanged,
            ),
            _ => return Ok(None),
        };
        Ok(Some(
            SemanticActionProposal::try_new(
                intent,
                SemanticEffectClass::Read,
                SemanticWaitCondition::Immediate,
                verification,
                SemanticSettleBudget::try_new(MIN_AGENT_BROWSER_SNAPSHOT_SETTLE_MILLIS).unwrap(),
            )
            .unwrap(),
        ))
    }
    fn assess(
        &self,
        action: &SemanticPreparedAction,
    ) -> Result<AgentEffectAssessment, AgentWorkFailure> {
        assert_eq!(
            action.kind(),
            if self.1 {
                SemanticActionKind::Scroll
            } else {
                SemanticActionKind::Click
            }
        );
        assert_eq!(
            action.verification(),
            if self.1 {
                SemanticVerification::ScrollPositionChanged
            } else {
                SemanticVerification::TargetState {
                    state: SemanticState::Expanded,
                    present: true,
                }
            }
        );
        Ok(AgentEffectAssessment::new(
            action,
            action.frame().origin().clone(),
            SemanticEffectClass::Read,
        ))
    }
    fn attest_account(
        &self,
        context: ContextJoin,
        now: AgentPolicyInstant,
    ) -> Result<AgentContextAccountBinding, AgentWorkFailure> {
        Task.attest_account(context, now)
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Value {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let (header, length) = loop {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|chunk| chunk == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end]).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            break (end + 4, length);
        }
    };
    while bytes.len() < header + length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    serde_json::from_slice(&bytes[header..]).unwrap()
}

struct FirstLookCopy(AgentWorkExtractionTask, bool);
impl AgentWorkTask for FirstLookCopy {
    fn allows_human_request(&self) -> bool {
        self.1
    }
    fn assess(
        &self,
        action: &SemanticPreparedAction,
    ) -> Result<AgentEffectAssessment, AgentWorkFailure> {
        self.0.assess(action)
    }
    fn whole_first_look(&self) -> bool {
        self.1
    }
    fn extraction_schema(&self) -> Option<&SemanticExtractionSchema> {
        self.0.extraction_schema()
    }
    fn evaluate(
        &mut self,
        look: &SemanticObservation,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure> {
        self.0.evaluate(look)
    }
    fn accept_extraction(
        &mut self,
        result: &SemanticExtractionResult<'_>,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure> {
        self.0.accept_extraction(result)
    }
    fn attest_account(
        &self,
        context: ContextJoin,
        now: AgentPolicyInstant,
    ) -> Result<AgentContextAccountBinding, AgentWorkFailure> {
        self.0.attest_account(context, now)
    }
}

#[test]
fn typed_copy_finishes_after_one_decision_without_a_page_model_or_native_action() {
    let _serial = lock(&SERIAL);
    for refreshed in [false, true] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            for index in 0..if refreshed { 4 } else { 2 } {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                let request = read_request(&mut stream);
                let body = if index % 2 == 0 {
                json!({"object":"response.input_tokens","input_tokens":100})
            } else {
                let schemas = request["text"]["format"]["schema"]["properties"]["answers"]["properties"].as_object().unwrap();
                assert!(schemas.contains_key("locate_0"));
                let answers: serde_json::Map<_, _> = schemas.iter().map(|(key, schema)| {
                    let answer = if schema["properties"]["noul"].is_object() {
                        json!({"type":"noul","noul":if key == "done" {0.99} else {0.01}})
                    } else {
                        let keys = schema["properties"]["probabilities"]["properties"].as_object().unwrap();
                        let selected = if key == "locate_0" {"@a2"} else {"none"};
                        assert!(keys.contains_key(selected));
                        let probabilities: serde_json::Map<_, _> = keys.keys().map(|key| (key.clone(), json!(if key == selected {1.0} else {0.0}))).collect();
                        json!({"type":"choice","choice":selected,"confidence":1.0,"probabilities":probabilities})
                    };
                    (key.clone(), answer)
                }).collect();
                json!({"object":"response","status":"completed","model":"gpt-6-luna","service_tier":"default","error":null,"incomplete_details":null,
                    "output":[{"type":"reasoning","summary":[]},{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":json!({"answers":answers}).to_string()}]}],
                    "usage":{"input_tokens":100,"output_tokens":200,"total_tokens":300,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":50}}})
            }.to_string();
                write!(stream, "HTTP/1.1 200 Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let mut input = input_with_effects(&[SemanticEffectClass::Read]);
        input.decision_provider =
            Some(super::super::super::AgentBrowserDecisionProvider::Emulation);
        let task = AgentWorkExtractionTask::try_new(
            vec![
                SemanticExtractionFieldSchema::try_text("label".into(), true, 64)
                    .unwrap()
                    .with_verbatim_text()
                    .unwrap(),
            ],
            AgentAccountScope::Anonymous,
        )
        .unwrap();
        let (controller, handle) = AgentWorkController::try_new_for_probe(
            input,
            AgentProviderTransport::try_new_loopback(
                AgentProviderTransportConfig::STANDARD,
                &endpoint,
                &endpoint,
            )
            .unwrap(),
            AgentProviderCredential::try_new(
                AgentProviderKind::OpenAiResponses,
                "fixture-key".into(),
            )
            .unwrap(),
            Arc::new(Audit(Fault::None)),
            Box::new(FirstLookCopy(task, refreshed)),
            AgentBrowserRetention::Stateless,
        )
        .unwrap();
        let (outcome, shutdown, calls, _) = drive(
            controller,
            handle,
            if refreshed {
                Fault::DecisionRefresh
            } else {
                Fault::DecisionClick(false)
            },
        );
        assert!(matches!(shutdown, AgentBrowserShutdownOutcome::Clean(_)));
        if !refreshed {
            assert_eq!(calls, [1, 2, 3, 4, 5, 6]);
        }
        let AgentWorkOutcome::Succeeded(success) = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(
            success.closure().model_calls(),
            if refreshed { 2 } else { 1 },
            "a fresh look must retire the old decision and obtain one for its own provenance"
        );
        assert_eq!(success.closure().effects(), 0);
        assert!(
            matches!(success.extraction().unwrap().fields()[0].value(), SemanticExtractedValue::Text(value) if value.as_str() == "Details")
        );
        server.join().unwrap();
    }
}

#[test]
fn typed_native_read_verifies_or_closes_without_retry_and_keeps_accounting() {
    let _serial = lock(&SERIAL);
    for (scroll, applied, refused) in [
        (false, false, false),
        (false, true, false),
        (true, false, false),
        (false, false, true),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            for index in 0..if applied { 5 } else { 2 } {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing request {index}");
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                let request = read_request(&mut stream);
                let (status, body) = match index {
                    0 | 2 => (
                        200,
                        json!({"object":"response.input_tokens","input_tokens":100}),
                    ),
                    1 | 3 => {
                        assert_eq!(request["model"], "gpt-6-luna");
                        let schemas = request["text"]["format"]["schema"]["properties"]["answers"]
                            ["properties"]
                            .as_object()
                            .unwrap();
                        let answers: serde_json::Map<_, _> = schemas.iter().map(|(key, schema)| {
                            let answer = if schema["properties"]["noul"].is_object() {
                                json!({"type":"noul","noul":if scroll && key == "more_below" {0.99} else {0.01}})
                            } else {
                                let keys = schema["properties"]["probabilities"]["properties"].as_object().unwrap();
                                let selection = match key.as_str() {
                                    "operation" => if scroll {"scroll"} else {"click"},
                                    "click_target" | "scroll_target" => keys.keys().find(|key| key.as_str() != "none").unwrap(),
                                    _ => "none",
                                };
                                let probabilities: serde_json::Map<_, _> = keys.keys().map(|key| (key.clone(), json!(if key == selection {1.0} else {0.0}))).collect();
                                json!({"type":"choice","choice":selection,"confidence":1.0,"probabilities":probabilities})
                            };
                            (key.clone(), answer)
                        }).collect();
                        (
                            200,
                            json!({"object":"response","status":"completed","model":"gpt-6-luna","service_tier":"default","error":null,"incomplete_details":null,
                            "output":[{"type":"reasoning","summary":[]},{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":json!({"answers":answers}).to_string()}]}],
                            "usage":{"input_tokens":100,"output_tokens":200,"total_tokens":300,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":50}}}),
                        )
                    }
                    _ => {
                        assert_eq!(request["model"], "gpt-5.6-luna");
                        (401, json!({}))
                    }
                };
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let mut input = input_with_effects(&[SemanticEffectClass::Read]);
        input.decision_provider =
            Some(super::super::super::AgentBrowserDecisionProvider::Emulation);
        let extraction = AgentWorkExtractionTask::try_new(
            vec![SemanticExtractionFieldSchema::try_text("detail".into(), true, 64).unwrap()],
            AgentAccountScope::Anonymous,
        )
        .unwrap();
        let (controller, handle) = AgentWorkController::try_new_for_probe(
            input,
            AgentProviderTransport::try_new_loopback(
                AgentProviderTransportConfig::STANDARD,
                &endpoint,
                &endpoint,
            )
            .unwrap(),
            AgentProviderCredential::try_new(
                AgentProviderKind::OpenAiResponses,
                "fixture-key".into(),
            )
            .unwrap(),
            Arc::new(Audit(Fault::None)),
            Box::new(DecisionTask(
                extraction.extraction_schema().unwrap().clone(),
                scroll,
            )),
            AgentBrowserRetention::Stateless,
        )
        .unwrap();
        let (outcome, shutdown, calls, events) = drive(
            controller,
            handle,
            if refused {
                Fault::DecisionTargetChanged
            } else if scroll {
                Fault::ScrollVerification
            } else {
                Fault::DecisionClick(applied)
            },
        );
        let AgentWorkOutcome::ClosedUnsuccessfully(closed) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(matches!(shutdown, AgentBrowserShutdownOutcome::Clean(_)));
        assert_eq!(
            calls,
            if refused {
                vec![1, 2, 3, 7, 4, 5, 6]
            } else {
                vec![1, 2, 3, 7, 3, 4, 5, 6]
            }
        );
        assert_eq!(closed.policy_settlement().closure().effects(), 1);
        assert_eq!(
            closed.policy_settlement().closure().model_calls(),
            if applied { 3 } else { 1 }
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind(), AgentWorkEventKind::Verified))
                .count(),
            usize::from(applied)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind(), AgentWorkEventKind::ActionUnverified(_)))
                .count(),
            usize::from(!applied)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind(), AgentWorkEventKind::DecisionSettled(_)))
                .count(),
            if applied { 2 } else { 1 }
        );
        server.join().unwrap();
    }
}

/// The task's own consent press, which a page that saved the choice and
/// loaded itself again under it refuses as changed.
struct ConsentTask(DecisionTask);
impl AgentWorkTask for ConsentTask {
    fn allows_actions_before_extraction(&self) -> bool {
        true
    }
    fn extraction_schema(&self) -> Option<&SemanticExtractionSchema> {
        self.0.extraction_schema()
    }
    fn evaluate(
        &mut self,
        observation: &SemanticObservation,
    ) -> Result<AgentWorkTaskProgress, AgentWorkFailure> {
        self.0.evaluate(observation)
    }
    fn model_action_operations(
        &self,
        node: &SemanticNode,
        observation: &SemanticObservation,
    ) -> Result<SemanticOperations, AgentWorkFailure> {
        self.0.model_action_operations(node, observation)
    }
    fn decision_action_recipe(
        &self,
        operation: &DecisionOperation,
        observation: &SemanticObservation,
    ) -> Result<Option<SemanticActionProposal>, AgentWorkFailure> {
        self.0.decision_action_recipe(operation, observation)
    }
    fn consent_dismissal(&self, observation: &SemanticObservation) -> Option<SemanticReferenceId> {
        Some(observation.frames()[0].nodes()[1].reference())
    }
    fn assess(
        &self,
        action: &SemanticPreparedAction,
    ) -> Result<AgentEffectAssessment, AgentWorkFailure> {
        self.0.assess(action)
    }
    fn attest_account(
        &self,
        context: ContextJoin,
        now: AgentPolicyInstant,
    ) -> Result<AgentContextAccountBinding, AgentWorkFailure> {
        self.0.attest_account(context, now)
    }
}

#[test]
fn a_consent_press_the_page_refuses_as_changed_is_looked_at_again() {
    for fault in [Fault::DecisionTargetChanged, Fault::ConsentReload] {
        consent_press_refused_as_changed(fault);
    }
}

fn consent_press_refused_as_changed(fault: Fault) {
    let _serial = lock(&SERIAL);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        for index in 0..2 {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing request {index}");
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            let request = read_request(&mut stream);
            let body = if index == 0 {
                json!({"object":"response.input_tokens","input_tokens":100})
            } else {
                let schemas = request["text"]["format"]["schema"]["properties"]["answers"]
                    ["properties"]
                    .as_object()
                    .unwrap();
                let answers: serde_json::Map<_, _> = schemas.iter().map(|(key, schema)| {
                    let answer = if schema["properties"]["noul"].is_object() {
                        json!({"type":"noul","noul":0.01})
                    } else {
                        let keys = schema["properties"]["probabilities"]["properties"].as_object().unwrap();
                        let selection = if key == "wall" { "cookie" } else { "none" };
                        let probabilities: serde_json::Map<_, _> = keys.keys().map(|key| (key.clone(), json!(if key == selection {1.0} else {0.0}))).collect();
                        json!({"type":"choice","choice":selection,"confidence":1.0,"probabilities":probabilities})
                    };
                    (key.clone(), answer)
                }).collect();
                json!({"object":"response","status":"completed","model":"gpt-6-luna","service_tier":"default","error":null,"incomplete_details":null,
                "output":[{"type":"reasoning","summary":[]},{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":json!({"answers":answers}).to_string()}]}],
                "usage":{"input_tokens":100,"output_tokens":200,"total_tokens":300,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":50}}})
            };
            let body = body.to_string();
            write!(stream, "HTTP/1.1 200 Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let mut input = input_with_effects(&[SemanticEffectClass::Read]);
    input.decision_provider = Some(super::super::super::AgentBrowserDecisionProvider::Emulation);
    let extraction = AgentWorkExtractionTask::try_new(
        vec![SemanticExtractionFieldSchema::try_text("detail".into(), true, 64).unwrap()],
        AgentAccountScope::Anonymous,
    )
    .unwrap();
    let (controller, handle) = AgentWorkController::try_new_for_probe(
        input,
        AgentProviderTransport::try_new_loopback(
            AgentProviderTransportConfig::STANDARD,
            &endpoint,
            &endpoint,
        )
        .unwrap(),
        AgentProviderCredential::try_new(AgentProviderKind::OpenAiResponses, "fixture-key".into())
            .unwrap(),
        Arc::new(Audit(Fault::None)),
        Box::new(ConsentTask(DecisionTask(
            extraction.extraction_schema().unwrap().clone(),
            false,
        ))),
        AgentBrowserRetention::Stateless,
    )
    .unwrap();
    let (outcome, shutdown, calls, _) = drive(controller, handle, fault);
    assert!(matches!(shutdown, AgentBrowserShutdownOutcome::Clean(_)));
    let action = calls.iter().position(|&call| call == 7).expect("the press");
    assert_eq!(
        calls[action + 1],
        3,
        "the page is looked at again after the press: {calls:?}"
    );
    if let AgentWorkOutcome::ClosedUnsuccessfully(closed) = &outcome {
        assert!(
            !matches!(
                closed.failure(),
                AgentWorkFailure::Browser(AgentBrowserProviderError::Action(_))
                    | AgentWorkFailure::Observation(_)
            ),
            "{fault:?}: {outcome:?}"
        );
    }
    server.join().unwrap();
}
