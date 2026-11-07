//! Settings → Connections: the command-line tools Work can use and the MCP
//! servers the person added. Secrets go straight to the Keychain and never
//! come back; responses say only which ones are held.
use super::*;
use zephium_core::ids::ProfileId;
use zephium_core::work::WorkError;
use zephium_ipc::work::{
    WorkCliRowV1, WorkConnectionsResponseV1, WorkServerCheckV1, WorkServerDraftV1,
    WorkServerOutcomeV1,
};

fn profile_of(expected: &str) -> Result<ProfileId, WorkError> {
    ProfileId::parse(expected)
        .filter(|id| id.to_string() == expected)
        .ok_or(WorkError::Invalid)
}

fn admitted(
    caller: &WebviewWindow,
    app: &tauri::AppHandle,
    command: &str,
) -> Result<(), WorkError> {
    if !authorize(caller, CallerPolicy::Main, command) {
        return Err(WorkError::Unavailable);
    }
    if shutdown_started(app) {
        return Err(WorkError::Shutdown);
    }
    Ok(())
}

#[cfg(feature = "work-product")]
mod live {
    use super::*;
    use zephium_app::work_connections::client::keychain;
    use zephium_app::work_connections::{cli, store};
    use zephium_ipc::work::{
        WorkCliStatusV1, WorkServerAuthV1, WorkServerRowV1, WorkServerTransportV1, WorkServerV1,
    };

    pub(super) fn store(
        app: &tauri::AppHandle,
    ) -> Result<&'static store::ConnectionStore, WorkError> {
        if let Some(store) = store::shared() {
            return Ok(store);
        }
        let data = app
            .path()
            .app_data_dir()
            .map_err(|_| WorkError::Unavailable)?;
        store::install(&data);
        store::shared().ok_or(WorkError::Unavailable)
    }

    pub(super) async fn clis() -> Vec<WorkCliRowV1> {
        cli::forget();
        cli::statuses()
            .await
            .into_iter()
            .map(|status| {
                let (status_v1, account) = match (&status.path, &status.auth) {
                    (None, _) => (WorkCliStatusV1::Missing, None),
                    (Some(_), cli::CliAuth::SignedIn(account)) => {
                        (WorkCliStatusV1::SignedIn, account.clone())
                    }
                    (Some(_), cli::CliAuth::SignedOut) => (WorkCliStatusV1::SignedOut, None),
                    (Some(_), cli::CliAuth::NotNeeded(account)) => {
                        (WorkCliStatusV1::Ready, account.clone())
                    }
                    (Some(_), cli::CliAuth::Unknown) => (WorkCliStatusV1::Unknown, None),
                };
                WorkCliRowV1 {
                    id: status.cli.id().to_owned(),
                    status: status_v1,
                    version: status.version,
                    path: status.path.map(|path| path.to_string_lossy().into_owned()),
                    account,
                }
            })
            .collect()
    }

    /// Which secrets each server holds, from Keychain attributes only.
    pub(super) fn rows(profile: &str, servers: Vec<WorkServerV1>) -> Vec<WorkServerRowV1> {
        servers
            .into_iter()
            .map(|server| {
                let mut accounts: Vec<String> = Vec::new();
                match &server.transport {
                    WorkServerTransportV1::Stdio { env, .. } => accounts.extend(
                        env.iter()
                            .filter(|e| e.secret)
                            .map(|e| format!("env.{}", e.name)),
                    ),
                    WorkServerTransportV1::Http {
                        auth: WorkServerAuthV1::Bearer,
                        ..
                    } => accounts.push("bearer".into()),
                    _ => {}
                }
                let secrets = accounts
                    .into_iter()
                    .filter(|account| keychain::present(profile, &server.id, account))
                    .collect();
                let signed_in = matches!(
                    server.transport,
                    WorkServerTransportV1::Http {
                        auth: WorkServerAuthV1::OAuth,
                        ..
                    }
                ) && keychain::present(profile, &server.id, "oauth");
                WorkServerRowV1 {
                    server,
                    secrets,
                    signed_in,
                }
            })
            .collect()
    }

    /// The Keychain accounts a server may hold, for moving or forgetting them.
    pub(super) fn accounts(server: &WorkServerV1) -> Vec<String> {
        let mut accounts = Vec::new();
        match &server.transport {
            WorkServerTransportV1::Stdio { env, .. } => accounts.extend(
                env.iter()
                    .filter(|e| e.secret)
                    .map(|e| format!("env.{}", e.name)),
            ),
            WorkServerTransportV1::Http {
                auth: WorkServerAuthV1::Bearer,
                ..
            } => accounts.push("bearer".into()),
            WorkServerTransportV1::Http {
                auth: WorkServerAuthV1::OAuth,
                ..
            } => accounts.push("oauth".into()),
            _ => {}
        }
        accounts
    }
}

async fn listing(
    app: &tauri::AppHandle,
    profile: ProfileId,
    with_clis: bool,
) -> Result<(Vec<WorkCliRowV1>, Vec<zephium_ipc::work::WorkServerRowV1>), WorkError> {
    #[cfg(feature = "work-product")]
    {
        let store = live::store(app)?;
        let id = profile.to_string();
        let servers = store.servers(&id).map_err(|_| WorkError::Unavailable)?;
        let clis = if with_clis {
            live::clis().await
        } else {
            Vec::new()
        };
        let rows = tokio::task::spawn_blocking(move || live::rows(&id, servers))
            .await
            .map_err(|_| WorkError::Unavailable)?;
        Ok((clis, rows))
    }
    #[cfg(not(feature = "work-product"))]
    {
        let _ = (app, profile, with_clis);
        Err(WorkError::Unavailable)
    }
}

fn response(
    expected: String,
    result: Result<(Vec<WorkCliRowV1>, Vec<zephium_ipc::work::WorkServerRowV1>), WorkError>,
) -> WorkConnectionsResponseV1 {
    match result {
        Ok((clis, servers)) => WorkConnectionsResponseV1 {
            version: 1,
            profile: expected,
            clis,
            servers,
            error: None,
        },
        Err(error) => WorkConnectionsResponseV1 {
            version: 1,
            profile: expected,
            clis: Vec::new(),
            servers: Vec::new(),
            error: Some(error.into()),
        },
    }
}

/// The tools found on this Mac and the servers this profile added.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_connections(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
) -> WorkConnectionsResponseV1 {
    let result = async {
        admitted(&caller, &app, "work_connections")?;
        let profile = profile_of(&expected_profile)?;
        listing(&app, profile, true).await
    }
    .await;
    response(expected_profile, result)
}

#[cfg(feature = "work-product")]
fn connection_mutations() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default)
}

#[cfg(feature = "work-product")]
#[derive(Clone)]
struct CheckedDraft {
    draft: WorkServerDraftV1,
    secrets: std::collections::HashMap<String, String>,
    tools: Vec<zephium_app::work_connections::client::McpTool>,
    checked: std::time::Instant,
}
#[cfg(feature = "work-product")]
fn drafts() -> &'static std::sync::Mutex<std::collections::HashMap<(String, String), CheckedDraft>>
{
    static DRAFTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(String, String), CheckedDraft>>,
    > = std::sync::OnceLock::new();
    DRAFTS.get_or_init(Default::default)
}

/// Connect an unsaved draft, sign in if necessary, and preview its tools.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_preview_connection(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    draft: WorkServerDraftV1,
) -> WorkServerCheckV1 {
    let id = draft.server.id.clone();
    let result = async {
        admitted(&caller, &app, "work_preview_connection")?;
        let profile = profile_of(&expected_profile)?;
        #[cfg(feature = "work-product")]
        {
            let _guard = connection_mutations().lock().await;
            use zephium_app::work_connections::{
                client::{
                    keychain,
                    oauth::{MemoryTokens, TokenStore},
                    McpError,
                },
                mcp,
            };
            if !valid_draft(&draft) {
                return Err(WorkError::Invalid);
            }
            let owner = profile.to_string();
            let store = live::store(&app)?;
            let (who, candidate) = (owner.clone(), draft.clone());
            let secrets = tokio::task::spawn_blocking(move || {
                let existing = store.servers(&who).map_err(|_| WorkError::Unavailable)?;
                let old = existing.iter().find(|s| {
                    s.id == candidate
                        .previous
                        .as_deref()
                        .unwrap_or(&candidate.server.id)
                });
                let mut secrets = std::collections::HashMap::new();
                // Never forward held credentials to an edited URL or command.
                if let Some(old) = old.filter(|s| s.transport == candidate.server.transport) {
                    for account in live::accounts(old) {
                        if keychain::present(&who, &old.id, &account) {
                            secrets.insert(
                                account.clone(),
                                keychain::read(&who, &old.id, &account)
                                    .map_err(|_| WorkError::Unavailable)?,
                            );
                        }
                    }
                }
                for secret in &candidate.secrets {
                    secrets.insert(secret.account.clone(), secret.value.clone());
                }
                Ok::<_, WorkError>(secrets)
            })
            .await
            .map_err(|_| WorkError::Unavailable)??;
            let tokens = std::sync::Arc::new(MemoryTokens(std::sync::Mutex::new(
                secrets.get("oauth").cloned(),
            )));
            let endpoint = || {
                mcp::endpoint_with(
                    &draft.server,
                    |account| secrets.get(account).cloned().ok_or(McpError::Unauthorized),
                    tokens.clone(),
                )
            };
            let (mut check, mut tools) =
                mcp::check_endpoint(&owner, &draft.server, endpoint()).await;
            if check.outcome == WorkServerOutcomeV1::SignIn {
                if let zephium_ipc::work::WorkServerTransportV1::Http {
                    url,
                    auth: zephium_ipc::work::WorkServerAuthV1::OAuth,
                } = &draft.server.transport
                {
                    match sign_in_flow(&app, &owner, &id, url, tokens.clone()).await {
                        Ok(()) => {
                            (check, tools) =
                                mcp::check_endpoint(&owner, &draft.server, endpoint()).await
                        }
                        Err(McpError::Cancelled | McpError::Timeout) => {
                            check.outcome = WorkServerOutcomeV1::Cancelled
                        }
                        Err(_) => check.outcome = WorkServerOutcomeV1::SignIn,
                    }
                }
            }
            if let Some(tools) = tools {
                let mut secrets = secrets;
                if let Some(token) = tokens.load() {
                    secrets.insert("oauth".into(), token);
                }
                let mut drafts = drafts().lock().unwrap_or_else(|p| p.into_inner());
                drafts.retain(|_, draft| {
                    draft.checked.elapsed() < std::time::Duration::from_secs(300)
                });
                if drafts.len() >= 32 {
                    return Err(WorkError::Unavailable);
                }
                drafts.insert(
                    (owner.clone(), id.clone()),
                    CheckedDraft {
                        draft,
                        secrets,
                        tools,
                        checked: std::time::Instant::now(),
                    },
                );
            }
            if check.outcome == WorkServerOutcomeV1::Ready {
                let expiry_key = (owner, id.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                    let mut drafts = drafts().lock().unwrap_or_else(|p| p.into_inner());
                    if drafts.get(&expiry_key).is_some_and(|held| {
                        held.checked.elapsed() >= std::time::Duration::from_secs(300)
                    }) {
                        drafts.remove(&expiry_key);
                    }
                });
            }
            Ok(check)
        }
        #[cfg(not(feature = "work-product"))]
        {
            let _ = (profile, draft);
            Err(WorkError::Unavailable)
        }
    }
    .await;
    result.unwrap_or_else(|error| check_failed(expected_profile, id, error))
}

#[cfg(feature = "work-product")]
fn valid_draft(draft: &WorkServerDraftV1) -> bool {
    draft.server.validate()
        && draft.secrets.len() <= 40
        && draft.secrets.iter().all(|s| {
            !s.value.is_empty()
                && s.value.len() <= 8192
                && live::accounts(&draft.server).contains(&s.account)
        })
        && draft.secrets.iter().enumerate().all(|(i, s)| {
            !draft.secrets[..i]
                .iter()
                .any(|old| old.account == s.account)
        })
}

/// Adds or replaces a server; new secrets go to the Keychain.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_save_connection(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    draft: WorkServerDraftV1,
) -> WorkConnectionsResponseV1 {
    let result = async {
        admitted(&caller, &app, "work_save_connection")?;
        let profile = profile_of(&expected_profile)?;
        #[cfg(feature = "work-product")]
        {
            let _guard = connection_mutations().lock().await;
            use zephium_app::work_connections::client::keychain;
            let store = live::store(&app)?;
            let id = profile.to_string();
            if !valid_draft(&draft) {
                return Err(WorkError::Invalid);
            }
            let checked = {
                let drafts = drafts().lock().unwrap_or_else(|p| p.into_inner());
                let key = (id.clone(), draft.server.id.clone());
                if !drafts.get(&key).is_some_and(|held| {
                    held.draft == draft
                        && held.checked.elapsed() < std::time::Duration::from_secs(300)
                }) {
                    None
                } else {
                    drafts.get(&key).cloned()
                }
            };
            let receipt_key = (id.clone(), draft.server.id.clone());
            tokio::task::spawn_blocking(move || {
                let existing = store.servers(&id).map_err(|_| WorkError::Unavailable)?;
                let old = existing
                    .iter()
                    .find(|s| s.id == draft.previous.as_deref().unwrap_or(&draft.server.id));
                let enable_only = old.is_some_and(|old| {
                    old.id == draft.server.id
                        && old.name == draft.server.name
                        && old.transport == draft.server.transport
                        && draft.secrets.is_empty()
                });
                if checked.is_none() && !enable_only {
                    return Err(WorkError::Invalid);
                }
                let empty = std::collections::HashMap::new();
                let secrets = checked
                    .as_ref()
                    .map(|checked| &checked.secrets)
                    .unwrap_or(&empty);
                store
                    .commit_verified(
                        &id,
                        draft.server.clone(),
                        draft.previous.as_deref(),
                        secrets,
                        &keychain::SystemVault,
                    )
                    .map_err(|_| WorkError::Unavailable)?;
                if let Some(checked) = checked {
                    store
                        .put_tools(&id, &draft.server.id, Some(&checked.tools))
                        .map_err(|_| WorkError::Unavailable)?;
                }
                if let Some(old) = old.filter(|old| old.id != draft.server.id) {
                    keychain::delete_server(&id, &old.id).map_err(|_| WorkError::Unavailable)?;
                    let _ = store.put_tools(&id, &old.id, None);
                } else if let Some(old) = old {
                    for account in live::accounts(old) {
                        if old.id != draft.server.id
                            || !live::accounts(&draft.server).contains(&account)
                        {
                            keychain::delete(&id, &old.id, &account)
                                .map_err(|_| WorkError::Unavailable)?;
                        }
                    }
                }
                Ok::<_, WorkError>(())
            })
            .await
            .map_err(|_| WorkError::Unavailable)??;
            drafts()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&receipt_key);
        }
        #[cfg(not(feature = "work-product"))]
        let _ = draft;
        listing(&app, profile, false).await
    }
    .await;
    response(expected_profile, result)
}

/// Removes a server and forgets its secrets.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_remove_connection(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    id: String,
) -> WorkConnectionsResponseV1 {
    let result = async {
        admitted(&caller, &app, "work_remove_connection")?;
        let profile = profile_of(&expected_profile)?;
        #[cfg(feature = "work-product")]
        {
            if let Some((_, cancel)) = sign_ins()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&(expected_profile.clone(), id.clone()))
            {
                let _ = cancel.send(true);
            }
            let _guard = connection_mutations().lock().await;
            drafts()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&(expected_profile.clone(), id.clone()));
            use zephium_app::work_connections::client::keychain;
            let store = live::store(&app)?;
            let owner = profile.to_string();
            tokio::task::spawn_blocking(move || {
                let servers = store.servers(&owner).map_err(|_| WorkError::Unavailable)?;
                if !servers.iter().any(|s| s.id == id) {
                    return Err(WorkError::NotFound);
                }
                store
                    .remove_with_vault(&owner, &id, &keychain::SystemVault)
                    .map_err(|_| WorkError::Unavailable)?;
                Ok::<_, WorkError>(())
            })
            .await
            .map_err(|_| WorkError::Unavailable)??;
        }
        #[cfg(not(feature = "work-product"))]
        let _ = id;
        listing(&app, profile, false).await
    }
    .await;
    response(expected_profile, result)
}

fn check_failed(expected: String, id: String, error: WorkError) -> WorkServerCheckV1 {
    WorkServerCheckV1 {
        version: 1,
        profile: expected,
        id,
        outcome: WorkServerOutcomeV1::Failed,
        server_name: None,
        tools: Vec::new(),
        error: Some(error.into()),
    }
}

/// Connects to a server once and lists its tools.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_check_connection(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    id: String,
) -> WorkServerCheckV1 {
    let key = id.clone();
    let result = async {
        admitted(&caller, &app, "work_check_connection")?;
        let profile = profile_of(&expected_profile)?;
        #[cfg(feature = "work-product")]
        {
            let _guard = connection_mutations().lock().await;
            let store = live::store(&app)?;
            let owner = profile.to_string();
            let server = store
                .servers(&owner)
                .map_err(|_| WorkError::Unavailable)?
                .into_iter()
                .find(|s| s.id == id)
                .ok_or(WorkError::NotFound)?;
            Ok(zephium_app::work_connections::mcp::check(&owner, &server).await)
        }
        #[cfg(not(feature = "work-product"))]
        {
            let _ = profile;
            Err(WorkError::Unavailable)
        }
    }
    .await;
    result.unwrap_or_else(|error| check_failed(expected_profile, key, error))
}

#[cfg(feature = "work-product")]
type SignIns =
    std::collections::HashMap<(String, String), (String, tokio::sync::watch::Sender<bool>)>;
#[cfg(feature = "work-product")]
fn sign_ins() -> &'static std::sync::Mutex<SignIns> {
    static PENDING: std::sync::OnceLock<std::sync::Mutex<SignIns>> = std::sync::OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// The browser tab owner cancels only the flow that owns the closed tab.
#[tauri::command]
#[specta::specta]
pub(crate) fn work_cancel_connection_sign_in(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    id: String,
    request_id: String,
) -> bool {
    if admitted(&caller, &app, "work_cancel_connection_sign_in").is_err()
        || profile_of(&expected_profile).is_err()
    {
        return false;
    }
    #[cfg(feature = "work-product")]
    {
        let pending = sign_ins().lock().unwrap_or_else(|p| p.into_inner());
        if let Some((request, cancel)) = pending.get(&(expected_profile, id)) {
            if *request == request_id {
                return cancel.send(true).is_ok();
            }
        }
    }
    #[cfg(not(feature = "work-product"))]
    let _ = (id, request_id);
    false
}

#[cfg(feature = "work-product")]
struct SignInLease {
    app: tauri::AppHandle,
    key: (String, String),
    request_id: String,
}
#[cfg(feature = "work-product")]
impl Drop for SignInLease {
    fn drop(&mut self) {
        {
            let mut pending = sign_ins().lock().unwrap_or_else(|p| p.into_inner());
            if pending
                .get(&self.key)
                .is_some_and(|(request, _)| request == &self.request_id)
            {
                pending.remove(&self.key);
            }
        }
        emit_to_privileged(
            &self.app,
            MAIN_LABEL,
            "zephium:connection-sign-in",
            &serde_json::json!({
                "version": 1, "phase": "finished", "profile": self.key.0, "id": self.key.1, "request_id": self.request_id,
            }),
        );
    }
}

#[cfg(feature = "work-product")]
async fn sign_in_flow(
    app: &tauri::AppHandle,
    owner: &str,
    id: &str,
    url: &str,
    tokens: std::sync::Arc<dyn zephium_app::work_connections::client::oauth::TokenStore>,
) -> Result<(), zephium_app::work_connections::client::McpError> {
    static NEXT_SIGN_IN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let request_id = NEXT_SIGN_IN
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .to_string();
    let pending_key = (owner.to_owned(), id.to_owned());
    let (cancel, receiver) = tokio::sync::watch::channel(false);
    let cancel_page = cancel.clone();
    if let Some((_, old)) = sign_ins()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(pending_key.clone(), (request_id.to_owned(), cancel))
    {
        let _ = old.send(true);
    }
    let lease = SignInLease {
        app: app.clone(),
        key: pending_key.clone(),
        request_id: request_id.clone(),
    };
    emit_to_privileged(
        app,
        MAIN_LABEL,
        "zephium:connection-sign-in",
        &serde_json::json!({
            "version": 1, "phase": "started", "profile": owner, "id": id, "request_id": request_id,
        }),
    );
    let opener = app.clone();
    let (flow, flow_profile, flow_server) =
        (request_id.to_owned(), owner.to_owned(), id.to_owned());
    let signed = zephium_app::work_connections::client::oauth::sign_in_cancellable(
        url,
        tokens,
        move |page| {
            if zephium_core::navigation::is_allowed_str(&page) {
                let shell = opener.state::<Handle>();
                let admission = dispatch_operation(
                    &opener,
                    &shell,
                    Command::OpenUrl {
                        input: page,
                        new_tab: true,
                    },
                );
                if !admission.accepted { let _ = cancel_page.send(true); return; }
                emit_to_privileged(
                    &opener,
                    MAIN_LABEL,
                    "zephium:connection-sign-in",
                    &serde_json::json!({
                        "version": 1, "phase": "open_requested", "profile": flow_profile, "id": flow_server,
                        "request_id": flow, "operation_id": admission.operation_id,
                    }),
                );
            } else { let _ = cancel_page.send(true); }
        },
        receiver,
    )
    .await;
    drop(lease);
    signed
}

/// Signs in to an HTTP server in a new tab, then checks it.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_sign_in_connection(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    id: String,
) -> WorkServerCheckV1 {
    let key = id.clone();
    let result = async {
        admitted(&caller, &app, "work_sign_in_connection")?;
        let profile = profile_of(&expected_profile)?;
        #[cfg(feature = "work-product")]
        {
            let _guard = connection_mutations().lock().await;
            use zephium_ipc::work::WorkServerTransportV1;
            let store = live::store(&app)?;
            let owner = profile.to_string();
            let server = store
                .servers(&owner)
                .map_err(|_| WorkError::Unavailable)?
                .into_iter()
                .find(|s| s.id == id)
                .ok_or(WorkError::NotFound)?;
            let WorkServerTransportV1::Http {
                url,
                auth: zephium_ipc::work::WorkServerAuthV1::OAuth,
            } = &server.transport
            else {
                return Err(WorkError::Invalid);
            };
            // Browser sign-in can wait for minutes. Revoke only around actual
            // credential writes, leaving existing and unrelated sessions usable.
            let tokens = store.track_tokens(
                &owner,
                &server.id,
                zephium_app::work_connections::client::keychain::oauth_store(&owner, &server.id),
            );
            let signed = sign_in_flow(&app, &owner, &id, url, tokens).await;
            use zephium_app::work_connections::client::McpError;
            let check = match signed {
                Ok(()) => zephium_app::work_connections::mcp::check(&owner, &server).await,
                Err(error) => WorkServerCheckV1 {
                    version: 1,
                    profile: owner,
                    id: server.id.clone(),
                    server_name: None,
                    tools: vec![],
                    error: None,
                    outcome: match error {
                        McpError::Cancelled | McpError::Timeout => WorkServerOutcomeV1::Cancelled,
                        McpError::Unauthorized => WorkServerOutcomeV1::SignIn,
                        _ => WorkServerOutcomeV1::Failed,
                    },
                },
            };
            Ok(check)
        }
        #[cfg(not(feature = "work-product"))]
        {
            let _ = profile;
            Err(WorkError::Unavailable)
        }
    }
    .await;
    result.unwrap_or_else(|error| check_failed(expected_profile, key, error))
}
