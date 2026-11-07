//! Shipping Work composition and retention-only qualification separation.
use std::{fs, path::Path};

fn compact(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}
fn require(source: &str, tokens: &[&str]) -> Result<(), String> {
    let source = compact(source);
    for token in tokens {
        if !source.contains(&compact(token)) {
            return Err(format!("Work composition lost {token}"));
        }
    }
    Ok(())
}

fn production(source: &str) -> Result<(), String> {
    for token in [
        "println!",
        "eprintln!",
        "dbg!",
        "diagnostic!",
        "log::",
        "tracing::",
        "serde_json",
        "try_new_for_probe",
        "InspectablePublic",
        "evaluate_javascript",
        "impl AgentBrowserPort",
        "AgentRunPolicy::",
        "AgentRunSupervisor::",
        "SemanticEffectClass::",
        "AgentAccountScope::",
        "wikipedia",
        "tokio::spawn",
        "thread::spawn",
        "#[tauri::command]",
    ] {
        if source.contains(token) {
            return Err(format!(
                "Work composition acquired forbidden authority/content: {token}"
            ));
        }
    }
    Ok(())
}

fn preparation_precedes_attachment(source: &str) -> Result<(), String> {
    let source = compact(source);
    let prepared = source
        .find("let(composition,prepared)=prepare_owned(")
        .ok_or("desktop lost preparation-before-consumption")?;
    let attached = source
        .find("composition.attach(&shell.callback_handle())")
        .ok_or("desktop lost exact attachment")?;
    if prepared >= attached {
        return Err("desktop attachment precedes successful preparation".into());
    }
    Ok(())
}

fn ordinary_navigation_observer(source: &str) -> Result<(), String> {
    require(
        source,
        &[
            "let request = qualifier::load_request(started, profile)?;",
            "let profile = wait_for_profile(app, control, started, None)?;",
            "wait_for_profile(app, control, started, Some(profile))?;",
            "control::profile_wait_failure(started, Instant::now(), control.admission.cancelled())",
            ".admission.admit(|| {",
            "#[cfg(not(feature = \"macos-work-retained-product-probe\"))] let admit = super::admit_trusted_work;",
            "#[cfg(feature = \"macos-work-retained-product-probe\")] let admit = super::admit_retained_trusted_work;",
            "let view = admit(app, request)",
            "Some((view.clone(), false))",
            "let settled = control.admission.settle(result);",
            "if state.control.admission.cancel() { request_stop(&state.control); api.prevent_exit();",
            "let accepted = matches!(state.control.admission.terminal(), Some(Ok(report)) if report.accepted);",
            "worker.join().is_ok()",
            "let qualified = accepted && joined && normal_shutdown_clean;",
            "!owner.terminal_failure.load(Ordering::Acquire)",
            "qualifier::cancel(view)",
            "let mut observer = ApplicationObserver::default();",
        ],
    )?;
    for forbidden in [
        "AgentWorkController::",
        "MacosWorkComposition::",
        "NativeWorkComposition::",
        "WebviewEngine::",
        "SqliteStore::",
        "spawn_suspended",
        "prepare_public_qualification",
        "shutdown_with_deadline",
        "set_focus(",
        "activate(",
        "process::exit(",
        "#[tauri::command]",
        "impl AgentBrowserPort",
        "ready_for_shutdown",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "navigation observer acquired another owner/authority: {forbidden}"
            ));
        }
    }
    Ok(())
}

fn inspection_composition(retained: &str, discovery: &str, observer: &str) -> Result<(), String> {
    for wrapper in [retained, discovery] {
        let production = wrapper.split("\n#[cfg(test)]").next().unwrap_or(wrapper);
        require(production, &[
            "fn allows_progressive_observation(&self) -> bool { self.0.allows_progressive_observation() }",
        ])?;
    }
    let retained = retained.split("\n#[cfg(test)]").next().unwrap_or(retained);
    let observer = observer.split("\n#[cfg(test)]").next().unwrap_or(observer);
    for source in [retained, observer] {
        require(source, &[
            "self.observe_kind(event.kind());",
            "AgentBrowserToolKind::Read | AgentBrowserToolKind::Locate | AgentBrowserToolKind::Snapshot,",
            "AgentWorkEventKind::ToolProposed(_)",
            "AgentWorkEventKind::ActionActive",
            "AgentWorkEventKind::Recovery => self.failed = true",
        ])?;
    }
    require(retained, &[") if DEFINITION.inspection => {}"])?;
    require(observer, &[") if self.definition.inspection => {}"])?;
    Ok(())
}

fn commerce_selection(desktop: &str, composition: &str, retained: &str) -> Result<(), String> {
    // Specialization must inherit the exact debug/isolated/retained entry and
    // public-retention gates, not introduce an independent launcher or runtime.
    require(desktop, &["macos-work-retained-commerce-probe = [\"macos-work-retained-product-probe\", \"zephium-work-composition/retained-commerce-qualification\"]"])?;
    require(
        composition,
        &["retained-commerce-qualification = [\"retained-product-qualification\"]"],
    )?;
    require(retained, &[
        "#[cfg(feature = \"retained-commerce-qualification\")] #[path = \"retained_commerce_objective.rs\"] mod objective;",
        "#[cfg(not(feature = \"retained-commerce-qualification\"))] #[path = \"retained_svelte_objective.rs\"] mod objective;",
        "use objective::{DOCUMENT_POLICY, INITIAL, ORIGIN, PATH_PREFIX, TASK_NAME};",
        "initial: INITIAL, document_policy: DOCUMENT_POLICY, origin: ORIGIN, task_name: TASK_NAME,",
        "ContextNavigationTarget::parse(INITIAL)",
        "PATH_PREFIX.into()",
    ])
}

pub(crate) fn check(root: &Path) -> Result<(), String> {
    let read = |path: &str| {
        fs::read_to_string(root.join(path))
            .map_err(|_| format!("missing Work composition source {path}"))
    };
    let native = read("crates/zephium-work-composition/src/native.rs")?;
    require(
        &native,
        &[
            "pub struct NativeWorkComposition",
            "impl NativeWorkComposition",
            "let binding = request.browser_profile.ok_or(AgentWorkFailure::Contract)?;",
            "prepared.with_browser_profile(binding)",
        ],
    )?;
    require(&read("crates/zephium-app/src/work.rs")?, &[
        "self.controller.profile_storage_binding()? != (binding.profile(), binding.storage_class())",
        "profile == Some(crate::AgentWorkProfileReadiness::Ready(binding))",
        "if !profile_valid { self.stopping = true; self.fail(crate::contract_failure!()); self.abort_staged(); }",
    ])?;
    require(&read("crates/zephium-app/src/shell/mod.rs")?, &[
        "let profile = self.work_profile_binding(); if let Some(work) = &mut self.work { work.admit(submission, Some(profile)); }",
    ])?;
    ordinary_navigation_observer(&read("desktop/src/navigation_probe.rs")?)?;
    commerce_selection(
        &read("desktop/Cargo.toml")?,
        &read("crates/zephium-work-composition/Cargo.toml")?,
        &read("crates/zephium-work-composition/src/retained_product_qualification.rs")?,
    )?;
    inspection_composition(
        &read("crates/zephium-work-composition/src/retained_product_qualification.rs")?,
        &read("crates/zephium-work-composition/src/discovery_qualification.rs")?,
        &read("crates/zephium-work-composition/src/navigation_qualification_observer.rs")?,
    )?;
    require(&read("desktop/Cargo.toml")?, &[
        "macos-work-navigation-probe = [\"macos-work\", \"zephium-work-composition/navigation-qualification\", \"tauri/custom-protocol\"]",
    ])?;
    require(
        &read("desktop/build.rs")?,
        &[
            "env::var_os(\"CARGO_FEATURE_MACOS_WORK_NAVIGATION_PROBE\").is_some()",
            "navigation_probe_config::validate(",
        ],
    )?;
    require(&read("desktop/src/lib.rs")?, &[
        "#[cfg(all(feature = \"macos-work-navigation-probe\", any(not(debug_assertions), not(target_os = \"macos\"))))] compile_error!",
        "#[cfg(all(feature = \"macos-work-navigation-probe\", not(feature = \"macos-work-profile-enrollment\"), target_os = \"macos\"))] navigation_probe::install(app.handle())?;",
    ])?;
    production(&native)?;
    require(
        &native,
        &[
            "engine: Arc<WebviewEngine>",
            "store: Arc<SqliteStore>",
            "task: Box<dyn AgentWorkTask>",
            "shell.attach_work(self.store.clone(), self.engine.clone())",
            "pub fn prepare(&self, request: TrustedWorkRequest",
            "PreparedAgentWork::try_new(",
            "AgentWorkApplicationPorts::new(engine.clone(), self.store.clone(), Box::new(move |sink|",
            "native: Arc<Mutex<NativeLifetimeOwner>>",
            "NativeLifetimeOwner::Dormant",
            "engine.take_agent_browser_lifetime_factory()",
            "let NativeLifetimeOwner::Factory(factory) = &mut *native else { return None; };",
            "factory.begin(move |event|",
            "shell.attach_successor_work(self.store.clone(), self.engine.clone(), predecessor)",
        ],
    )?;
    if native
        .matches("take_agent_browser_lifetime_factory(")
        .count()
        != 1
        || native.contains("take_agent_browser_port(")
    {
        return Err("native factory must have one exact take site".into());
    }
    let desktop = read("desktop/src/work.rs")?;
    production(&desktop)?;
    require(
        &desktop,
        &[
            "MacosWorkComposition::new(engine, store)",
            "pub fn admit_trusted_work(",
            "pub fn admit_successor_trusted_work(",
            "if predecessor.is_some() != owner.initial_attached",
            "composition.attach_successor(&shell.callback_handle(), predecessor)",
            "owner.composition = Some(composition)",
            "composition.prepare(request)",
            "view.admit(prepared)",
            "Mailbox { prepared, handle: view, }",
            "AttachmentMailbox { prepared: Box::new(prepared), composition, }",
            "let prepared = prepare(composition).map_err(PreparationFailure::Contract)?; let composition = slot.take()",
        ],
    )?;
    preparation_precedes_attachment(&desktop)?;
    require(&read("desktop/src/lib.rs")?, &["#[cfg(feature = \"macos-work\")] mod work;", "#[cfg(feature = \"macos-work\")] if !work::install(app.handle(), engine.clone(), store.clone())"])?;
    require(&read("desktop/Cargo.toml")?, &["macos-work = [\"dep:zephium-work-composition\", \"zephium-work-composition/macos-work\"]", "zephium-work-composition = { workspace = true, optional = true }"])?;
    let manifest = read("crates/zephium-work-composition/Cargo.toml")?;
    require(
        &read("crates/zephium-engine/Cargo.toml")?,
        &[
            "native-agentic-public-resource-probe = [\"native-agentic-work-resource-probe\"]",
            "native-agentic-work-resource-probe = [\"native-agentic-foreground-probe\"]",
        ],
    )?;
    require(
        &manifest,
        &[
            "default = []",
            "native-work = [\"dep:zephium-app\", \"dep:zephium-engine\", \"dep:zephium-store\", \"dep:zephium-agent-controller\", \"dep:zephium-agent-provider-transport\", \"dep:zephium-agentic\"]",
            "macos-work = [\"native-work\"]",
            "navigation-qualification = [\"macos-work\", \"dep:zephium-agentic\", \"dep:zephium-core\", \"dep:zephium-agent-runtime\"]",
            "public-qualification = [\"macos-work\", \"zephium-app/work-execution-probe\", \"zephium-agent-provider-transport/probe-harness\",]",
            "retained-public-qualification = [\"retained-qualification\", \"zephium-engine/native-agentic-public-resource-probe\"]",
        ],
    )?;
    let parsed: toml::Value =
        toml::from_str(&manifest).map_err(|_| "invalid composition manifest")?;
    let dependencies = parsed
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or("missing composition dependencies")?;
    if dependencies
        .values()
        .any(|dep| dep.get("optional").and_then(toml::Value::as_bool) != Some(true))
    {
        return Err("composition dependencies must stay dormant".into());
    }
    require(
        &read("crates/zephium-work-composition/src/lib.rs")?,
        &[
            "#[cfg(feature = \"native-work\")] mod native;",
            "#[cfg(feature = \"native-work\")] pub use native::{NativeWorkComposition, TrustedWorkRequest};",
            "#[cfg(feature = \"macos-work\")] pub use native::NativeWorkComposition as MacosWorkComposition;",
            "#[cfg(all(feature = \"native-work\", not(any(target_os = \"macos\", target_os = \"windows\"))))] compile_error!",
            "#[cfg(feature = \"public-qualification\")] mod qualification;",
            "#[cfg(all(feature = \"navigation-qualification\", not(debug_assertions)))] compile_error!",
            "#[cfg(feature = \"navigation-qualification\")] #[doc(hidden)] pub mod navigation_qualification;",
            "#[cfg(all(feature = \"public-qualification\", not(debug_assertions)))] compile_error!",
        ],
    )?;
    require(
        &read("crates/zephium-app/src/work.rs")?,
        &[
            "#[cfg(feature = \"work-execution-probe\")] #[path = \"work_probe.rs\"] mod probe;",
            "!Arc::ptr_eq(&run.engine, &self.engine)",
            "Arc::ptr_eq(&self.engine, engine)",
            "if deadline <= Instant::now() { return Err(AgentWorkFailure::Deadline); }",
        ],
    )?;
    require(
        &read("crates/zephium-app/src/shell/mod.rs")?,
        &[
            "!work.belongs_to_engine(&self.engine)",
            "!work.accepts_predecessor(self.work.as_deref())",
            "previous.retire_projection()",
        ],
    )?;
    require(&read("crates/zephium-app/Cargo.toml")?, &["work-execution-probe = [\"work-execution\", \"zephium-agent-controller/probe-harness\"]"])?;
    require(
        &read("crates/zephium-app/src/work_probe.rs")?,
        &[
            "#[cfg(not(debug_assertions))] compile_error!",
            "AgentBrowserRetention::InspectablePublicData",
            "Self::from_controller(controller, handle, config.runtime, ports)",
        ],
    )?;
    let qualifier = read("crates/zephium-terra-macos-probe/src/work_application.rs")?;
    combined_result_qualification(&qualifier)?;
    require(
        &qualifier,
        &[
            "zephium_app::spawn_suspended(",
            "MacosWorkComposition::new(engine.clone(), store.clone())",
            "composition.prepare_public_qualification(request)",
            "shell.shutdown_with_deadline(",
            "AgentWorkApplicationPhase::Succeeded",
            "ShutdownOutcome::Clean",
        ],
    )?;
    for token in [
        "impl AgentBrowserPort",
        "AgentWorkController::",
        "PendingAgentRuntime",
        "take_agent_browser_port",
    ] {
        if qualifier.contains(token) {
            return Err(format!(
                "application qualifier bypasses composition: {token}"
            ));
        }
    }
    Ok(())
}

fn combined_result_qualification(source: &str) -> Result<(), String> {
    require(
        source,
        &[
            "super::work_actor::combined_input(started)?",
            "effects >= 3 && effects == native_actions && verify_prepared_result(&result)",
            "source.snapshot != SemanticSnapshotGeneration::INITIAL",
            "sources.next().is_none()",
            "*source_bytes == text.len()",
            "text == value.as_str()",
            "verified && view.take_extraction().is_none()",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commerce_selection_cannot_bypass_original_qualification_or_retained_entry() {
        let desktop = include_str!("../../desktop/Cargo.toml");
        let composition = include_str!("../../crates/zephium-work-composition/Cargo.toml");
        let retained = include_str!(
            "../../crates/zephium-work-composition/src/retained_product_qualification.rs"
        );
        commerce_selection(desktop, composition, retained).unwrap();
        assert!(commerce_selection(
            &desktop.replace(
                "macos-work-retained-commerce-probe = [\"macos-work-retained-product-probe\",",
                "macos-work-retained-commerce-probe = [\"macos-work-navigation-probe\","
            ),
            composition,
            retained
        )
        .is_err());
        assert!(commerce_selection(
            desktop,
            &composition.replace(
                "retained-commerce-qualification = [\"retained-product-qualification\"]",
                "retained-commerce-qualification = [\"macos-work\"]"
            ),
            retained
        )
        .is_err());
        assert!(commerce_selection(
            desktop,
            composition,
            &retained.replace(
                "retained_commerce_objective.rs",
                "retained_svelte_objective.rs"
            )
        )
        .is_err());
        assert!(commerce_selection(
            desktop,
            composition,
            &retained.replace("PATH_PREFIX.into()", "\"/\".into()")
        )
        .is_err());
    }
    #[test]
    fn exact_discovery_wrappers_and_observers_cannot_drop_inspection() {
        let retained = include_str!(
            "../../crates/zephium-work-composition/src/retained_product_qualification.rs"
        );
        let discovery =
            include_str!("../../crates/zephium-work-composition/src/discovery_qualification.rs");
        let observer = include_str!(
            "../../crates/zephium-work-composition/src/navigation_qualification_observer.rs"
        );
        inspection_composition(retained, discovery, observer).unwrap();
        assert!(inspection_composition(
            &retained.replace("self.0.allows_progressive_observation()", "false"),
            discovery,
            observer
        )
        .is_err());
        assert!(inspection_composition(
            retained,
            &discovery.replace("self.0.allows_progressive_observation()", "false"),
            observer
        )
        .is_err());
        assert!(inspection_composition(
            &retained.replace(
                "AgentBrowserToolKind::Snapshot",
                "AgentBrowserToolKind::Wait"
            ),
            discovery,
            observer
        )
        .is_err());
        assert!(inspection_composition(
            retained,
            discovery,
            &observer.replace(
                "AgentBrowserToolKind::Snapshot",
                "AgentBrowserToolKind::Wait"
            )
        )
        .is_err());
        assert!(inspection_composition(
            &retained.replace("self.observe_kind(event.kind());", ""),
            discovery,
            observer
        )
        .is_err());
    }
    #[test]
    fn navigation_observer_cannot_bypass_admission_or_ordinary_shutdown() {
        let source = include_str!("../../desktop/src/navigation_probe.rs");
        ordinary_navigation_observer(source).unwrap();
        for boundary in [
            "let admit = super::admit_trusted_work;",
            "let admit = super::admit_retained_trusted_work;",
            "worker.join().is_ok()",
            "accepted && joined && normal_shutdown_clean",
            "api.prevent_exit()",
            ".admit(||",
            "control.admission.settle(result)",
            "state.control.admission.cancel()",
            "state.control.admission.terminal()",
        ] {
            assert!(ordinary_navigation_observer(&source.replace(boundary, "removed")).is_err());
        }
        for mutation in [
            "WebviewEngine::new()",
            "window.set_focus()",
            "shell.shutdown_with_deadline()",
        ] {
            assert!(ordinary_navigation_observer(&format!("{source}\n{mutation}")).is_err());
        }
    }
    #[test]
    fn retained_public_retention_is_excluded_and_shipping_selection_is_stateless() {
        let controller = include_str!("../../crates/zephium-agent-controller/src/work_retained.rs");
        require(
            controller,
            &[
                "task, AgentBrowserRetention::Stateless,",
                "#[cfg(feature = \"probe-harness\")] pub fn try_new_for_public_probe(",
                "task, AgentBrowserRetention::InspectablePublicData,",
            ],
        )
        .unwrap();
        let product = include_str!("../../crates/zephium-app/src/work_resources_product.rs");
        require(product, &["#[cfg(feature = \"work-execution-probe\")] #[path = \"work_resources_public_product.rs\"] mod public_qualification;"]).unwrap();
        let actor = include_str!("../../crates/zephium-app/src/work_resources_application.rs");
        require(actor, &["#[cfg(feature = \"work-execution-probe\")] #[path = \"work_resources_public_actor.rs\"] mod public_qualification;"]).unwrap();
        for (source, token) in [
            (controller, "AgentBrowserRetention::Stateless"),
            (product, "#[cfg(feature = \"work-execution-probe\")]"),
            (actor, "#[cfg(feature = \"work-execution-probe\")]"),
        ] {
            assert!(require(&source.replace(token, "removed"), &[token]).is_err());
        }
    }
    #[test]
    fn combined_qualification_requires_verified_effects_and_exact_post_action_source() {
        let source = include_str!("../../crates/zephium-terra-macos-probe/src/work_application.rs");
        combined_result_qualification(source).unwrap();
        for boundary in [
            "super::work_actor::combined_input(started)?",
            "effects >= 3 && effects == native_actions && verify_prepared_result(&result)",
            "source.snapshot != SemanticSnapshotGeneration::INITIAL",
            "sources.next().is_none()",
            "*source_bytes == text.len()",
            "text == value.as_str()",
            "verified && view.take_extraction().is_none()",
        ] {
            assert!(combined_result_qualification(&source.replace(boundary, "removed")).is_err());
        }
    }
    #[test]
    fn production_composition_has_no_task_policy_or_probe_authority() {
        for source in [
            include_str!("../../crates/zephium-work-composition/src/native.rs"),
            include_str!("../../desktop/src/work.rs"),
        ] {
            production(source).unwrap();
            for mutation in [
                "#[tauri::command]",
                "AgentRunPolicy::new()",
                "SemanticEffectClass::LocalWrite",
                "println!(secret)",
                "InspectablePublicData",
            ] {
                assert!(production(&format!("{source}\n{mutation}")).is_err());
            }
        }
    }
    #[test]
    fn actual_composition_identity_feature_and_qualification_gates_are_present() {
        check(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..")).unwrap();
    }
    #[test]
    fn attachment_before_validation_is_rejected_by_the_architecture_gate() {
        let source = include_str!("../../desktop/src/work.rs");
        preparation_precedes_attachment(source).unwrap();
        assert!(preparation_precedes_attachment(&format!(
            "composition.attach(&shell.callback_handle());\n{source}"
        ))
        .is_err());
        assert!(preparation_precedes_attachment("").is_err());
    }
}
