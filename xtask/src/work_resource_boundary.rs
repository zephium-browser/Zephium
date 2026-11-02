//! Mechanical guards for persistent resource ownership and its opt-in adapter.

use std::path::Path;

pub(crate) fn check(root: &Path) -> Result<(), String> {
    let source =
        std::fs::read_to_string(root.join("crates/zephium-agentic/src/work_browser_resource.rs"))
            .map_err(|e| e.to_string())?;
    let port = std::fs::read_to_string(root.join("crates/zephium-agentic/src/context_port.rs"))
        .map_err(|e| e.to_string())?;
    validate(&source, &port)?;
    for (path, required, forbidden) in ADAPTER_RULES {
        let source = std::fs::read_to_string(root.join(path)).map_err(|e| e.to_string())?;
        let source = adapter_source(path, source)?;
        validate_adapter(&source, required, forbidden).map_err(|e| format!("{path}: {e}"))?;
        if path.ends_with("platform/macos/agent_context.rs") {
            validate_url_revocation_order(&source)?;
        }
    }
    validate_preflight_order(
        &std::fs::read_to_string(
            root.join("crates/zephium-work-composition/src/retained_qualification.rs"),
        )
        .map_err(|e| e.to_string())?,
    )?;
    Ok(())
}

fn validate_preflight_order(source: &str) -> Result<(), String> {
    let source = compact(production(source));
    let preflight = source
        .find("let(fixture,target)=task::document()?")
        .ok_or("missing document preflight")?;
    for effect in [
        "RetainedWorkProbeOwner::new(",
        "letcredential_job=std::thread::Builder::new()",
    ] {
        if source
            .find(effect)
            .is_none_or(|position| position <= preflight)
        {
            return Err(
                "document preflight must precede native owner and credential worker".into(),
            );
        }
    }
    Ok(())
}

fn validate_url_revocation_order(source: &str) -> Result<(), String> {
    let source = compact(production(source));
    let branch = source
        .split("ifwork_location.as_ref().is_some_and(|gate|gate.failed()){")
        .nth(1)
        .ok_or("missing failed URL branch")?;
    let close = branch
        .find("work_location_semantic.revoke_document_authority();")
        .ok_or("missing semantic URL revocation")?;
    for callback in ["report(", "invoke_owned_unit_callback("] {
        if branch
            .find(callback)
            .is_none_or(|position| close >= position)
        {
            return Err("semantic URL revocation must precede host observers".into());
        }
    }
    Ok(())
}

const ADAPTER_RULES: &[(&str, &[&str], &[&str])] = &[
    (
        "crates/zephium-engine/src/host/work_resource_action.rs",
        &[
            "PASSIVE_SETTLEMENT:Duration=Duration::from_secs(3)",
            "RETIREMENT_MARGIN:Duration=Duration::from_millis(100)",
            "deadline.checked_sub(RETIREMENT_MARGIN)",
            "guard.action_drain_current(&action.lease,action.attempt,now)",
            "semantic.revoked_settling_action(action.attempt)",
            "action.accepts_result_drain(semantic.draining_action(action.attempt),semantic.revoked_settling_action(action.attempt),)",
            "!self.cancelled&&self.dispatched&&(runtime_pending||runtime_settling)",
            "semantic.settling_action(action.attempt)",
            // Only a confirmed commit's same-site POST may outlive its action.
            "ifexpired||(!current&&!drain&&!posting)",
            "ifaction.cancelled||terminal_ready",
            "action.wakes>=MAX_WAKES",
            "task.complete(terminal)",
        ],
        &["evaluateJavaScript", "requestAnimationFrame", "std::thread", "tokio::"],
    ),
    (
        "crates/zephium-engine/src/host/work_resource_observation.rs",
        &[
            "OBSERVATION_BUDGET:Duration=Duration::from_secs(5)",
            // A document's first look waits for a heavy app, its wakes cover
            // its whole budget, and a look never dispatched is not ready.
            "FIRST_OBSERVATION_BUDGET:Duration=Duration::from_secs(15)",
            "constCLEANUP_WAKES:u16=4;", "ifread.wakes>=read.max_wakes",
            "max_wakes:max_wakes(duration)",
            "read.refuse(ifread.dispatched{SemanticRuntimePortFailure::TimedOut}else{SemanticRuntimePortFailure::NotReady});",
            "RENDERING_OPPORTUNITY:Duration=Duration::from_millis(100)",
            "request.invocation().invocation().get()>resource.last_invocation",
            "current_scope(request.observation().scope(),resource.last_invocation)",
            "anchor.snapshot_generation().get()==last_invocation",
            "SemanticScope::Table(_)|SemanticScope::Frame(_)=>false",
            // f8996d56: a busy peer presentation retires idle reading surfaces or waits.
            "letpresentation_busy=self.work_resources.iter().any(|(id,resource)|{*id!=guard.resource().identity().context()&&resource.presentation_in_flight()});",
            "ifpresentation_busy&&!resource.retire_reading_presentation(){task.refuse(SemanticRuntimePortFailure::NotReady);return;}",
            "Arc::ptr_eq(&resource.guard,&guard)", "Arc::ptr_eq(&resource.guard,guard)",
            "WorkObservationPresentation::human_current", "gate.observation_stamp(read.correlation.frame().context())",
            "Some(read.document)", "!self.erasure_tombstones.contains(&resource.profile())",
            "resource.observation=Some(WorkObservation", "wake:ObservationWake::schedule(&guard)",
            "guard.admits(&read.lease,now)", "presentation.present()", "Instant::now()>=read.deadline",
            // 27a3726e: only a clean successful read keeps its page presented.
            "letpreserve=read.refusal.is_none()&&read.outcome.as_ref().is_some_and(|outcome|outcome.is_ok())&&resource.revocation.is_none()&&resource.destruction.is_none();",
            "letretired=preserve||read.retirement_ready()", "read.delivery_ready(retired,idle)",
            "self.callback_returned", "self.wake.as_ref().is_none_or(ObservationWake::drained)",
            "self.correlation!=*correlation||!self.dispatched||self.callback_returned",
            "drop(debt);notify_work_resource(queued)",
            "self.permit=None;self.active.store(false,Ordering::Release)",
            "ended.presentation=None", "task.complete(outcome)",
        ],
        &["ContextRegistry", "create_agent_context", "evaluateJavaScript", "requestAnimationFrame", "makeKey", "makeFirstResponder", "std::thread", "tokio::"],
    ),
    (
        "crates/zephium-engine/src/platform/macos/work_observation_presentation.rs",
        &[
            // cd6792df..53d344ba: the page is hosted beneath the human window's
            // chrome instead of a borderless surface; the window refuses its focus.
            "letmain=page.window().ok_or(PresentationState::Unavailable)?",
            "letresting_facts=[page.isHidden(),original_frame.size==viewport().size,!responder_inside(&main,&page),]",
            "if!resting_facts.into_iter().all(|fact|fact)",
            "if!super::passive_page::register(&self.page)",
            "self.state=PresentationState::Acquiring;",
            "self.parent.addSubview_positioned_relativeTo(&self.page,NSWindowOrderingMode::Below,None);self.page.setFrame(viewport());self.page.setHidden(false);",
            "Instant::now()>=self.deadline",
            "self.page.setHidden(true);self.page.setFrame(self.original_frame);super::passive_page::unregister(&self.page);",
            "letoriginal_parent=unsafe{self.page.superview()}.is_some_and(|parent|std::ptr::eq(&*parent,&*self.parent));",
            "page.load().is_some_and(|page|hosted_current(&page,&main))",
            "&&!responder_inside(main,page)", "self.cleanup_failed|=", "human_owners(&self.app)!=before",
        ],
        &["makeKeyAndOrderFront", "makeFirstResponder", "activateIgnoringOtherApps", "setActivationPolicy", "setInactiveSchedulingPolicy", "requestAnimationFrame", "evaluateJavaScript", "runUntilDate"],
    ),
    (
        "crates/zephium-agentic/src/work_browser_navigation.rs",
        &[
            "letOk(request)=active.native_request()",
            "request.operation()!=self.join.operation||request.redirect_policy().is_some()",
            "state.context()!=source||!state.can_automate()",
            "row.observation.is_some()||row.navigation.is_some()",
            "row.document_available=false;row.observed=false;row.navigation=Some(join.clone())",
            "row.navigation.as_ref()!=Some(&completion.join)",
            "Ok(target)ifself.document_policy.admits_final_document(&self.target,&target)=>{Ok(target)}",
            "row.navigation_epoch=completion.join.operation.context().navigation_epoch()",
            "row.frame_generation=completion.join.operation.context().frame_generation()",
            "row.lease.as_ref()==Some(&completion.join.lease)",
            "now<completion.join.lease.deadline",
        ],
        &["Serialize", "Deserialize", "ContextRegistry", "std::thread", "tokio::", "evaluateJavaScript", "ForegroundRendering"],
    ),
    (
        "crates/zephium-engine/src/work_resource_navigation_port.rs",
        &[
            "state.observed!=Some(request.source())",
            "state.document_epoch!=request.source().navigation_epoch().get()",
            "state.reads!=0||state.callbacks!=0||state.navigation.is_some()",
            "state.navigation=Some(request.navigation().operation());state.observed=None",
            "state.navigation==Some(operation)",
            "now<lease.deadline()",
            "self.guard.navigation_terminal_begin(operation)",
            "completion(request.into_completion().settle(outcome))",
            "self.guard.navigation_terminal_end(operation,committed);self.permit.release()",
            "task.and_then(WorkNavigationTask::rejected)",
            "ifaccepted||executed.load(Ordering::Acquire)",
        ],
        &["ContextRegistry", "ContextIdentity::new", "ContextJoin::", "cancel_run(", "ForegroundRendering", "evaluateJavaScript"],
    ),
    (
        "crates/zephium-engine/src/host/work_resource_navigation.rs",
        &[
            "Arc::ptr_eq(&resource.guard,&guard)",
            "!self.erasure_tombstones.contains(&resource.profile())",
            "!resource.pending()&&resource.ready()",
            "NAVIGATION_BUDGET.min(Duration::from_millis(",
            "lease.deadline().millis().saturating_sub(now.millis())",
            // 99bf594a: dispatch waits for the runtime to park, then arms the successor;
            // each stage rechecks its own lease authority.
            "resource.navigation=Some(WorkNavigation{task,timer,deadline,stage:WorkNavigationStage::Parking,departure,})",
            // d4b32b9c: a failed load returns to its departure only under the same
            // health, deadline and dispatch-lease checks as the original navigation.
            "iferased||!self.guard.is_healthy()||now>=lease.deadline()||!self.guard.navigation_dispatch_current(&lease,operation,now){returnfalse;}",
            "gate.arm_departure(source,operation,departure.clone()).is_ok()",
            "WorkNavigationStage::Parking=>self.guard.navigation_dispatch_current(&lease,operation,now)",
            "WorkNavigationStage::Navigating=>self.guard.navigation_completion_current(&lease,operation,now)",
            "view.semantic_runtime_parked()",
            "view.prepare_semantic_document_load().is_ok()&&view.work_navigation().is_some_and(|gate|gate.arm_successor(source,&native).is_ok())",
            ".load_url(native.target().as_url().as_str())",
            "gate.take_successor_terminal()",
            "pending.task.complete(outcome)",
            "request.navigation().operation()==operation",
        ],
        &["ContextRegistry", "ContextIdentity::new", "create_agent_context", "construct_work_resource", "evaluateJavaScript", "std::thread", "tokio::"],
    ),
    (
        "crates/zephium-app/src/work_resources_product.rs",
        &[
            // b8bf7f37: the actor also carries the durable human wait; a continuation must have one.
            "letspec=input.retained_resource_spec()?", "StagedActor::try_new(input,browser,runtime,provider,access,audit,task,waiting,)",
            "letwaiting=waiting.ok_or_else(||crate::contract_failure!())?;StagedActor::try_new(input,browser,runtime,provider,access,audit,task,Some(waiting),)",
            "spec.identity.profile()!=profile.profile()", "spec.storage!=profile.storage_class()",
            "!std::ptr::addr_eq(Arc::as_ptr(&journal),Arc::as_ptr(&audit))",
            "Arc::ptr_eq(engine,&prepared.engine)",
            "std::ptr::addr_eq(Arc::as_ptr(store),Arc::as_ptr(&prepared.journal))",
            "std::ptr::addr_eq(Arc::as_ptr(store),Arc::as_ptr(&prepared.audit))",
            "readiness==AgentWorkProfileReadiness::Ready(prepared.profile)",
            "!self.signal.stop.load(Ordering::Acquire)", "callback.wake_retained_work()",
            "RetainedWork::constructing(owner,pending,prepared.journal,prepared.audit,)",
            "self.failed_owner=Some(owner)", "self.native_uncertain=true",
            "projection.events.len()<MAX_AGENT_WORK_EVENTS", "projection.extraction=work.take_extraction()",
            "work.shutdown_until(self.clock.as_ref(),deadline)",
            "clean&&!self.native_uncertain&&self.failed_owner.is_none()",
            "implDropforProductWork",
            "self.prepared.is_some(){self.refuse(RetainedRefusal::Discarded,RetainedLaneFacts::default());}",
        ],
        &["InspectablePublic", "try_new_for_probe", "AgentWorkController::", "PendingAgentRuntime", "ContextRegistry", "std::thread", "tokio::spawn", "Serialize", "Deserialize", "execute_semantic_action", "invoke_semantic", "AgentNativeShutdownProof"],
    ),
    (
        "crates/zephium-app/src/shell/mod.rs",
        &[
            // b892f874/24832e90: a stuck retained Work moves to a draining graveyard,
            // and shutdown begins on every retained owner instead of one.
            // A retained work waits for no application work or lifecycle,
            // and a lone retained work for no other retained or page group.
            "letrefusal=ifself.work.is_some()||!matches!(self.agent_lifecycle,AgentLifecycleOwner::Absent){Some(Refusal::Busy)}",
            "ifself.retained_work.as_ref().is_some_and(|work|!work.is_closed())||self.retained_page_runtime.is_some(){trace_refusal(Refusal::Busy,lane);work.refuse(Refusal::Busy,lane);}",
            "!work.admits(&self.engine,&self.store,self.work_profile_binding())",
            "self.retained_work=Some(Box::new(work));self.retained_work.as_mut().unwrap().initialize()",
            "ifletSome(mutstuck)=self.retained_work.take(){stuck.begin_shutdown();self.retained_graveyard.push(*stuck);}",
            "forpagein&mutself.retained_pages{page.begin_shutdown();}",
            "ifletSome(work)=&mutself.retained_work{work.begin_shutdown();}",
            "forworkin&mutself.retained_graveyard{work.begin_shutdown();}",
        ], &[],
    ),
    (
        "crates/zephium-agentic/src/work_browser_document.rs",
        &[
            "#[default]Exact", "url.scheme()==\"https\"", "url.query().is_none()",
            "url.fragment().is_none()", "url.username().is_empty()", "url.password().is_none()",
            "query|!query.is_empty()", "effective.as_url().fragment().is_none()",
            "Some(requested.as_url().as_str())",
        ],
        &["Serialize", "Deserialize", "reqwest", "WKWebView", "ContextNavigationRequest"],
    ),
    (
        "crates/zephium-engine/src/platform/macos/agent_context.rs",
        &[
            "ifgate.failed(){",
            "invoke_owned_unit_callback(navigation_invariant_failure.as_ref(),navigation_callback_panicked.as_ref(),);navigation_semantic.cancel();",
            "ifwork_location.as_ref().is_some_and(|gate|gate.failed()){",
            "work_location_semantic.revoke_document_authority();",
            // Work's own store is persistent, carries exactly the identifier
            // derived for its profile, and is never the profile's own store.
            "persistent&&identifier==Some(expected)&&identifier!=Some(profile.bytes())",
            "Some(expected)=>{work_store_matches(profile,&actual_store)&&Retained::as_ptr(&actual_store)==Retained::as_ptr(expected)}",
        ],
        &[],
    ),
    (
        // Work's own website data replaces per-run ephemeral storage for pages
        // without the person's session (2026-10-09): a stable, hash-derived
        // identifier per profile that can never equal the profile's own.
        "crates/zephium-engine/src/work_website_data.rs",
        &[
            "Sha256::new().chain_update(DOMAIN).chain_update(profile.bytes())",
            "ifid==profile.bytes(){id[15]^=1;}",
        ],
        &["dataStoreForIdentifier", "ProfileId::generate()"],
    ),
    (
        "crates/zephium-app/src/work_resources_probe.rs",
        &[
            "owner:WorkResourceOwner", "WorkResourceOwner::new(WorkId::generate(),profile,wake,factory)",
            "self.owner.retained_browser(lease,now)", "SnapshotReleaseBrowser::from_capture(browser,self.resource.clone().ok_or(\"resource\")?,capture,)",
            "AgentWorkRetainedController::try_new(", "PendingScopedAgentRuntime::spawn_suspended(",
            "AgentProviderTransportConfig::STANDARD", "self.owner.poll_native_event()", "self.owner.shutdown_audit(audit)",
            "ifself.constructed||self.pending.is_some()", "ifself.acquired||self.pending.is_some()", "ifself.started||self.pending.is_some()",
            "!=WorkBrowserResourcePhase::Destroyed", "self.audit_attempted=true",
            "self.audit_pending=Some(audit)", "self.audit_pending==Some(settlement.audit())",
            "self.audit_pending=None", "accounted&&self.audit_pending.is_none()",
            "resource.reads.load(Ordering::Acquire)==0", "health.reporter_retired()",
        ],
        &["AgentNativeShutdownProof", "AgentBrowserShutdownOutcome", "ContextRegistry::new", "WorkBrowserResources::new", "Serialize", "Deserialize", "attach_successor", "browser_loop", "persist_extraction_result", "try_new_for_probe", "reap_absent("],
    ),
    (
        "crates/zephium-engine/src/platform/macos/agentic_resource_composition_probe.rs",
        &[
            "port:ResourceWitnessPort,resource:WorkBrowserResourceJoin", "MainThreadMarker::new()?",
            "if!admission.remains_current()", "engine.agent_context_port.resource_witness_port()?",
            "Request{resource:self.resource.clone(),operation,document:self.document,}", "ifactual==expected",
            "Self::with_document(engine,admission,resource,Document::RenderingFixture)",
            "Document::PublicProductBrief.admits(target)",
            "Self::with_document(engine,admission,resource,Document::PublicProductBrief)",
            "self.schedule(Operation::Acquire,completion)", "self.schedule(Operation::Retire,completion)",
            "engine.agent_context_port.construction_evidence(resource)",
        ],
        &["AgentProvider", "AgentBrowserPort", "WorkBrowserResources", "evaluateJavaScript", "setActivationPolicy", "makeKey", "makeMain", "NSRunLoop", "requestAnimationFrame", "unsafe", "Serialize", "Deserialize"],
    ),
    (
        "crates/zephium-work-composition/src/retained_qualification.rs",
        &[
            "owner:RetainedWorkProbeOwner", "admission:ForegroundRenderingAdmission",
            "load_probe_openai_credential()", "native.take_agent_browser_port(move|event|sink(event))",
            "task::capture(retire)",
            "self.owner.construct_with_policy(self.target.clone(),task::document_policy(),now()?,)",
            "self.owner.document(now()?)?",
            "self.store.clone()", "self.owner.start(", "lifecycle.drain_until(deadline)",
            "let(fixture,target)=task::document()?", "input::input(", "task::OBJECTIVE", "sample.trace()",
            "EXPECTED_RESOURCE.with(|r|*r.borrow_mut()=self.owner.resource().cloned())",
            "lifecycle_trace(self.phase,&event,construction)",
            "WorkResourceRenderingProbe::construction_evidence(&self.engine,resource)",
            "#[cfg(feature=\"retained-public-qualification\")]#[path=\"retained_qualification_public.rs\"]modtask;",
            "mpsc::sync_channel(4)", "TOTAL:Duration=Duration::from_secs(150)", "CLEANUP:Duration=Duration::from_secs(5)",
            "ifstate==ForegroundRenderingState::Retiring{return;}",
            "deliver_snapshot_retirement(&release,state,&signal)", "self.owner.presentation_returned()",
            "task::verify_owned(extraction,expected)", "owner.poll_seal(", "self.owner.locally_retired()",
            "(Phase::Destroy,WorkBrowserResourceEvent::Destroyed(_))=>*phase=Phase::NativeDrain,",
            "HARD_CLEANUP_GRACE:Duration=Duration::from_secs(5)",
            "deadline.checked_add(HARD_CLEANUP_GRACE)",
            "record_cleanup_failure(&mutself.phase,&mutself.cleanup_failure,reason)",
            "ifreason==\"hard_cleanup_deadline\"{*phase=Phase::Done;}",
            "progress_native_close(&mutself.owner,&mutself.phase,now()?)?",
            "letclosed=self.closure_ready()", "self.signal.shutdown.store(true,Ordering::Release)",
            "drop((self.engine,self.store,self.render))", "slot.borrow_mut().retain(retained)",
            "release_if(NativeShutdownOwner::drain_after_engine_shutdown)",
            "proof.matches_runtime(control)", "Some(proof.lease().resource())==self.owner.resource()",
            "&&self.worker_drained",
            "self.admission.remains_current()", "drop(self)", "fixture.shutdown()",
            "fnpoll_actor(&mutself)->Result<(),&'staticstr>{self.drain_actor_metadata()?;if!self.completion.as_ref().is_some_and(AgentRuntimeCompletion::is_stopped){returnOk(());}",
        ],
        &["AgentNativeShutdownProof", "AgentBrowserShutdownOutcome", "ContextRegistry", "WorkBrowserResources", "browser_loop", "AgentBrowserSession", "evaluateJavaScript", "NSRunLoop", "std::thread::sleep", "OPENAI_API_KEY", "Command::new", "attach_successor", "persist_extraction_result"],
    ),
    (
        "crates/zephium-work-composition/src/retained_qualification_task.rs",
        &[
            "snapshot.completeness()==SemanticCompleteness::Complete", "observation.request().context().identity()==self.context",
            "!sample.current||!sample.complete||sample.boundaries!=0||sample.markers.into_iter().any(|present|!present)",
            "node.role()==SemanticRole::Paragraph", "fragment.provenance().reference()!=expected.reference",
            "fragment.provenance().frame()!=&expected.frame", "source.reference==expected.reference",
            "source.frame==expected.frame", "source.role==SemanticRole::Paragraph",
            "AgentAccountScope::Anonymous", "FixtureServer::start()", "Some(fixture)",
            "RetainedProbeCapture::BoundedReadiness(retire)",
            "zephium_agentic::WorkBrowserDocumentPolicy::Exact",
        ],
        &["allows_actions_before_extraction", "navigation_target", "with_source_roles", "with_subtree_extraction", "AgentBrowserPort", "Serialize", "Deserialize"],
    ),
    (
        "crates/zephium-work-composition/src/retained_qualification_input.rs",
        &[
            "AgentRunBudget::try_new(8,100_000,100_000,1)",
            "AgentBrowserModel::Luna",
            "AgentEffectScope::try_new(&[SemanticEffectClass::Read])",
            "SemanticSensitivity::Public",
            "AgentAccountScope::Anonymous",
            "ContextProfileStorageClass::Ephemeral",
            "AgentWorkRunInput::try_new(",
            "objective.into()",
        ],
        &[
            "AgentBrowserPort",
            "Serialize",
            "Deserialize",
            "std::env",
            "Command::new",
            "try_new_for_probe",
        ],
    ),
    (
        "crates/zephium-work-composition/src/retained_qualification_public.rs",
        &[
            "TARGET:&str=\"https://shop.pimoroni.com/products/raspberry-pi-pico-2\"",
            "ContextNavigationTarget::parse(TARGET)",
            "WorkResourceRenderingProbe::admits_public_product(&target)",
            "WorkResourceRenderingProbe::for_public_product(engine,admission,resource)",
            "RetainedProbeCapture::OneShot(retire)",
            "zephium_agentic::WorkBrowserDocumentPolicy::InitialQueryFinalization",
            "RetainedProbeTrace::ConfiguredPublic",
            "RetainedProbeTrace::PublicObservation",
            "context.kind()!=ContextKind::Owned",
            "value.starts_with(\"Alowcost,\")&&value.contains(\"RP2350\")",
            "value==PRODUCT",
            "SemanticReadField::AccessibleName",
            "SemanticReadField::VisibleText",
            "snapshot.completeness()==SemanticCompleteness::Complete",
            "observation.request().context().identity()==self.context",
            "!sample.current||!sample.complete||sample.boundaries!=0||matched!=[true;2]",
            "ifexpected.is_some()",
            "self.extraction.attest_account(context,now)",
            "self.accepted||result.observation()!=expected.observation",
            "fragment.field()!=source_field",
            "fragment.provenance().reference()!=expected.sources[index].reference",
            "fragment.provenance().frame()!=&expected.frame",
            "source.field==source_field",
            "fragment.provenance().invocation()!=expected.invocation",
            "fragment.provenance().snapshot()!=expected.snapshot",
            "source.invocation==expected.invocation",
            "source.snapshot==expected.snapshot",
            "source.reference==expected.sources[index].reference",
            "source.frame==expected.frame",
            "value.as_str()==expected.sources[index].value",
            "sources.next().is_none()",
            "self.accepted=true",
            "AgentAccountScope::Anonymous",
        ],
        &[
            "allows_actions_before_extraction",
            "navigation_target",
            "with_subtree_extraction",
            "AgentBrowserPort",
            "Serialize",
            "Deserialize",
            "FixtureServer::start",
            "std::env",
            "Command::new",
            "Duration::from",
        ],
    ),
    (
        "crates/zephium-app/src/work_resources_snapshot_probe.rs",
        &[
            "browser:RetainedBrowser",
            "resource:WorkBrowserResourceJoin",
            "browser.binding().lease().resource()!=&release.resource",
            "self.observation=Some(observation);self.release.start()?",
            "ifself.release.returned()?{Ok(self.observation.take())}",
            "ifself.observed{returnErr(AgentWorkFailure::Contract);}",
            "fnallows_readiness_retry(&self)->bool{self.readiness_retry}",
            "RetainedProbeCapture::OneShot(retire)=>(retire,false)",
            "RetainedProbeCapture::BoundedReadiness(retire)=>(retire,true)",
            "Self::new(browser,release.clone())?", "browser.readiness_retry=readiness_retry",
            "ifself.readiness_retry&&matches!(result,Err(AgentWorkFailure::Observation(SemanticRuntimePortFailure::NotReady))){self.observed=false;}",
            "letreturned=ReleaseCompletion(Some(self.clone()))",
            "implDropforReleaseCompletion",
            "let_=slot.complete(false)",
            "listener:Mutex<Option<Arc<Waker>>>",
            "std::panic::catch_unwind",
        ],
        &["AgentBrowserPort", "AgentProviderTransport", "ContextRegistry", "AgentNativeShutdownProof", "Serialize", "Deserialize", "evaluateJavaScript", "browser_loop", "tokio::spawn", "std::thread", "WorkBrowserResources::new"],
    ),
    (
        "crates/zephium-agent-controller/src/work.rs",
        &[
            // 08219870: a retained browser must also support screenshots to take one.
            "ifretained.is_some()&&(extraction_schema.is_none()||(actions_before_extraction&&!retained.as_ref().is_some_and(|browser|browser.supports_actions()))||subtree_extraction||navigation_target.is_some()||navigation_route.is_some()||(navigation_discovery.is_some()&&!retained.as_ref().is_some_and(|browser|browser.supports_navigation()))||(viewport_screenshot&&!retained.as_ref().is_some_and(|browser|browser.supports_screenshots())))",
            "resources:retained.is_none().then(||WorkContextResources{",
            "ifstate.native.retained.is_some(){journal.emit(AgentWorkEventKind::ContextActive)?;self.start_session()?;self.browser_loop(worker,browser).await?;returnself.close_retained(worker,None).await;}",
            "state.transport.take().ok_or_else(||crate::contract!())?",
            "worker.try_claim_scoped_terminal(class)",
            "claim.commit(delivery,settlement,provider)",
            "state.native.retained.take()",
            "Self::reconcile_deferred_audit(state)",
            "Self::drain_retained_navigation(state,deadline).await",
            "delivery.proof()==settlement.proof()",
            "ifstate.native.retained.as_ref().is_some_and(|browser|!browser.allows_readiness_retry()){returnSelf::observe_once(state,worker,browser).await;}",
            // Readiness is waited for by time: another page's look holds the presentation.
            "whilestarted.elapsed()<NOT_READY_PATIENCE",
            "constNOT_READY_PATIENCE:Duration=Duration::from_secs(30);",
            "tokio::time::sleep(delay)",
        ],
        &["WorkBrowserResources::new", "implAgentWorkRetainedBrowser", "attach_successor_work"],
    ),
    (
        "crates/zephium-agent-controller/src/work_retained.rs",
        &[
            "pubtraitAgentWorkRetainedBrowser:Send", "fnregister_listener(&mutself,waker:Waker)",
            "fncheck_health(&self,now:AgentPolicyInstant)",
            "fnsupports_navigation(&self)->bool{false}",
            "fndispatch_navigation(&mutself,_active:&AgentActiveNavigation)",
            "retained.poll_navigation(now)",
            "session.settle_navigation_terminal(&terminal).is_ok()",
            "fnallows_readiness_retry(&self)->bool{true}",
            "fnbinding(&self)->&WorkBrowserReadBinding",
            "implzephium_agent_runtime::AgentRuntimeScopedControllerforAgentWorkRetainedController",
            "controller.execute(&mutworker,&WorkBrowser::Retained).await",
            "AgentWorkController::with_transport(",
            "browser.register_listener(cx.waker().clone())",
            "binding.requested_document()!=&input.context.target",
            "binding.document_policy()!=input.context.document_policy",
            "input.context.document_policy.admits_final_document(binding.requested_document(),binding.document())",
            "binding.storage()!=input.context.storage",
            "session.try_finish_unsuccessful()", "session.try_finish()",
            "provider:Some(terminal.provider)",
            "state.native.retained_delivery=Some(delivery)",
            "controller.drain_recovery(&mutworker,&WorkBrowser::Retained).await",
            // 8755115b: the frozen terminal intent replaces the unsuccessful flag.
            "controller.publish_terminal(&mutworker).await",
            "journal.audit.is_quiescent()",
            "Self::Retained=>ContextDispatch::Unsupported",
            "self.0.journal_admission(owner)",
        ],
        &["AgentBrowserPort", "ContextRegistry::new", "AgentNativeShutdownProof", "AgentNativeShutdownCoordinator", "AgentWorkJournalPort", "AgentWorkJournalRequest", "Serialize", "Deserialize", "std::thread", "tokio::spawn", "try_prove_shutdown"],
    ),
    (
        "crates/zephium-app/src/work_resources_application.rs",
        &[
            "owner:WorkResourceOwner", "journal:Arc<dynAgentWorkJournalPort>",
            "std::ptr::addr_eq(Arc::as_ptr(&journal),Arc::as_ptr(&audit))",
            "work.dispatch(AgentWorkJournalRequest::Claim)",
            "letwaker=owner.shared.notifications.clone().into()",
            "matchself.owner.poll_native_event()", "self.unexpected_native=Some(event)",
            "record==mutation.next()", "AdmissionPhase::Starting,AgentWorkDisposition::Running",
            "PendingScopedAgentRuntime::spawn_suspended(config,scope,controller)",
            "completion.set_waker(self.waker.clone())",
            "drained.work_terminal(&active.runtime,record)",
            "drained.lease().resource()==&self.resource",
            "policy==Some(drained.policy())",
            "active.extraction.is_none()", "!active.handle.has_pending_events()",
            "prior.as_bytes()[48..64]==request.run.bytes()",
            "staged.lease.deadline()<=now", "flight.reconciliations>=MAX_RECONCILIATIONS",
            "self.dispatch_attempt(flight.request,flight.phase,flight.reconciliations+1)",
            "self.owner.locally_retired()", "self.flight.is_none()", "self.unstarted.is_none()",
            "self.destruction_settled=true", "Err(Refusal::Busy)=>returnOk(false)",
            // 24832e90: readiness moved into native_audit_ready and grouped pages wait for their turn.
            // A never-started actor whose row is classified, or that never had one, now takes
            // its audit and ends unclean like a classified scoped recovery: holding it forever
            // kept its group's native seat, so the run's later pages waited out their turn.
            "fnnative_audit_ready(&self)->bool{self.local_shutdown_settled()||(self.destroyed&&self.owner.locally_retired()&&self.unexpected_native.is_none()&&self.finally_unclean())}",
            "fnfinally_unclean(&self)->bool{ifself.unstarted.is_some(){self.final_unstarted_is_classified()}else{self.final_scoped_recovery_is_classified()}}",
            // Only an actor the journal never admitted, with nothing in its audit ledger, is released.
            "ifself.record.is_some()||self.flight.is_some()||self.persistence_failure.is_some(){return;}",
            ".is_ok_and(|status|status.pending()==0&&status.in_flight()==0)",
            "ifself.grouped&&!self.group_shutdown{returnOk(false);}ifdeadline.is_some_and(|deadline|Instant::now()>=deadline)||!self.native_audit_ready(){returnOk(false);}",
            "RetainedNativeShutdown::new(&self.owner)?", "shutdown.settle(event)",
            "and_then(RetainedNativeShutdown::next_deadline)",
            "notifications.epoch.snapshot()", "self.poll_shutdown_before(now,Some(deadline))",
            "notifications.epoch.wait_until_changed(epoch,wake_at)",
            "Ok(true)=>returnInstant::now()<deadline", "next.min(deadline)",
            "ifInstant::now()>=deadline{returnfalse;}",
            "self.poll_before(now,deadline)", "lifecycle.drain_until(drain_deadline)",
            "deadline.unwrap_or_else(||Instant::now()+Duration::from_millis(100))",
            "#[cfg(all(test,feature=\"work-execution-probe\"))]",
        ],
        &[
            "pubstruct", "pubfn", "pub(crate)", "Serialize", "Deserialize",
            "AgentNativeShutdownProof", "AgentBrowserShutdownOutcome", "ContextRegistry",
            "std::thread", "tokio::spawn", "InspectablePublic", "AgentBrowserRetention",
            "AgentWorkJournalRequest::Read", "port.dispatch", "invoke_semantic",
            "execute_semantic_action", "attach_successor_work",
        ],
    ),
    (
        "crates/zephium-app/src/work_resources_wait.rs",
        &[
            "generation:Mutex<Option<u64>>", "changed:Condvar", "value.checked_add(1)",
            "self.changed.notify_all()", "self.generation.lock().map_err(|_|Refusal::Uncertain)?",
            "letcurrent=generation.ok_or(Refusal::Uncertain)?", "ifcurrent!=observed",
            "deadline.checked_duration_since(Instant::now())", ".wait_timeout(generation,remaining)",
        ],
        &[
            "pubstruct", "pubfn", "pub(crate)", "Serialize", "Deserialize", "std::thread",
            "tokio::", "AgentBrowserPort", "ContextNativeEvent", "AgentWorkJournal", "CallbackHandle",
        ],
    ),
    (
        "crates/zephium-app/src/work_resources_shutdown.rs",
        &[
            "shared:Arc<Shared>", "coordinator:Option<AgentNativeShutdownCoordinator>",
            "proof:Option<AgentNativeShutdownProof>", "if!owner.locally_retired()",
            "owner.shared.lock_rows()?.begin_native_shutdown()?", "shared:owner.shared.clone()",
            "self.shared.global_current()", "agent_native_shutdown_retry_delay(coordinator.status().attempts())",
            "begin_port_seal(audit)", "self.shared.port.seal_for_shutdown(audit)",
            "begin_resource_audit(audit)", "self.shared.port.audit_resources(audit)",
            "coordinator.settle_shutdown_audit(*settlement)", "coordinator.settle_resource_audit(*settlement)",
            "coordinator.finish()", "self.proof=Some(proof)", "self.failed=true", "returnErr(Box::new(event))",
            "ifcoordinator.status().stage()==AgentNativeShutdownStage::ResourceAuditRequired{self.retry_at=Some(",
            "elseifcoordinator.status().stage()==AgentNativeShutdownStage::Exhausted{",
        ],
        &[
            "pubstruct", "pubfn", "pub(crate)", "Serialize", "Deserialize", "ContextRegistry",
            "WorkBrowserResources::new", "AgentNativeShutdownResources", "ContextNativeResourceSnapshot",
            "AgentWorkJournal", "std::thread", "tokio::spawn", "port.dispatch", "invoke_semantic",
        ],
    ),
    (
        "crates/zephium-app/src/work_resources_controller.rs",
        &[
            "implAgentWorkRetainedBrowserforRetainedBrowser", "browser:LeaseBrowser",
            "binding:WorkBrowserReadBinding",
            "lease:WorkBrowserExecutionLease", "retired:Arc<LeaseRetirement>",
            "self.retired.fail()", "self.browser.refusal()",
            "actors.len()>=MAX_LIVE_CONTEXTS", "actors.push(Arc::downgrade(&listener))",
            "self.pending.swap(false,Ordering::AcqRel)", "self.pending.swap(true,Ordering::AcqRel)",
            "if!self.pending.swap(true,Ordering::AcqRel)&&std::panic::catch_unwind", "std::panic::catch_unwind",
            "read:Option<(PendingRead,SemanticObservationRequest)>", "revoke:Option<PendingLifecycle>",
            "navigation:Option<PendingNavigation>",
            "ifself.navigation.is_some()||self.action.is_some(){returnOk(None);}",
            "Ok(Some(event.into_terminal()))",
            "ticket.register_waker(listener.into())", "Some(ticket)",
            "self.browser.shared.lock_rows()", "revoke_with_delivery(&self.browser.lease)",
            // 7cf84437: reads carry the observation capability.
            "observe_initial_with_capability(&self.browser.lease,capability,now)", "request.observation().clone()",
            ".observe_expansion_with_capability(&self.browser.lease,previous,acknowledgement,target,kind,capability,now,)",
            "Some(LifecycleResult::Delivered(proof))",
        ],
        &["pubstruct", "pubfn", "pub(crate)", "AgentBrowserPort", "ContextRegistry", "AgentNativeShutdownProof", "AgentWorkJournal", "Serialize", "Deserialize", "std::thread", "tokio::spawn", "port.dispatch", "port.invoke_semantic", "port.seal_for_shutdown", "running.swap"],
    ),
    (
        "crates/zephium-app/src/work_resources_navigation.rs",
        &[
            "slot:Arc<Mutex<NavigationOperation>>", "preparation:Option<WorkBrowserNavigationPreparation>",
            "callback:Option<WorkBrowserNavigationCompletionCallback>",
            "preparation.bind(active)", "work_resource_navigate(request,callback)",
            "navigation_dispatch_refused(*request)", "settle_navigation(terminal,now)",
            "slot.flight.abandoned=true", "self.resource.fail()",
        ],
        &["ContextRegistry", "WorkBrowserResources::new", "port.dispatch", "Serialize", "Deserialize", "std::thread", "tokio::spawn", "AgentProviderTransport"],
    ),
    (
        "crates/zephium-agentic/src/work_browser_observation.rs",
        &[
            // 1783cf12/98f3e6c9: two derived facts; the requested document is the admission one.
            "#[derive(Debug)]pubstructWorkBrowserReadBinding{lease:WorkBrowserExecutionLease,frame:SemanticFrameJoin,document:Arc<ContextNavigationTarget>,requested_document:Arc<ContextNavigationTarget>,current_requested_document:Arc<ContextNavigationTarget>,document_policy:crate::WorkBrowserDocumentPolicy,storage:ContextProfileStorageClass,isolated_public:bool,at_admission_document:bool,}",
            "self.admits_lease(lease,now)?;letrow=self.row_mut(lease.resource())?;if!row.document_available||row.navigation.is_some()||row.action.is_some(){returnErr(WorkBrowserResourceError::Pending);}letdocument=row.effective_document.as_ref()",
            "ContextJoin::work_execution(ContextIdentity::new(row.join.identity.context,lease.run,row.join.identity.profile,ContextKind::Owned,)",
            "Ok(WorkBrowserReadBinding{lease:lease.clone(),frame,document:Arc::clone(document),current_requested_document:row.current_requested_document.as_ref().or(row.document.as_ref()).cloned().ok_or(WorkBrowserResourceError::Phase)?,requested_document:row.admission_document.clone().ok_or(WorkBrowserResourceError::Phase)?,document_policy:row.document_policy,storage:row.storage,isolated_public:row.isolated_public,at_admission_document:row.navigation_epoch==row.admission_epoch,})",
            "letbinding=self.read_binding(lease,now)?;letrow=self.row_mut(lease.resource())?;ifrow.observation.is_some()",
            "letframe=binding.frame;letcontext=frame.context();",
            "SemanticObservationId::new(u64::from(sequence))",
            "SemanticInvocationId::new(u64::from(sequence))",
            "*sequence<=MAX_SEMANTIC_RUNTIME_DOCUMENT_INVOCATIONS",
        ],
        &[
            "Serialize", "Deserialize", "AgentBrowserPort", "AgentNativeShutdownProof",
            "pubfnnew(", "pubconstfnnew(", "evaluateJavaScript",
        ],
    ),
    (
        "crates/zephium-app/src/work_resources.rs",
        &[
            "typeNativeFactory=Box<dynFnOnce(NativeSink)->Option<Arc<dynAgentBrowserPort>>>",
            "slots:Mutex<Vec<OwnedSlot>>", "slots.len()>=3", "mpsc::sync_channel(1)",
            "mpsc::sync_channel(2)", "self.pending.swap(true,Ordering::AcqRel)",
            "epoch:wait::NotificationEpoch", "epoch:wait::NotificationEpoch::default()",
            "if!self.epoch.publish(){self.failed.store(true,Ordering::Release);}",
            "self.shared.notifications.pending.swap(false,Ordering::AcqRel)",
            "Arc::downgrade(resource)", "resource.retain(OwnedSlot::Lifecycle(slot.clone()))?",
            "resource.retain(OwnedSlot::Read(slot.clone()))?", "state.flight.rejected(&resource)",
            "self.shared.lock_rows()?.admits_lease(&self.lease,now)?",
            "self.shared.lock_rows()?.revoke_with_delivery(&self.lease)?",
            "self.delivery.is_some()&&(resource.reads.load(Ordering::Acquire)!=0||resource.navigations.load(Ordering::Acquire)!=0||resource.actions.load(Ordering::Acquire)!=0)",
            "self.flight.finish(resource);letproof=ended.join_delivery(receipt)",
            "implDropforWorkResourceOwner", "implDropforLeaseBrowser",
            "implDropforLeaseBrowser{fndrop(&mutself){self.retired.fail();}}",
            "structLeaseRetirement{state:AtomicU8,resource:Weak<Resource>,}",
            "self.state.compare_exchange(Self::ACTIVE,Self::FAILED,Ordering::AcqRel,Ordering::Acquire,)",
            "self.state.compare_exchange(Self::ACTIVE,Self::RETIRED,Ordering::AcqRel,Ordering::Acquire,)",
            "Ok(_)|Err(Self::FAILED)=>Some(LeaseFailure(self))",
            "implDropforLeaseFailure<'_>{fndrop(&mutself){ifletSome(resource)=self.0.resource.upgrade(){resource.fail();}}}",
            "self.retired.check_active()?",
            "retirement.retire()?;}resource.reusable.store(true,Ordering::Release)",
            "fnlocally_retired(&self)->bool",
            "!resource.lock_local(&resource.health)?.reporter_retired()",
            "health.reporter_retired()&&matches!(",
            "self.shared.global_current()&&self.shared.lock_rows()",
            "self.rows.is_poisoned()||self.resources.is_poisoned()",
            "self.resources.lock().map_err(|_|self.refusal())",
            "mutex.lock().map_err(|_|self.refusal())",
            "slots.iter().any(OwnedSlot::is_poisoned)",
            "drop(request.take_resource_health_reporter());}letmutstate=resource.lock_local(&slot)?",
            "Err(TryLockError::WouldBlock)=>false",
            "letmutslot=resource.lock_local(slot)?",
            "Err(mpsc::TryRecvError::Empty)=>failure.map_or(Ok(None),Err)",
        ],
        &[
            "pubstruct", "pubfn", "pub(crate)", "ContextRegistry", "ContextIdentity::",
            "NativeEventSink", "AgentNativeShutdownProof", "AgentBrowserShutdownOutcome",
            "attach_successor_work", "Serialize", "Deserialize", "port.dispatch(",
            "port.invoke_semantic(", "port.execute_semantic_action(", "port.transfer_cookies(",
            "AgentWorkController", "PendingAgentRuntime", "AgentWorkJournal",
            "state.store(Self::ACTIVE",
        ],
    ),
    (
        "crates/zephium-agentic/src/work_browser_health.rs",
        &[
            "resource:WorkBrowserResourceJoin,state:Arc<HealthState>,installed:AtomicBool",
            "resource!=&self.resource", "self.installed.swap(true,Ordering::AcqRel)",
            "self.state.fetch_max(state,Ordering::AcqRel)",
            "self.pending_wake.swap(true,Ordering::AcqRel)",
            "self.state.pending_wake.swap(false,Ordering::AcqRel)",
            "waker:Mutex<Option<Arc<Waker>>>", "std::panic::catch_unwind",
            "self.state.receiver_alive.store(false,Ordering::Release)",
            "self.state.reporter_retired.store(true,Ordering::Release)",
            "UNCERTAIN});self.state.reporter_retired.store(true,Ordering::Release)",
            "self.state.reporter_retired.load(Ordering::Acquire)",
        ],
        &["ContextJoin", "ContextIdentity", "Serialize", "VecDeque", "std::thread", "AgentNativeShutdownProof"],
    ),
    (
        "crates/zephium-agentic/src/work_browser_delivery.rs",
        &[
            "Arc::ptr_eq(&self.0,&other.0)",
            "DeliveryBinding(Arc::new(DeliveryState{state:AtomicU8::new(PENDING)",
            "pubfnregister_waker(", "coordination:Mutex<ListenerRegistration>",
            "self.binding.0.listener.coordination.is_poisoned()",
            "NotificationPhase::Running|NotificationPhase::Completed=>{self.binding.0.listener.fail();Err(WorkBrowserLeaseDeliveryPollError::RegistrationClosed)}",
            "_=>{self.binding.0.listener.fail();Err(WorkBrowserLeaseDeliveryPollError::Notification)}",
            "Err(poisoned)=>{let_registration=poisoned.into_inner();self.binding.0.listener.fail();Err(WorkBrowserLeaseDeliveryPollError::Notification)}",
            "drop(proposed);result",
            "ifregistration.notifier_taken||registration.phase!=NotificationPhase::Pending",
            "ifterminal==RETURNED&&registration.waker.is_some()&&!registration.notifier_taken",
            "ifpublished&&!registration.notifier_taken{",
            "registration.phase=NotificationPhase::Running;registration.waker.clone()};",
            "letreturned=waker.is_none_or(|waker|{std::panic::catch_unwind",
            "ifreturned&&!self.binding.0.listener.failed.load(Ordering::Acquire){registration.phase=NotificationPhase::Completed;true}",
            "self.binding.0.publish(RETURNED)", "self.binding.0.publish(UNPROVEN)",
            "self.binding.0.listener.failed.load(Ordering::Acquire)",
            "pubstructWorkBrowserLeaseDeliveryNotification",
            "PENDING=>Ok(None)",
            "compare_exchange(state,CONSUMED,Ordering::AcqRel,Ordering::Acquire)",
            "(state!=CONSUMED&&state!=ABANDONED).then_some(ABANDONED)",
            "compare_exchange(PENDING,terminal,Ordering::AcqRel,Ordering::Acquire)",
            "compare_exchange(PENDING,UNPROVEN,Ordering::AcqRel,Ordering::Acquire,)",
            "receipt.returned&&self.lease==receipt.lease&&self.delivery.as_ref().is_some_and(|binding|binding.matches(&receipt.binding))",
            "Err(Box::new(WorkBrowserLeaseDeliveryRefusal{ended:self,receipt,}))",
        ],
        &[
            "AgentNativeShutdownProof", "AgentBrowserShutdownOutcome", "Serialize", "Deserialize",
            "std::thread", "tokio::", "Condvar", "FnOnce", "is_clean(",
            "signaled.swap(",
        ],
    ),
    (
        "crates/zephium-engine/src/agent_work_resource_probe_port.rs",
        &[
            "ifself.construction_evidence_claimed.swap(true,Ordering::AcqRel){return;}",
            "letevidence=sample()", "*first=Some(evidence)", "admission.witness_resource(resource)?",
            "letguard=admission.witness_resource(resource)?", "*guard.construction_evidence.lock().ok()?",
            "#[cfg(feature=\"native-agentic-public-resource-probe\")]PublicProductBrief",
            "Self::RenderingFixture=>fixed_fixture(target)", "Self::RenderingFixture=>8", "Self::PublicProductBrief=>1",
            "target.as_url().as_str()==\"https://shop.pimoroni.com/products/raspberry-pi-pico-2\"",
            "url.host_str()==Some(\"127.0.0.1\")", "url.path()==\"/semantic-rendering-v1.html\"",
            "url.query().is_none()", "url.fragment().is_none()", "url.username().is_empty()", "url.password().is_none()",
            "self.resource==next.resource", "self.view==next.view", "self.world==next.world", "self.document==next.document",
            "self.completed.checked_add(1)==Some(next.completed)", "next.invocation>self.invocation",
            "self.admission.witness_resource(&request.resource)", "self.admission.reserve()",
            "completion(self.request.clone(),evidence)", "self.permit.release()",
            "if!state.taken||state.sealed||state.factory.is_some()",
        ],
        &["ContextJoin::", "ContextIdentity::", "#[derive(Serialize", "reqwest::", "evaluateJavaScript"],
    ),
    (
        "crates/zephium-engine/src/host/work_resource_witness.rs",
        &[
            "request.document.admits(target)", "admission.document!=request.document",
            "self.witness_admission.as_mut().is_none_or(Admission::read)",
            "self.samples>=self.document.read_limit()", "self.samples+=1",
            "resource.witness_admission=Some(Admission{document:request.document,samples:0,})",
            "ifrequest.operation==Op::Retire{letstate=resource.retire_witness_state();task.complete(state,None);return;}",
            "Arc::ptr_eq(&resource.guard,&guard)", "self.witness_attempted=true",
            "self.retire_witness_state()==State::Retired", "ifstate==State::Retired",
            "view.semantic()?.witness_identity()?", "view.work_navigation()?.witness_document()?",
            "resource.last_invocation!=0||guard.execution_reserved()",
            "resource.witness=Some(RenderingHolder", "lease.present_resource(guard.resource())",
            "letSome(permit)=guard.notification_permit()", "Duration::from_millis(50)",
            "self.action.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()",
            "queued.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()",
        ],
        &["ContextJoin", "ContextIdentity", "cancel_run(", "evaluateJavaScript", "load_url(", "setActivationPolicy", "activateIgnoringOtherApps", "makeKeyAndOrderFront", "setInactiveSchedulingPolicy"],
    ),
    (
        "crates/zephium-engine/src/platform/macos/agentic_resource_driver.rs",
        &[
            "letForegroundRenderingAdmission(human)=admission", "if!human.is_current()",
            "mpsc::sync_channel(4)", "Duration::from_secs(15)", "Duration::from_secs(5)",
            "[u64;7]=[0,50,100,200,400,800,1600]", "Vec::with_capacity(8)",
            "self.rows.construct_document(", "self.rows.observation_dispatch_refused(*request)",
            "failure:ContextPortFailure::Stale", "old.run()!=lease.run()", "old.resource()==lease.resource()",
            "prior.retained_after_one_read(&stamp)", "self.ended!=2", "self.port.seal_for_shutdown(audit)",
            "!rendering_reply_matches(&self.phase,request)", "*index<2&&request==expected&&request.operation==RenderOp::Inspect",
            "sample_foreground_snapshot(&snapshot,correlation.frame().context(),&origin)",
            "self.rows.is_quiescent()", "self.human.is_current()", "drop(self)",
        ],
        &["ContextIdentity::", "ContextRegistry::", "invoke_semantic(", "ContextNativeRequest::", "evaluateJavaScript", "NSApplication::", "reqwest::", "OPENAI_API_KEY", "std::thread::sleep", "runUntilDate", "activateIgnoringOtherApps", "makeKeyAndOrderFront"],
    ),
    (
        "crates/zephium-agentic/src/work_browser_observation.rs",
        &[
            "self.admits_lease(lease,now)?",
            "row.observation_sequence.checked_add(1)",
            "*sequence<=MAX_SEMANTIC_RUNTIME_DOCUMENT_INVOCATIONS",
            "ContextGeneration::new(lease.generation)",
            "row.observation_sequence=sequence;row.observation=Some(join.clone())",
            "row.observation.as_ref()!=Some(&completion.join)",
            "row.lease.as_ref()!=Some(&completion.join.lease)",
            "now>=completion.join.lease.deadline",
            "SemanticRuntimeSettlement::try_new(self.join.correlation.clone(),outcome)",
            "SemanticObservationBudget::INITIAL_FILTERED",
            "SemanticRuntimeBudget::INITIAL_FILTERED",
        ],
        &[".cancel_run(", "evaluateJavaScript", "ForegroundRendering"],
    ),
    (
        "crates/zephium-engine/src/work_resource_port.rs",
        &[
            "request.resource()!=&self.resource",
            "request.document()!=self.document.as_ref()",
            "request.document_policy()!=self.document_policy",
            "construction_pending:true",
            "state.phase==Phase::Constructing&&state.construction_pending&&!state.uncertain",
            // a866eb38: human handoff delivery is a fourth owned callback debt.
            // ad2ef7cb: an in-flight media delivery also keeps the resource busy.
            "!state.construction_pending&&state.retirement_delivery.is_none()&&state.human_delivery.is_none()&&!state.media_delivery&&state.reads==0&&state.navigation.is_none()&&state.action.is_none()&&state.callbacks==0&&!state.notification_pending",
            "letremove=guard.construction_returned()",
            "self.admission.work_construction_returned(&guard)",
            "state.phase=Phase::Revoking",
            "state.callbacks==0",
            "now<lease.deadline()",
            "state.reads==0&&state.navigation.is_none()&&state.action.is_none()&&state.callbacks==0&&!state.notification_pending",
            "admission.reserve_audit().ok()",
            "state.notification_pending||state.phase==Phase::Destroyed",
            "completion(matcheffective{Some(document)ifoutcome==Outcome::HumanContinued=>{request.complete_human_document(document)}Some(document)=>request.complete_document(document),None=>request.complete(outcome),})",
            "self.guard.read_terminal_begin()",
            "self.guard.read_terminal_end()",
            "self.permit.release()",
            "state.phase==Phase::Retained&&!state.uncertain&&state.lease.is_none()&&state.retirement_delivery.is_none()",
            "state.phase==Phase::Acquiring&&state.retirement_delivery.is_none()",
            "state.lease.is_some()||state.retirement_delivery.is_some()",
            "state.retirement_delivery.as_ref()==Some(lease)",
            "exact&&callback_returned&&permit_released&&port_open&&self.health_current()&&!state.uncertain&&state.phase==Phase::Retained",
            "health:Option<WorkBrowserResourceHealthReporter>,health_permit:Option<AgentTaskPermit>",
            "guard.health=health;guard.health_permit=health_permit",
            "health.is_current(&self.resource)",
            "ifrequest.operation()==Operation::Construct{guard.install_health();}",
            "letpublished=ifretained{delivery.is_none_or(|owner|owner.publish_returned())}else{drop(delivery);false}",
            "drop(state);letnotified=notification.is_none_or(|notification|notification.notify())",
            "if!notified{state.uncertain=true;",
            "ifexact&&permit_released&&state.retirement_delivery.as_ref()==Some(lease){",
            "self.permit.release();ifmedia{self.guard.finish_media_delivery(callback_returned,self.permit.released&&self.permit.admission.counts().is_some(),);}ifletSome(operation)=human{self.guard.finish_human_delivery(","ifletSome(lease)=&revocation{self.guard.finish_revocation_delivery(",
            "self.permit.released&&self.permit.admission.counts().is_some()",
            "(construction||revocation.is_some()||human.is_some()||media)&&self.guard.destruction_started()",
            "Arc::ptr_eq(current,guard)",
            "ingress.rows.len()>=MAX_LIVE_CONTEXTS",
            ">=zephium_agentic::MAX_EXECUTING_CONTEXTS",
        ],
        &[
            "ContextIdentity::new(",
            "ContextJoin::",
            "ContextRegistry",
            "cancel_run(",
            "ForegroundRendering",
        ],
    ),
    (
        "crates/zephium-engine/src/host/work_resource.rs",
        &[
            // 88d35dc0: one slow-page retry, clamped to the original deadline (pinned in validate).
            "letoriginal_deadline=guard.construction_deadline(Instant::now());",
            "iforiginal_deadline.is_none_or(|deadline|deadline<=Instant::now()){task.complete(Outcome::Refused);return;}",
            "resource.record_construction_failure(\"construction_deadline\")",
            "resource.record_construction_failure(\"navigation_gate\")",
            "resource.record_construction_failure(\"native_health\")",
            "self.guard.record_construction_evidence(||{",
            "witness_admission:Option<witness::Admission>",
            "self.agent_contexts.len()+self.work_resources.len()>=MAX_LIVE_CONTEXTS",
            "execution_count<=MAX_EXECUTING_CONTEXTS",
            "guard.acquisition_current(lease,now)",
            "if!guard.construction_current()",
            "gate.arm_with_policy(document.clone(),guard.document_policy())",
            "gate.finalize_after_quiet_period(ticket,||{crate::platform::imp::current_url(view.view())})",
            "DocumentFinalizationProgress::Ready(effective)ifguard.construction_current()",
            "task.complete_document(effective)",
            "operation==Operation::Destroy&&!self.work_resources.contains_key(&id)&&!guard.callbacks_drained()",
            "WorkNativeResource::unconstructed(guard.clone(),reservation)",
            "ifresource.prepare_destruction()",
            // 08219870/64d6c7b9: screenshots and native Back are owned debts too.
            "self.retire_construction();self.cancel_observation(SemanticRuntimePortFailure::Shutdown);self.cancel_action();",
            "screenshot.cancelled.store(true,Ordering::Release);",
            "ifletSome(navigation)=self.navigation.take(){navigation.refuse(ContextPortFailure::Shutdown);}ifletSome(history)=self.history_back.take(){history.refuse(ContextPortFailure::Shutdown);}ifletSome(task)=self.revocation.take(){task.complete(Outcome::Refused);}self.retire_page();self.destruction_drained()",
            // Shared retirement still owes every physical callback. Windows
            // additionally retains native storage, frame and controller-close debt.
            "self.retirement_clean&&self.view.is_none()&&self.guard.callbacks_drained()&&!self.frame_in_flight.load(Ordering::Acquire)&&self.observation.is_none()&&self.action.is_none()&&self.navigation.is_none()&&!self.history_back_pending()",
            "#[cfg(target_os=\"macos\")]{self.screenshot.is_none()}",
            "#[cfg(target_os=\"windows\")]if!view.cancel_work_storage()||!view.work_native_activity_drained()||self.frame_in_flight.load(Ordering::Acquire){returnfalse;}",
            "letnative_profile=view.work_native_profile().unwrap_or(self.guard.resource().identity().profile());",
            "ifletErr(debt)=view.close(){letdebt=super::OwnedWindowsCleanupDebt::new(debt,self.native_resource.take());super::queue_windows_cleanup_debt(native_profile,debt);self.retirement_clean=false;self.guard.fail();}",
            "!self.erasure_tombstones.contains(&resource.profile())",
            "native_resource.reclassify(NativeResourceClass::AgentContext)",
            "retirement_clean:false",
            "crate::platform::imp::build_owned_work_view(",
            "letstore=ifguard.work_website_data(){Some(self.work_data_store(profile)?)}",
            "!=Some(&super::profiles::ProfilePersistenceClass::Durable)",
            "resource_retained:true",
            "guard.lease_drained(lease)",
            "guard.callbacks_drained()",
            "resource.observation.is_none()",
            "view.semantic_pending_for_audit()==Some(false)",
            "Arc::ptr_eq(&resource.guard,&guard)",
            "request.operation()==self.operation&&request.lease()==self.lease.as_ref()",
            "resource.construction.as_ref().is_some_and(|task|deadline.matches(task))",
            "resource.revocation.as_ref().is_some_and(|task|deadline.matches(task))",
            "resource.destruction.as_ref().is_some_and(|task|deadline.matches(task))",
        ],
        &[
            "ContextJoin",
            "view_origin",
            "cancel_run(",
            "cancel_semantic(",
            "AgentOwnedContext",
            "ContextNativePermit",
            "dispatch_action(",
            "evaluateJavaScript",
            "ForegroundRendering",
            "probe_",
        ],
    ),
    (
        "crates/zephium-engine/src/platform/work_document_navigation.rs",
        &[
            "canonical_equal:expected==&actual", "raw_equal:expected.as_str()==current",
            "query_equal:expected.query()==actual.query()", "fragment_equal:expected.fragment()==actual.fragment()",
            "state.evidence.location_callback_after_commit_before_ready=true",
            "state.phase!=Phase::Bootstrap||!state.bootstrap_finished||state.target.is_some()",
            "expected.as_url().as_str()==target",
            "state.native_id==Some(event.id)",
            "Some(target.as_url().as_str())==current",
            "ifletOk(mutstate)=self.0.lock(){state.clear_write_navigation();state.phase=Phase::Refused;}",
            "state.phase=Phase::Retired",
            "state.phase=Phase::Sampling",
            "state.location_revision!=revision",
            "target.as_url().as_str()==raw",
            "state.policy.admits_final_document(requested,effective)",
            "state.effective=Some(effective.clone())",
            "state.phase!=Phase::Ready||state.operation.is_some()||state.native_id.is_none()",
            "state.navigation_epoch!=source.navigation_epoch().get()",
            "source.identity()!=next.identity()",
            "source.context_generation()!=next.context_generation()",
            "source.cancellation_generation()!=next.cancellation_generation()",
            "source.navigation_epoch().get().checked_add(1)!=Some(next.navigation_epoch().get())",
            "source.frame_generation().get().checked_add(1)!=Some(next.frame_generation().get())",
            "request.redirect_policy().is_some()",
            "letoperation=state.operation.take()?",
            "state.navigation_epoch=operation.context().navigation_epoch().get()",
        ],
        &[
            "ContextRunId",
            "ContextJoin::",
            "ContextRegistry",
            "AgentNavigationOperation",
            "evaluateJavaScript",
        ],
    ),
    (
        "crates/zephium-engine/src/agent_context_port.rs",
        &[
            "if!self.work_is_absent(){return;}",
            "if!self.work_is_absent(){returnErr(ContextPortFailure::ProfileBusy);}",
            "self.schedule_work_lifecycle(request,completion)",
            "self.schedule_work_observation(request,completion)",
            "self.schedule_work_navigation(request,completion)",
        ],
        &[],
    ),
    (
        "crates/zephium-engine/src/host/agent_context.rs",
        &[
            "checked_add(self.work_resources.len())",
            ".filter(|resource|resource.resident())",
            ".filter(|resource|resource.pending())",
            "task.work_ingress_matches(",
            ".any(|resource|resource.profile()==profile)",
            "self.force_shutdown_work_resources()&&self.agent_contexts.is_empty()",
        ],
        &[],
    ),
    (
        "crates/zephium-engine/src/host/content_rules.rs",
        &[
            "self.work_resources",
            "resource.replace_content_policy_registration(registration)",
        ],
        &[],
    ),
    (
        "crates/zephium-engine/src/platform/macos/mod.rs",
        &[
            "pubfncurrent_url(view:&wry::WebView)->Option<String>{bounded_current_url(view).ok()}",
            "PAGE_URL_UTF16_LIMIT:usize=8*1_024", "PAGE_URL_UTF8_LIMIT:usize=8*1_024",
            "value.length()>PAGE_URL_UTF16_LIMIT", "value.len()<=PAGE_URL_UTF8_LIMIT",
            "Ok(current)=>(gate.ready(Some(&current)),E::compare(expected,&current))",
            "Err(CurrentUrlUnavailable::MissingNativeUrl)=>(false,E::MissingNativeUrl)",
            "Err(CurrentUrlUnavailable::MissingAbsoluteString)=>(false,E::MissingAbsoluteString)",
            "Err(CurrentUrlUnavailable::Utf16Limit)=>(false,E::Utf16Limit)",
            "Err(CurrentUrlUnavailable::Utf8Limit)=>(false,E::Utf8Limit)",
        ],
        &[],
    ),
];

// Admits the Shell's read-group handle (385c39c2): creation and the failed and
// sealed flags cross the crate; its journal and runtime stay private.
const GROUP_HANDLE: [&str; 5] = [
    "pub(crate) struct RetainedWorkGroup {",
    "pub(crate) fn try_new(work: WorkId, capacity: u8) -> Result<Self, RuntimeSpawnError> {",
    "pub(crate) fn is_failed(&self) -> bool {",
    "pub(crate) fn seal(&self) {",
    "pub(crate) fn is_sealed(&self) -> bool {",
];

// Admits the retained adapter's private image broker (ad2ef7cb): it holds the
// port but answers only the two lease-bound screenshot-source calls.
const IMAGE_BROKER: [&str; 4] = [
    "struct RetainedImageSource {\n    port: Arc<dyn AgentBrowserPort>,\n    lease: WorkBrowserExecutionLease,\n}",
    "self.port\n            .work_screenshot_public_candidate(&self.lease, context)",
    "self.port.claim_work_screenshot_source(&self.lease, image)",
    "port: self.browser.shared.port.clone(),",
];

fn adapter_source(path: &str, source: String) -> Result<String, String> {
    if path.ends_with("work_resources_application.rs") {
        admit_group_handle(&source)
    } else if path.ends_with("work_resources_controller.rs") {
        admit_image_broker(&source)
    } else {
        Ok(source)
    }
}

fn admit_image_broker(source: &str) -> Result<String, String> {
    let mut source = source.to_owned();
    for item in IMAGE_BROKER {
        if source.matches(item).count() != 1 {
            return Err(format!("Work retained image broker drifted: {item}"));
        }
        source = source.replacen(item, "", 1);
    }
    if source.contains("self.port") {
        return Err(
            "Work retained image broker reaches its port beyond the two reviewed calls".into(),
        );
    }
    Ok(source)
}

fn admit_group_handle(source: &str) -> Result<String, String> {
    let mut source = source.to_owned();
    for item in GROUP_HANDLE {
        if source.matches(item).count() != 1 {
            return Err(format!("Work read-group handle drifted: {item}"));
        }
        source = source.replacen(item, &item["pub(crate) ".len()..], 1);
    }
    Ok(source)
}

fn validate_adapter(source: &str, required: &[&str], forbidden: &[&str]) -> Result<(), String> {
    let source = compact(production(source));
    for needle in required {
        require(&source, needle)?;
    }
    for needle in forbidden {
        if source.contains(needle) {
            return Err(format!("Work native boundary forbids {needle}"));
        }
    }
    Ok(())
}

fn production(source: &str) -> &str {
    source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production)
}

fn compact(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}
fn require(source: &str, needle: &str) -> Result<(), String> {
    if source.contains(needle) {
        Ok(())
    } else {
        Err(format!("Work resource boundary missing {needle}"))
    }
}
fn validate(source: &str, port: &str) -> Result<(), String> {
    let source = compact(source);
    let port = compact(port);
    require(&source, "pubstructWorkBrowserResourceIdentity{work:WorkId,resource:WorkBrowserResourceId,profile:ProfileId,context:ContextId,}")?;
    require(
        &source,
        "std::time::Duration::from_secs(matchself{Self::Initial=>30,Self::SlowPageRetry=>60,})",
    )?;
    for needle in [
        "Arc::ptr_eq(&self.0,&other.0)",
        "self.sequence.checked_add(1)",
        "self.rows.len()>=MAX_LIVE_CONTEXTS",
        "row.lease.is_some()).count()>=MAX_EXECUTING_CONTEXTS",
        "row.phase=WorkBrowserResourcePhase::Revoking;row.pending=Some(operation.clone())",
        "row.destruction_attempted=true;row.destruction=Some(operation.clone())",
        "sequence:lease.generation",
        "sequence:resource.incarnation",
        "ifsealed||now>=lease.deadline",
        "WorkBrowserResourceEvent::RevocationRequired(lease.clone())",
        "ifdebt.bounded()&&debt.is_empty()&&resource_retained",
        "resource_retained&&row.observation.is_none()",
        "row.pending.is_none()&&row.destruction.is_none()&&row.lease.is_none()",
        "row.pending.as_ref()!=Some(&completion.operation)",
        "row.document_policy.admits_final_document(requested,effective)",
        "row.effective_document=effective",
        "row.destruction.as_ref()==Some(&completion.operation)",
        "WorkBrowserResourceEvent::DebtSettled(row.join.clone())",
        "request:Box<WorkBrowserResourceRequest>",
        "pub(crate)structWorkBrowserNativeShutdownAdmission{_authority:Authority,}",
        "ifself.native_shutdown_started{returnErr(WorkBrowserResourceError::Sealed);}",
        "if!self.is_quiescent(){returnErr(WorkBrowserResourceError::Phase);}",
        "self.native_shutdown_started=true",
        "WorkBrowserNativeShutdownAdmission{_authority:self.authority.clone(),}",
    ] {
        require(&source, needle)?;
    }
    for forbidden in [
        "ContextIdentity::new(",
        "AgentNativeShutdownProof",
        "std::thread",
        "tokio::",
        "ContextNativeResourceSnapshot",
        "cancel_run(",
        "begin_document_load(",
        "evaluateJavaScript",
        "SEMANTIC_RUNTIME_CHANNEL_STOP",
    ] {
        if source.contains(forbidden) {
            return Err(format!("Work resource boundary forbids {forbidden}"));
        }
    }
    require(&port, "fnwork_resource_lifecycle(&self,request:crate::WorkBrowserResourceRequest,_completion:crate::WorkBrowserResourceCompletionCallback,)->crate::WorkBrowserResourceDispatch{crate::WorkBrowserResourceDispatch::Rejected{request:Box::new(request),failure:ContextPortFailure::Unsupported,}}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = include_str!("../../crates/zephium-agentic/src/work_browser_resource.rs");
    const PORT: &str = include_str!("../../crates/zephium-agentic/src/context_port.rs");
    #[test]
    fn document_refusal_clears_one_shot_write_authority_before_sealing() {
        let source =
            include_str!("../../crates/zephium-engine/src/platform/work_document_navigation.rs");
        let (_, required, forbidden) = ADAPTER_RULES
            .iter()
            .find(|(path, _, _)| path.ends_with("platform/work_document_navigation.rs"))
            .unwrap();
        validate_adapter(source, required, forbidden).unwrap();
        let source = compact(production(source));
        let guarded = "ifletOk(mutstate)=self.0.lock(){state.clear_write_navigation();state.phase=Phase::Refused;}";
        for replacement in [
            "ifletOk(mutstate)=self.0.lock(){state.phase=Phase::Refused;}",
            "ifletOk(mutstate)=self.0.lock(){state.clear_write_navigation();}",
            "ifletOk(mutstate)=self.0.lock(){state.phase=Phase::Refused;state.clear_write_navigation();}",
        ] {
            assert!(source.contains(guarded));
            assert!(validate_adapter(&source.replace(guarded, replacement), required, forbidden).is_err());
        }
    }
    #[test]
    fn url_revocation_cannot_be_moved_after_host_failure_notification() {
        let source =
            include_str!("../../crates/zephium-engine/src/platform/macos/agent_context.rs");
        validate_url_revocation_order(source).unwrap();
        let source = compact(production(source));
        let close = "work_location_semantic.revoke_document_authority();";
        let changed = source.replace(close, "").replace(
            "invoke_owned_unit_callback(location_invariant.as_ref(),location_panic.as_ref(),);",
            &format!("invoke_owned_unit_callback(location_invariant.as_ref(),location_panic.as_ref(),);{close}"),
        );
        assert!(validate_url_revocation_order(&changed).is_err());
    }
    #[test]
    fn passive_action_lifetime_cannot_depend_on_queued_host_terminal_or_extend_budget() {
        let source = include_str!("../../crates/zephium-engine/src/host/work_resource_action.rs");
        let (_, required, forbidden) = ADAPTER_RULES
            .iter()
            .find(|(path, _, _)| path.ends_with("host/work_resource_action.rs"))
            .unwrap();
        validate_adapter(source, required, forbidden).unwrap();
        let source = compact(production(source));
        for changed in [
            source.replace(
                "(runtime_pending||runtime_settling)",
                "(runtime_pending||(self.terminal.is_some()&&runtime_settling))",
            ),
            source.replace("!self.cancelled&&self.dispatched", "self.dispatched"),
            source.replace("Duration::from_secs(3)", "Duration::from_secs(30)"),
            source.replace(
                "deadline.checked_sub(RETIREMENT_MARGIN)",
                "deadline.checked_add(RETIREMENT_MARGIN)",
            ),
            source.replace("semantic.revoked_settling_action(action.attempt)", "true"),
        ] {
            assert!(validate_adapter(&changed, required, forbidden).is_err());
        }
    }
    #[test]
    fn fixed_document_preflight_precedes_native_or_credential_activity() {
        let source =
            include_str!("../../crates/zephium-work-composition/src/retained_qualification.rs");
        validate_preflight_order(source).unwrap();
        let source = compact(production(source));
        let without = source.replace("let(fixture,target)=task::document()?;", "");
        assert!(validate_preflight_order(&format!(
            "{without}let(fixture,target)=task::document()?;"
        ))
        .is_err());
    }
    #[test]
    fn real_public_brief_cannot_widen_target_or_drop_exact_source_guards() {
        let source = include_str!(
            "../../crates/zephium-work-composition/src/retained_qualification_public.rs"
        );
        let (_, required, forbidden) = ADAPTER_RULES
            .iter()
            .find(|(path, _, _)| path.ends_with("retained_qualification_public.rs"))
            .unwrap();
        validate_adapter(source, required, forbidden).unwrap();
        let source = compact(production(source));
        for changed in [
            source.replace(
                "https://shop.pimoroni.com/products/raspberry-pi-pico-2",
                "https://example.test/other",
            ),
            source.replace(
                "fragment.provenance().invocation()!=expected.invocation",
                "false",
            ),
            source.replace("source.snapshot==expected.snapshot", "true"),
            source.replace("fragment.field()!=source_field", "false"),
            source.replace("matched!=[true;2]", "false"),
            source.replace("sources.next().is_none()", "true"),
            format!("{source}fn allows_actions_before_extraction()->bool{{true}}"),
        ] {
            assert!(validate_adapter(&changed, required, forbidden).is_err());
        }
    }
    #[test]
    fn resource_boundary_rejects_run_ownership_false_drain_and_probe_authority() {
        validate(SOURCE, PORT).unwrap();
        let source = compact(production(SOURCE));
        for changed in [
            source.replace("work:WorkId,", "work:ContextRunId,"),
            source.replace("Arc::ptr_eq(&self.0,&other.0)", "true"),
            source.replace("debt.is_empty()&&resource_retained", "true"),
            source.replace("sealed||now>=lease.deadline", "false"),
            source.replace("row.destruction.is_none()", "true"),
            source.replace("sequence:lease.generation", "sequence:self.next()?"),
            source.replace(
                "self.native_shutdown_started=true",
                "self.native_shutdown_started=false",
            ),
            source.replace("if!self.is_quiescent()", "iffalse"),
            format!("{source}\nlet fake = AgentNativeShutdownProof;"),
            format!("{source}\ncancel_run();"),
        ] {
            assert!(validate(&changed, PORT).is_err());
        }
        assert!(validate(
            SOURCE,
            &PORT.replace(
                "failure: ContextPortFailure::Unsupported,",
                "failure: ContextPortFailure::Cancelled,"
            )
        )
        .is_err());
    }
    #[test]
    fn native_boundary_rejects_missing_ownership_guards_and_legacy_authority() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for (path, required, forbidden) in ADAPTER_RULES {
            let original = std::fs::read_to_string(root.join(path)).unwrap();
            let original = adapter_source(path, original).unwrap();
            let source = compact(production(&original));
            validate_adapter(&source, required, forbidden)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            for needle in *required {
                let changed = source.replace(needle, "false");
                assert!(
                    validate_adapter(&changed, required, forbidden).is_err(),
                    "{path}: {needle}"
                );
            }
            for needle in *forbidden {
                assert!(
                    validate_adapter(&format!("{source}{needle}"), required, forbidden).is_err(),
                    "{path}: {needle}"
                );
            }
        }
    }
}
