//! Privileged Work intent transport. Serialized data cannot create an attempt.
use tauri::{Manager, WebviewWindow};
use zephium_core::work::{WorkCommandId, WorkId};
use zephium_core::{ids::ProfileId, work::WorkError};
use zephium_ipc::work::{WorkCallV1, WorkResponseV1};
use zephium_ipc::work::{WorkOperationResponseV1, WorkOperationStateV1, WorkOperationV1};

#[cfg(feature = "work-product")]
pub(crate) struct WorkProductState {
    pub(crate) operations: super::work_operations::WorkOperations,
    providers: std::sync::Arc<super::work_provider::WorkProviders>,
}
#[cfg(feature = "work-product")]
impl WorkProductState {
    pub(crate) fn page_frame(
        &self,
        attempt: zephium_core::work::WorkAttemptId,
        step: zephium_core::work::WorkStepId,
    ) -> Option<std::sync::Arc<Vec<u8>>> {
        self.providers.activity.frame(attempt, step)
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn clear_website_data(&self, profile: ProfileId) -> std::sync::mpsc::Receiver<bool> {
        self.providers.clear_website_data(profile)
    }
}
/// Admits a Work run that Rust itself starts (the Ask panel's "Run in
/// Work"), through the same operation owner as the panel's own runs.
#[cfg(all(feature = "work-product", not(target_os = "linux")))]
pub(crate) fn admit_run(
    app: &tauri::AppHandle,
    profile: ProfileId,
    operation: WorkCommandId,
    input: WorkOperationV1,
) -> Result<WorkOperationStateV1, WorkError> {
    let owner = app
        .try_state::<WorkProductState>()
        .ok_or(WorkError::Unavailable)?;
    let provider = owner.providers.clone();
    let shell = app.state::<zephium_app::Handle>().inner().clone();
    let task_input = input.clone();
    owner
        .operations
        .admit((profile, operation), input, async move {
            provider.run(shell, profile, task_input).await
        })
}

/// Durable last frames of pages the agent read, managed beside the media blobs.
pub(crate) struct WorkFrames(pub(crate) std::sync::Arc<zephium_store::WorkFrameStore>);

#[cfg(feature = "work-product")]
pub(crate) fn install(
    app: &tauri::AppHandle,
    engine: std::sync::Arc<zephium_engine::WebviewEngine>,
    store: std::sync::Arc<zephium_store::SqliteStore>,
    frames: Option<std::sync::Arc<zephium_store::WorkFrameStore>>,
) -> bool {
    use zephium_core::ports::store::Store;
    #[cfg(feature = "work-development-traces")]
    super::work_diagnostics::install(app);
    #[cfg(feature = "work-integration-qa")]
    super::work_captures::install(app);
    zephium_app::work_screenshots::initialize(store.app_setting_readback("work.screenshots"));
    let ai_enabled = store.app_setting("ai.enabled").as_deref() != Some("false");
    let work_enabled = store.app_setting("work.enabled").as_deref() != Some("false");
    let providers = std::sync::Arc::new(super::work_provider::WorkProviders::new(
        engine, store, frames,
    ));
    {
        let app = app.clone();
        providers
            .activity
            .set_page_observer(std::sync::Arc::new(move |change| {
                super::emit_to_privileged(&app, super::MAIN_LABEL, "zephium:work-changed", &change);
            }));
    }
    {
        let app = app.clone();
        super::work_decision::set_observer(Box::new(move |change| {
            super::emit_to_privileged(
                &app,
                super::MAIN_LABEL,
                "zephium:work-decision-preference-changed",
                &change,
            );
        }));
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let app = app.clone();
        let alerts = alerts::WorkAlerts::default();
        providers
            .browser
            .set_human_page_observer(std::sync::Arc::new(move |change| {
                super::emit_to_privileged(
                    &app,
                    super::MAIN_LABEL,
                    "zephium:work-human-changed",
                    &change,
                );
                alerts.refresh(&app);
            }));
    }
    app.manage(WorkProductState {
        operations: super::work_operations::WorkOperations::with_enablement(
            ai_enabled,
            work_enabled,
        ),
        providers,
    })
}

pub(crate) fn preference_processed(
    app: &tauri::AppHandle,
    disposition: &zephium_ipc::OperationDisposition,
) {
    #[cfg(feature = "work-product")]
    if let Some(owner) = app.try_state::<WorkProductState>() {
        owner.operations.preference_processed(disposition);
    }
    #[cfg(not(feature = "work-product"))]
    let _ = (app, disposition);
}

async fn selected_work(
    app: &tauri::AppHandle,
    profile: &str,
    work: WorkId,
) -> Result<ProfileId, WorkError> {
    if super::shutdown_started(app) {
        return Err(WorkError::Shutdown);
    }
    let profile = ProfileId::parse(profile)
        .filter(|id| id.to_string() == profile)
        .ok_or(WorkError::Invalid)?;
    let shell = app.state::<zephium_app::Handle>();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        admit("projection", None, || shell.work_projection(profile, work)).await?,
    )
    .await
    .map_err(|_| WorkError::Unavailable)??;
    if response.profile != profile {
        return Err(WorkError::ProfileUnavailable);
    }
    Ok(profile)
}

/// Saves a plan's calendar where the person chooses. Only a calendar: the
/// text must be one, and the name stays a plain file name.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_save_calendar(caller: WebviewWindow, name: String, text: String) -> bool {
    use tauri_plugin_dialog::DialogExt;
    if !super::authorize(&caller, super::CallerPolicy::Main, "work_save_calendar")
        || !calendar_file(&name, &text)
    {
        return false;
    }
    let app = caller.app_handle().clone();
    let picked = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(&name)
            .add_filter("Calendar", &["ics"])
            .blocking_save_file()
    })
    .await
    .ok()
    .flatten();
    let Some(path) = picked.and_then(|picked| picked.into_path().ok()) else {
        return false;
    };
    matches!(
        tokio::task::spawn_blocking(move || std::fs::write(path, text)).await,
        Ok(Ok(()))
    )
}

fn calendar_file(name: &str, text: &str) -> bool {
    name.len() <= 120
        && name.ends_with(".ics")
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
        && text.len() <= 1024 * 1024
        && text.starts_with("BEGIN:VCALENDAR\r\n")
        && text.trim_end().ends_with("END:VCALENDAR")
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn work_activity(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    work: WorkId,
) -> zephium_ipc::work::WorkActivityResponseV1 {
    let result = async {
        if !super::authorize(&caller, super::CallerPolicy::Main, "work_activity")
            || super::shutdown_started(&app)
        {
            return Err(WorkError::Unavailable);
        }
        let profile = ProfileId::parse(&expected_profile)
            .filter(|id| id.to_string() == expected_profile)
            .ok_or(WorkError::Invalid)?;
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            app.state::<zephium_app::Handle>()
                .work_projection(profile, work)?,
        )
        .await
        .map_err(|_| WorkError::Unavailable)??;
        if response.profile != profile {
            return Err(WorkError::ProfileUnavailable);
        }
        let zephium_core::work::port::WorkReply::Runtime(state) = response.reply else {
            return Err(WorkError::Invalid);
        };
        #[cfg(feature = "work-product")]
        {
            let activity = &app.state::<WorkProductState>().providers.activity;
            Ok((activity.read(&state), activity.read_pages(&state)))
        }
        #[cfg(not(feature = "work-product"))]
        {
            let _ = state;
            Err(WorkError::Unavailable)
        }
    }
    .await;
    let (signals, pages, error) = match result {
        Ok((signals, pages)) => (signals, pages, None),
        Err(error) => (Vec::new(), Vec::new(), Some(error.into())),
    };
    zephium_ipc::work::WorkActivityResponseV1 {
        version: 1,
        profile: expected_profile,
        work,
        signals,
        pages,
        error,
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn work_operation(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    operation: WorkCommandId,
    input: WorkOperationV1,
) -> WorkOperationResponseV1 {
    let result = async {
        if !super::authorize(&caller, super::CallerPolicy::Main, "work_operation") {
            return Err(WorkError::Unavailable);
        }
        if serde_json::to_vec(&input).map_or(true, |bytes| {
            bytes.len() > zephium_core::work::MAX_WORK_REQUEST_BYTES
        }) {
            return Err(WorkError::Capacity);
        }
        let profile = selected_work(&app, &expected_profile, input.work()).await?;
        #[cfg(feature = "work-product")]
        {
            let owner = app.state::<WorkProductState>();
            let provider = owner.providers.clone();
            let shell = app.state::<zephium_app::Handle>().inner().clone();
            let task_input = input.clone();
            owner
                .operations
                .admit((profile, operation), input, async move {
                    provider.run(shell, profile, task_input).await
                })
        }
        #[cfg(not(feature = "work-product"))]
        {
            let _ = profile;
            Err(WorkError::Unavailable)
        }
    }
    .await;
    WorkOperationResponseV1 {
        version: 1,
        profile: expected_profile,
        operation,
        state: result.unwrap_or_else(|error| WorkOperationStateV1::Refused {
            error: error.into(),
        }),
    }
}

/// Persist only the user's draft body. The original agent artifact and a live
/// execution remain unchanged; the returned user revision can be preserved.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn work_edit_draft(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    work: WorkId,
    expected_revision: zephium_core::work::WorkRevision,
    command: WorkCommandId,
    execution: zephium_core::work::WorkExecutionId,
    artifact: zephium_core::work::WorkArtifactId,
    body: String,
) -> WorkResponseV1 {
    use zephium_core::work::{
        artifact::WorkArtifactDataV1, port::WorkReply, runtime::WorkRuntimeIntent,
    };
    let result = async {
        if !super::authorize(&caller, super::CallerPolicy::Main, "work_edit_draft")
            || super::shutdown_started(&app)
            || body.len() > 16_000
        {
            return Err(WorkError::Invalid);
        }
        let profile = ProfileId::parse(&expected_profile)
            .filter(|id| id.to_string() == expected_profile)
            .ok_or(WorkError::Invalid)?;
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            app.state::<zephium_app::Handle>()
                .work_projection(profile, work)?,
        )
        .await
        .map_err(|_| WorkError::Unavailable)??;
        if response.profile != profile {
            return Err(WorkError::ProfileUnavailable);
        }
        let WorkReply::Runtime(projection) = response.reply else {
            return Err(WorkError::Invalid);
        };
        if projection.work.revision != expected_revision {
            return Err(WorkError::Conflict);
        }
        let execution = projection
            .executions
            .iter()
            .find(|e| e.id == execution)
            .ok_or(WorkError::NotFound)?;
        let original = execution
            .artifacts
            .iter()
            .find(|a| a.id == artifact)
            .ok_or(WorkError::NotFound)?;
        let edited = execution
            .user_artifacts
            .iter()
            .find(|u| u.artifact == artifact && u.edited_data.is_some());
        let mut data = edited
            .and_then(|u| u.edited_data.clone())
            .unwrap_or_else(|| original.data.clone());
        let WorkArtifactDataV1::Draft { body: current, .. } = &mut data else {
            return Err(WorkError::Invalid);
        };
        *current = body;
        let evidence = edited.map_or_else(|| original.evidence.clone(), |u| u.evidence.clone());
        data.validate(evidence.len())?;
        Ok(WorkCallV1::Execute {
            command: zephium_ipc::work::WorkCommandV1 {
                version: 1,
                work,
                expected_revision,
                command,
                intent: WorkRuntimeIntent::EditArtifact {
                    execution: execution.id,
                    artifact,
                    data,
                    evidence,
                },
            },
        })
    }
    .await;
    match result {
        Ok(call) => work_call(caller, app, expected_profile, call).await,
        Err(error) => WorkResponseV1 {
            version: 1,
            profile: expected_profile,
            reply: zephium_ipc::work::WorkReplyV1::Error {
                error: error.into(),
            },
        },
    }
}

/// Manifest preview for the composer. Bodies never cross this boundary.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_context_preview(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    purpose: zephium_core::work::context::WorkContextPurpose,
    selection: zephium_core::work::context::WorkContextSelectionV1,
) -> zephium_ipc::work::WorkContextPreviewV1 {
    use zephium_ipc::work::WorkContextPreviewV1;
    let result = async {
        if !super::authorize(&caller, super::CallerPolicy::Main, "work_context_preview")
            || super::shutdown_started(&app)
        {
            return Err(WorkError::Unavailable);
        }
        let profile = ProfileId::parse(&expected_profile)
            .filter(|id| id.to_string() == expected_profile)
            .ok_or(WorkError::Invalid)?;
        selection.validate()?;
        #[cfg(feature = "work-product")]
        {
            let shell = app.state::<zephium_app::Handle>().inner().clone();
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                zephium_app::work_context::WorkContextAdmission::new(shell)
                    .preview(profile, purpose, &selection),
            )
            .await
            .map_err(|_| WorkError::Unavailable)?
        }
        #[cfg(not(feature = "work-product"))]
        {
            let _ = (profile, purpose);
            Err(WorkError::Unavailable)
        }
    }
    .await;
    match result {
        Ok(disclosure) => WorkContextPreviewV1::Admitted { disclosure },
        Err(error) => WorkContextPreviewV1::Refused {
            error: error.into(),
        },
    }
}

/// Whether turning Work off would stop work that is running now.
#[tauri::command]
#[specta::specta]
pub(crate) fn work_running(caller: WebviewWindow, app: tauri::AppHandle) -> bool {
    if !super::authorize(&caller, super::CallerPolicy::Main, "work_running") {
        return false;
    }
    #[cfg(feature = "work-product")]
    {
        app.try_state::<WorkProductState>()
            .is_some_and(|owner| owner.operations.running())
    }
    #[cfg(not(feature = "work-product"))]
    {
        let _ = app;
        false
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn work_operation_status(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    work: WorkId,
    operation: WorkCommandId,
    acknowledge: bool,
) -> WorkOperationResponseV1 {
    let result = async {
        if !super::authorize(&caller, super::CallerPolicy::Main, "work_operation_status") {
            return Err(WorkError::Unavailable);
        }
        let profile = selected_work(&app, &expected_profile, work).await?;
        #[cfg(feature = "work-product")]
        {
            app.state::<WorkProductState>().operations.observe(
                (profile, operation),
                work,
                acknowledge,
            )
        }
        #[cfg(not(feature = "work-product"))]
        {
            let _ = (profile, acknowledge);
            Err(WorkError::Unavailable)
        }
    }
    .await;
    WorkOperationResponseV1 {
        version: 1,
        profile: expected_profile,
        operation,
        state: result.unwrap_or_else(|error| WorkOperationStateV1::Refused {
            error: error.into(),
        }),
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn work_call(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    call: WorkCallV1,
) -> WorkResponseV1 {
    // Invalid scope is echoed only as correlation, never used as an owner.
    let failed = |error: WorkError| WorkResponseV1 {
        version: 1,
        profile: expected_profile.clone(),
        reply: zephium_ipc::work::WorkReplyV1::Error {
            error: error.into(),
        },
    };
    if !super::authorize(&caller, super::CallerPolicy::Main, "work_call")
        || super::shutdown_started(&app)
    {
        return failed(WorkError::Unavailable);
    }
    let Some(profile) =
        ProfileId::parse(&expected_profile).filter(|id| id.to_string() == expected_profile)
    else {
        return failed(WorkError::Invalid);
    };
    if serde_json::to_vec(&call).map_or(true, |bytes| {
        bytes.len() > zephium_core::work::MAX_WORK_REQUEST_BYTES
    }) {
        return failed(WorkError::Capacity);
    }
    if !super::resource_close::touch(&app, caller.label()) {
        return failed(WorkError::Unavailable);
    }
    let shell = app.state::<zephium_app::Handle>();
    let name = call_name(&call);
    let request = match admit(name, call_fault(&call), || {
        shell.work_call(profile, call.clone())
    })
    .await
    {
        Ok(request) => request,
        Err(error) => return failed(error),
    };
    // An admitted Store operation may commit after observation expires. Its
    // stable command identity remains replayable through the original Store.
    let response =
        match tokio::time::timeout(std::time::Duration::from_secs(8), request.response(profile))
            .await
        {
            Ok(response) => response,
            Err(_) => failed(WorkError::OutcomeUnknown),
        };
    if let zephium_ipc::work::WorkReplyV1::Error { error } = &response.reply {
        if name.starts_with("environment:c") {
            trace(format_args!("work: phase=call call={name} error={error:?}"));
        }
    }
    response
}

/// The application admits four Work documents at a time. A refusal at that
/// bound is a queue, not an answer: wait briefly for a permit before refusing.
async fn admit<T>(
    name: &str,
    fault: Option<&'static str>,
    mut submit: impl FnMut() -> Result<T, WorkError>,
) -> Result<T, WorkError> {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(4);
    let mut waited = std::time::Duration::ZERO;
    let mut delay = std::time::Duration::from_millis(25);
    loop {
        match submit() {
            Err(WorkError::Capacity) if waited < WAIT => {
                tokio::time::sleep(delay).await;
                waited += delay;
                delay = (delay * 2).min(std::time::Duration::from_millis(400));
            }
            result => {
                if let Err(error) = &result {
                    trace(format_args!("{}", refusal(name, *error, fault, waited)));
                } else if !waited.is_zero() {
                    trace(format_args!(
                        "work: phase=call call={name} waited_ms={}",
                        waited.as_millis()
                    ));
                }
                return result;
            }
        }
    }
}

fn refusal(
    name: &str,
    error: WorkError,
    fault: Option<&'static str>,
    waited: std::time::Duration,
) -> String {
    let fault = match (error, fault) {
        (WorkError::Invalid, Some(fault)) => format!(" fault={fault}"),
        _ => String::new(),
    };
    format!(
        "work: phase=call call={name} refused={error:?}{fault} waited_ms={}",
        waited.as_millis()
    )
}

/// The bound a refused checkpoint view broke, named for the log.
fn call_fault(call: &WorkCallV1) -> Option<&'static str> {
    use zephium_core::work::environment::WorkEnvironmentCall;
    match call {
        WorkCallV1::Environment {
            request: WorkEnvironmentCall::Checkpoint { view, .. },
            ..
        } => view.fault().map(|fault| fault.name()),
        _ => None,
    }
}

fn trace(arguments: std::fmt::Arguments<'_>) {
    #[cfg(feature = "work-development-traces")]
    super::work_diagnostics::record(arguments);
    #[cfg(not(feature = "work-development-traces"))]
    super::write_diagnostic(arguments);
}

fn call_name(call: &WorkCallV1) -> &'static str {
    use zephium_core::work::environment::WorkEnvironmentCall;
    match call {
        WorkCallV1::Environment { request, .. } => match request {
            WorkEnvironmentCall::Checkpoint { .. } => "environment:checkpoint",
            WorkEnvironmentCall::Command { .. } => "environment:command",
            WorkEnvironmentCall::Open { .. } => "environment:open",
            WorkEnvironmentCall::Read { .. } => "environment:read",
            WorkEnvironmentCall::List { .. } => "environment:list",
        },
        WorkCallV1::Query { .. } => "query",
        WorkCallV1::Author { .. } => "author",
        WorkCallV1::Execute { .. } => "execute",
    }
}

#[cfg(all(
    feature = "work-product",
    any(target_os = "macos", target_os = "windows")
))]
#[path = "work_alerts.rs"]
mod alerts;
#[path = "work_human.rs"]
pub(crate) mod human;

pub(crate) fn release_human_presentations(app: &tauri::AppHandle) {
    #[cfg(all(
        feature = "work-product",
        any(target_os = "macos", target_os = "windows")
    ))]
    if let Some(owner) = app.try_state::<WorkProductState>() {
        owner.providers.browser.release_presented_human_pages();
    }
    #[cfg(not(all(
        feature = "work-product",
        any(target_os = "macos", target_os = "windows")
    )))]
    let _ = app;
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn a_busy_admission_waits_for_a_permit_instead_of_refusing() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let mut refusals = 2;
        let admitted = runtime.block_on(admit("query", None, || {
            if refusals > 0 {
                refusals -= 1;
                Err(WorkError::Capacity)
            } else {
                Ok(7)
            }
        }));
        assert_eq!(admitted, Ok(7));
        let other = runtime.block_on(admit::<u8>("query", None, || Err(WorkError::Invalid)));
        assert_eq!(other, Err(WorkError::Invalid));
    }

    #[test]
    fn a_refused_checkpoint_names_the_bound_its_view_broke() {
        use zephium_core::work::environment::{
            WorkElementPlacement, WorkEnvironmentCall, WorkEnvironmentView,
        };
        let mut view = WorkEnvironmentView::default();
        view.placements.push(WorkElementPlacement {
            element: 1.into(),
            x: 0,
            y: 0,
            width: 280,
            height: 60,
            revision: 2,
        });
        let call = WorkCallV1::Environment {
            version: 1,
            request: WorkEnvironmentCall::Checkpoint {
                id: 1.into(),
                expected: view.revision,
                view,
            },
        };
        let fault = call_fault(&call);
        assert_eq!(fault, Some("placement_size"));
        let zero = std::time::Duration::ZERO;
        assert_eq!(
            refusal("environment:checkpoint", WorkError::Invalid, fault, zero),
            "work: phase=call call=environment:checkpoint refused=Invalid fault=placement_size waited_ms=0"
        );
        assert_eq!(
            refusal("environment:checkpoint", WorkError::Conflict, fault, zero),
            "work: phase=call call=environment:checkpoint refused=Conflict waited_ms=0"
        );
    }
}

#[cfg(test)]
mod calendar_tests {
    #[test]
    fn only_a_calendar_under_a_plain_name_is_saved() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
        assert!(super::calendar_file("CS exam plan.ics", ics));
        assert!(!super::calendar_file("../plan.ics", ics));
        assert!(!super::calendar_file(".plan.ics", ics));
        assert!(!super::calendar_file("plan.sh", ics));
        assert!(!super::calendar_file("plan.ics", "#!/bin/sh\n"));
    }
}
