//! Per-profile DNR compilation, durable state and sealed snapshot preparation.
//! WebSocket handshakes and response-header enforcement are unavailable in
//! WebKitGTK's send-request hook; reports preserve those limitations.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;

use serde_json::Value;
use sha2::{Digest, Sha256};
use zephium_core::ids::ExtensionInstallId;
use zephium_webext::manifest::{normalize_resource, Manifest};
use zephium_webext_dnr::{
    parse_rules_json, CompiledRuleset, Rule, RulesetKind, DYNAMIC_RULESET_ID, INDEX_FORMAT_VERSION,
    MAX_INDEX_BYTES, MAX_NUMBER_OF_STATIC_RULESETS, MAX_RULESET_JSON_BYTES, SESSION_RULESET_ID,
};
use zephium_webext_runtime::api::declarative_net_request::{DnrState, Request, StaticRuleset};
use zephium_webext_runtime::errors::ApiError;
use zephium_webext_runtime::permissions::InstallPermissions;
use zephium_webkit_web_process_protocol::dnr::{
    DnrExtensionPolicy, DnrPolicySnapshot, DnrResourceAccess,
};

use crate::bridge::SealedPolicy;
use crate::package::{PreparedPackage, ResourcePath};

const MAX_REPORTED_RULES: usize = 256;
const MAX_SOURCE_BYTES_PER_INSTALL: usize = 256 * 1024 * 1024;
const MAX_STATE_BYTES: usize = 64 * 1024 * 1024;

/// One rule the Linux request hook cannot fully enforce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedRule {
    /// Manifest ruleset id, `_dynamic` or `_session`.
    pub ruleset_id: String,
    /// The source rule's id, when it was valid enough to identify.
    pub rule_id: Option<u32>,
    /// A bounded diagnostic suitable for extension review and details.
    pub reason: String,
}

/// Install-review and extension-details projection, without browsing URLs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkStatus {
    /// Engine limitations applying even when no rule declares a resource type.
    pub limitations: Vec<String>,
    /// Profile observation rows lost to the module's bounded reporting queue.
    pub dropped_observations: u64,
    /// Profile match rows lost; action counts may therefore be incomplete.
    pub dropped_matches: u64,
    /// Rule loading has not settled yet.
    pub loading: bool,
    /// Rules included in this install's published indexes.
    pub active_rule_count: usize,
    /// Total unsupported or rejected source rules.
    pub unsupported_rule_count: usize,
    /// First 256 unsupported or rejected rules.
    pub unsupported_rules: Vec<UnsupportedRule>,
    /// The last preparation error. The preceding policy remains in force.
    pub error: Option<String>,
}

impl NetworkStatus {
    fn unsupported(&mut self, ruleset: &str, id: Option<u32>, reason: &str) {
        self.unsupported_rule_count += 1;
        if self.unsupported_rules.len() < MAX_REPORTED_RULES {
            self.unsupported_rules.push(UnsupportedRule {
                ruleset_id: ruleset.to_owned(),
                rule_id: id,
                reason: reason.chars().take(512).collect(),
            });
        }
    }
}

pub(crate) struct Install {
    pub install: ExtensionInstallId,
    pub epoch: u64,
    pub extension_id: String,
    pub install_order: u64,
    pub root: PathBuf,
    pub manifest: Manifest,
    pub permissions: InstallPermissions,
    pub state_directory: Option<PathBuf>,
    pub fresh: bool,
    pub updated: bool,
}

pub(crate) enum Job {
    Install(Install),
    Call {
        install: ExtensionInstallId,
        epoch: u64,
        ticket: u64,
        request: Request,
    },
    Permissions {
        install: ExtensionInstallId,
        epoch: u64,
        ticket: u64,
        permissions: InstallPermissions,
    },
    Feedback {
        install: ExtensionInstallId,
        epoch: u64,
        ruleset_id: String,
        rule_id: u32,
        tab_id: i32,
        time_stamp: f64,
    },
    Remove {
        install: ExtensionInstallId,
    },
    Navigation {
        tab_id: i32,
    },
    TabClosed {
        tab_id: i32,
        now_ms: u64,
    },
}

pub(crate) struct Update {
    pub state: DnrState,
    pub value: Value,
    pub status: NetworkStatus,
    pub policy: Option<SealedPolicy>,
}

pub(crate) struct Done {
    pub install: ExtensionInstallId,
    pub epoch: u64,
    pub ticket: Option<u64>,
    pub revision: u64,
    pub prepared: bool,
    pub removed: bool,
    pub result: Result<Update, ApiError>,
}

/// A FIFO worker keeps install preparation and API updates in issue order.
pub(crate) struct NetworkExecutor {
    jobs: Option<mpsc::Sender<Job>>,
    decisions: mpsc::Sender<(u64, bool)>,
    done: mpsc::Receiver<Done>,
    wake: UnixStream,
    thread: Option<JoinHandle<()>>,
}

impl NetworkExecutor {
    pub(crate) fn start() -> std::io::Result<Self> {
        let (wake, signal) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        signal.set_nonblocking(true)?;
        let (jobs, inbox) = mpsc::channel();
        let (outbox, done) = mpsc::channel();
        let (decisions, approvals) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("zephium-webext-network".into())
            .spawn(move || run(inbox, approvals, outbox, signal))?;
        Ok(Self {
            jobs: Some(jobs),
            decisions,
            done,
            wake,
            thread: Some(thread),
        })
    }

    pub(crate) fn submit(&self, job: Job) -> bool {
        self.jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok())
    }

    pub(crate) fn fd(&self) -> std::os::fd::RawFd {
        self.wake.as_raw_fd()
    }

    pub(crate) fn finish(&self, revision: u64, commit: bool) -> bool {
        self.decisions.send((revision, commit)).is_ok()
    }

    pub(crate) fn take(&self) -> Vec<Done> {
        let mut buffer = [0; 64];
        let mut wake = &self.wake;
        while matches!(wake.read(&mut buffer), Ok(count) if count > 0) {}
        self.done.try_iter().collect()
    }

    pub(crate) fn stop(&mut self) {
        self.jobs.take();
        let _ = self.decisions.send((u64::MAX, false));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for NetworkExecutor {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Prepared {
    epoch: u64,
    state: DnrState,
    policy: DnrExtensionPolicy,
    directory: Option<PathBuf>,
    cache: HashMap<[u8; 32], Vec<u8>>,
    source_status: NetworkStatus,
    install_digest: [u8; 32],
}

fn failed() -> ApiError {
    ApiError::other("Declarative network request rules could not be prepared.")
}

struct Staged {
    persist: bool,
    state: DnrState,
    value: Value,
    status: NetworkStatus,
    extension: Option<DnrExtensionPolicy>,
    new_install: Option<Prepared>,
    removed: bool,
}

fn emit(outbox: &mpsc::Sender<Done>, signal: &mut UnixStream, done: Done) -> bool {
    if outbox.send(done).is_err() {
        return false;
    }
    match signal.write(&[1]) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::WouldBlock => true,
        Err(_) => false,
    }
}

fn run(
    inbox: mpsc::Receiver<Job>,
    decisions: mpsc::Receiver<(u64, bool)>,
    outbox: mpsc::Sender<Done>,
    mut signal: UnixStream,
) {
    let mut installs = BTreeMap::<ExtensionInstallId, Prepared>::new();
    let mut revision = 0u64;
    for job in inbox {
        let (install, epoch, ticket, result) = match job {
            Job::Install(install) => {
                let id = install.install;
                let epoch = install.epoch;
                let result = prepare_install(install).and_then(|mut prepared| {
                    let state = prepared.state.clone();
                    let (indexes, status) = compile_state(&mut prepared, &state)?;
                    prepared.policy.rulesets = indexes;
                    Ok(Staged {
                        persist: true,
                        state,
                        value: Value::Null,
                        status,
                        extension: Some(prepared.policy.clone()),
                        new_install: Some(prepared),
                        removed: false,
                    })
                });
                (id, epoch, None, result)
            }
            Job::Call {
                install,
                epoch,
                ticket,
                request,
            } => {
                let result = stage_call(&mut installs, install, epoch, &request);
                (install, epoch, Some(ticket), result)
            }
            Job::Permissions {
                install,
                epoch,
                ticket,
                permissions,
            } => {
                let result = installs
                    .get_mut(&install)
                    .filter(|prepared| prepared.epoch == epoch)
                    .ok_or_else(ApiError::context_invalidated)
                    .and_then(|prepared| {
                        let state = prepared.state.clone();
                        let (indexes, status) = compile_state(prepared, &state)?;
                        let mut extension = prepared.policy.clone();
                        extension.rulesets = indexes;
                        extension.host_permissions = permissions.snapshot().origins;
                        extension.require_host_access =
                            !permissions.has_api("declarativeNetRequest");
                        if !permissions.has_api("declarativeNetRequest")
                            && !permissions.has_api("declarativeNetRequestWithHostAccess")
                        {
                            extension.rulesets.clear();
                        }
                        Ok(Staged {
                            persist: false,
                            state,
                            value: Value::Null,
                            status,
                            extension: Some(extension),
                            new_install: None,
                            removed: false,
                        })
                    });
                (install, epoch, Some(ticket), result)
            }
            Job::Feedback {
                install,
                epoch,
                ruleset_id,
                rule_id,
                tab_id,
                time_stamp,
            } => {
                if let Some(prepared) = installs
                    .get_mut(&install)
                    .filter(|item| item.epoch == epoch)
                {
                    prepared
                        .state
                        .record_match(&ruleset_id, rule_id, tab_id, time_stamp);
                }
                continue;
            }
            Job::Remove { install } => {
                let epoch = installs.get(&install).map_or(0, |previous| previous.epoch);
                (
                    install,
                    epoch,
                    None,
                    Ok(Staged {
                        persist: false,
                        state: DnrState::default(),
                        value: Value::Null,
                        status: NetworkStatus::default(),
                        extension: None,
                        new_install: None,
                        removed: true,
                    }),
                )
            }
            Job::Navigation { tab_id } => {
                for prepared in installs.values_mut() {
                    prepared.state.navigation_committed(tab_id);
                }
                continue;
            }
            Job::TabClosed { tab_id, now_ms } => {
                for prepared in installs.values_mut() {
                    prepared.state.tab_closed(tab_id, now_ms);
                }
                continue;
            }
        };
        revision += 1;
        let mut staged = match result {
            Ok(staged) => staged,
            Err(error) => {
                if !emit(
                    &outbox,
                    &mut signal,
                    Done {
                        install,
                        epoch,
                        ticket,
                        revision,
                        prepared: false,
                        removed: false,
                        result: Err(error),
                    },
                ) {
                    return;
                }
                continue;
            }
        };
        let changes_policy = staged.extension.is_some() || staged.removed;
        if !changes_policy && staged.persist {
            let result = installs
                .get(&install)
                .and_then(|prepared| prepared.directory.as_ref())
                .map_or(Ok(()), |directory| {
                    write_atomic(&directory.join("dnr.json"), &staged.state.snapshot()?).map_err(
                        |_| {
                            ApiError::other("Declarative network request rules could not be saved.")
                        },
                    )
                });
            if let Err(error) = result {
                if !emit(
                    &outbox,
                    &mut signal,
                    Done {
                        install,
                        epoch,
                        ticket,
                        revision,
                        prepared: false,
                        removed: false,
                        result: Err(error),
                    },
                ) {
                    return;
                }
                continue;
            }
        }
        let policy = if changes_policy {
            match snapshot(
                &installs,
                staged
                    .extension
                    .as_ref()
                    .map(|extension| (install, extension)),
                staged.removed.then_some(install),
            ) {
                Ok(policy) => Some(policy),
                Err(error) => {
                    if !emit(
                        &outbox,
                        &mut signal,
                        Done {
                            install,
                            epoch,
                            ticket,
                            revision,
                            prepared: false,
                            removed: staged.removed,
                            result: Err(error),
                        },
                    ) {
                        return;
                    }
                    continue;
                }
            }
        } else {
            None
        };
        if let Some(policy) = policy {
            let directory = staged
                .new_install
                .as_ref()
                .and_then(|prepared| prepared.directory.as_ref())
                .or_else(|| {
                    installs
                        .get(&install)
                        .and_then(|prepared| prepared.directory.as_ref())
                });
            let persistence = if !staged.removed && staged.persist {
                directory
                    .map(|directory| {
                        PendingWrite::prepare(
                            &directory.join("dnr.json"),
                            &staged.state.snapshot()?,
                        )
                        .map_err(|_| {
                            ApiError::other("Declarative network request rules could not be saved.")
                        })
                    })
                    .transpose()
            } else {
                Ok(None)
            };
            let persistence = match persistence {
                Ok(persistence) => persistence,
                Err(error) => {
                    if !emit(
                        &outbox,
                        &mut signal,
                        Done {
                            install,
                            epoch,
                            ticket,
                            revision,
                            prepared: false,
                            removed: staged.removed,
                            result: Err(error),
                        },
                    ) {
                        return;
                    }
                    continue;
                }
            };
            if !emit(
                &outbox,
                &mut signal,
                Done {
                    install,
                    epoch,
                    ticket,
                    revision,
                    prepared: true,
                    removed: staged.removed,
                    result: Ok(Update {
                        state: staged.state.clone(),
                        value: staged.value.clone(),
                        status: staged.status.clone(),
                        policy: Some(policy),
                    }),
                },
            ) {
                return;
            }
            let commit = loop {
                match decisions.recv() {
                    Ok((u64::MAX, _)) | Err(_) => return,
                    Ok((current, commit)) if current == revision => break commit,
                    Ok(_) => {}
                }
            };
            let result = if commit {
                persistence.map_or(Ok(()), |write| {
                    write.commit().map_err(|_| {
                        ApiError::other("Declarative network request rules could not be saved.")
                    })
                })
            } else {
                Err(ApiError::other(
                    "Declarative network request policy was refused by a web process.",
                ))
            };
            if let Err(error) = result {
                if !emit(
                    &outbox,
                    &mut signal,
                    Done {
                        install,
                        epoch,
                        ticket,
                        revision,
                        prepared: false,
                        removed: staged.removed,
                        result: Err(error),
                    },
                ) {
                    return;
                }
                continue;
            }
        }
        if staged.removed {
            installs.remove(&install);
        } else if let Some(prepared) = staged.new_install.take() {
            installs.insert(install, prepared);
        } else if let Some(prepared) = installs.get_mut(&install) {
            prepared.state = staged.state.clone();
            if let Some(extension) = staged.extension {
                prepared.policy = extension;
            }
        }
        if !emit(
            &outbox,
            &mut signal,
            Done {
                install,
                epoch,
                ticket,
                revision,
                prepared: false,
                removed: staged.removed,
                result: Ok(Update {
                    state: staged.state,
                    value: staged.value,
                    status: staged.status,
                    policy: None,
                }),
            },
        ) {
            return;
        }
    }
}

fn stage_call(
    installs: &mut BTreeMap<ExtensionInstallId, Prepared>,
    install: ExtensionInstallId,
    epoch: u64,
    request: &Request,
) -> Result<Staged, ApiError> {
    let prepared = installs
        .get_mut(&install)
        .filter(|item| item.epoch == epoch)
        .ok_or_else(ApiError::context_invalidated)?;
    let (state, value) = prepared.state.execute(request)?;
    let (extension, status) = if request.changes_policy() {
        let (indexes, status) = compile_state(prepared, &state)?;
        let mut extension = prepared.policy.clone();
        extension.rulesets = indexes;
        (Some(extension), status)
    } else {
        (None, prepared.source_status.clone())
    };
    Ok(Staged {
        persist: request.changes_persistent_state(),
        state,
        value,
        status,
        extension,
        new_install: None,
        removed: false,
    })
}

fn snapshot(
    installs: &BTreeMap<ExtensionInstallId, Prepared>,
    replacement: Option<(ExtensionInstallId, &DnrExtensionPolicy)>,
    omit: Option<ExtensionInstallId>,
) -> Result<SealedPolicy, ApiError> {
    let mut extensions: Vec<_> = installs
        .iter()
        .filter(|(id, _)| {
            Some(**id) != omit
                && replacement
                    .as_ref()
                    .is_none_or(|(replace, _)| *id != replace)
        })
        .map(|(_, prepared)| prepared.policy.clone())
        .collect();
    if let Some((_, replacement)) = replacement {
        extensions.push(replacement.clone());
    }
    extensions.sort_by_key(|extension| extension.install_order);
    let bytes = DnrPolicySnapshot::encode(&extensions).map_err(|_| failed())?;
    SealedPolicy::new(&bytes).map_err(|_| failed())
}

fn prepare_install(install: Install) -> Result<Prepared, ApiError> {
    let mut source_status = NetworkStatus {
        limitations: vec![
            "WebSocket handshakes and service-worker-initiated fetches bypass the request hook.".into(),
            "WebTransport and webbundle resource enforcement is unavailable.".into(),
            "Response headers and response-header conditions cannot be enforced.".into(),
            "Static capacity is the guaranteed 30000 rules per install; an additional shared pool is not allocated.".into(),
            "Publication swaps each page atomically, but pages acknowledge asynchronously; a failed final durable commit requires rollback.".into(),
            "Subframe allowAllRequests cannot establish a frame-subtree exemption; top-document inheritance is supported.".into(),
            "Originating frame identities are unavailable for subresources; on non-HTTPS public hosts scripts, fonts, fetches, workers and pings are typed other.".into(),
        ],
        ..NetworkStatus::default()
    };
    let resources = install.manifest.raw()["declarative_net_request"]["rule_resources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if resources.len() > MAX_NUMBER_OF_STATIC_RULESETS {
        return Err(failed());
    }
    let mut statics = Vec::new();
    let mut source_bytes = 0usize;
    let package = PreparedPackage::open(&install.root).map_err(|_| failed())?;
    let mut install_digest = Sha256::new();
    install_digest.update(serde_json::to_vec(install.manifest.raw()).map_err(|_| failed())?);
    for resource in resources {
        let id = resource["id"]
            .as_str()
            .filter(|id| !id.is_empty() && !id.starts_with('_') && id.len() <= 1024)
            .ok_or_else(failed)?
            .to_owned();
        let path = resource["path"]
            .as_str()
            .and_then(normalize_resource)
            .ok_or_else(failed)?;
        let resource_path = ResourcePath::parse(&format!("/{path}")).map_err(|_| failed())?;
        let source = package
            .open_resource(&resource_path)
            .map_err(|_| failed())?;
        if source.length > MAX_RULESET_JSON_BYTES as u64 {
            return Err(failed());
        }
        let mut bytes = Vec::new();
        source
            .file
            .take(MAX_RULESET_JSON_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| failed())?;
        if bytes.len() > MAX_RULESET_JSON_BYTES {
            return Err(failed());
        }
        install_digest.update(id.as_bytes());
        install_digest.update(&bytes);
        source_bytes = source_bytes
            .checked_add(bytes.len())
            .filter(|bytes| *bytes <= MAX_SOURCE_BYTES_PER_INSTALL)
            .ok_or_else(failed)?;
        let parsed = parse_rules_json(&bytes, RulesetKind::Static).map_err(|_| failed())?;
        for rejection in parsed.rejected {
            source_status.unsupported(&id, rejection.id, &rejection.reason);
        }
        statics.push(StaticRuleset {
            id,
            enabled: resource["enabled"].as_bool().unwrap_or(false),
            rules: parsed.rules,
        });
    }
    let mut state = DnrState::from_static(statics)?;
    if !install.fresh {
        if let Some(directory) = &install.state_directory {
            match read_bounded(&directory.join("dnr.json"), MAX_STATE_BYTES) {
                Ok(bytes) if install.updated => state.restore_after_update(&bytes)?,
                Ok(bytes) => state.restore(&bytes)?,
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(ApiError::other(
                        "Declarative network request state could not be restored.",
                    ))
                }
            }
        }
    }
    state.check_static_limits()?;
    let permissions = install.permissions.snapshot();
    if install
        .manifest
        .web_accessible_resources()
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["use_dynamic_url"].as_bool() == Some(true))
        })
    {
        source_status.limitations.push("DNR extensionPath redirects to resources requiring dynamic WAR aliases are unavailable.".into());
    }
    let web_accessible_resources = resource_access(&install.manifest);
    Ok(Prepared {
        epoch: install.epoch,
        state,
        policy: DnrExtensionPolicy {
            extension_id: install.extension_id,
            install_order: install.install_order,
            host_permissions: permissions.origins,
            require_host_access: !install.permissions.has_api("declarativeNetRequest"),
            web_accessible_resources,
            rulesets: Vec::new(),
        },
        directory: install.state_directory,
        cache: HashMap::new(),
        source_status,
        install_digest: install_digest.finalize().into(),
    })
}

fn resource_access(manifest: &Manifest) -> Vec<DnrResourceAccess> {
    let Some(entries) = manifest
        .web_accessible_resources()
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let strings = |value: &Value| {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    if manifest.manifest_version() == 2 {
        return vec![DnrResourceAccess {
            resources: entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            matches: vec!["<all_urls>".into()],
            extension_ids: Vec::new(),
        }];
    }
    entries
        .iter()
        .filter(|entry| entry["use_dynamic_url"].as_bool() != Some(true))
        .map(|entry| DnrResourceAccess {
            resources: strings(&entry["resources"]),
            matches: strings(&entry["matches"]),
            extension_ids: strings(&entry["extension_ids"]),
        })
        .collect()
}

fn compile_state(
    prepared: &mut Prepared,
    state: &DnrState,
) -> Result<(Vec<Vec<u8>>, NetworkStatus), ApiError> {
    let mut status = prepared.source_status.clone();
    let mut indexes = Vec::new();
    for ruleset in state
        .static_rulesets
        .iter()
        .filter(|ruleset| ruleset.enabled)
    {
        let disabled = state.disabled_static_rules.get(&ruleset.id);
        let rules: Vec<_> = ruleset
            .rules
            .iter()
            .filter(|rule| disabled.is_none_or(|ids| !ids.contains(&rule.id)))
            .cloned()
            .collect();
        indexes.push(compile(
            prepared,
            &ruleset.id,
            RulesetKind::Static,
            rules,
            disabled.is_none_or(|ids| ids.is_empty()),
            &mut status,
        )?);
    }
    for (id, kind) in [
        (DYNAMIC_RULESET_ID, RulesetKind::Dynamic),
        (SESSION_RULESET_ID, RulesetKind::Session),
    ] {
        let rules = state.rules(kind)?;
        if !rules.is_empty() {
            indexes.push(compile(prepared, id, kind, rules, false, &mut status)?);
        }
    }
    Ok((indexes, status))
}

fn compile(
    prepared: &mut Prepared,
    id: &str,
    kind: RulesetKind,
    rules: Vec<Rule>,
    persistent_cache: bool,
    status: &mut NetworkStatus,
) -> Result<Vec<u8>, ApiError> {
    let rules: Vec<_> = rules.into_iter().filter(|rule| {
        if rule.condition.resource_types.as_ref().is_some_and(|types| types.iter().any(|kind| matches!(kind, zephium_webext_dnr::ResourceType::Websocket | zephium_webext_dnr::ResourceType::Webtransport | zephium_webext_dnr::ResourceType::Webbundle))) {
            status.unsupported(id, Some(rule.id), "WebSocket, WebTransport and webbundle requests are unavailable to the Linux request hook.");
        }
        if matches!(rule.action, zephium_webext_dnr::Action::AllowAllRequests) && rule.condition.resource_types.as_ref().is_some_and(|types| types.contains(&zephium_webext_dnr::ResourceType::SubFrame)) {
            status.unsupported(id, Some(rule.id), "Subframe allowAllRequests cannot exempt its descendant frame tree on Linux.");
        }
        if matches!(&rule.action, zephium_webext_dnr::Action::Redirect { redirect } if redirect.extension_path.is_some()) {
            // Dynamic WAR aliases are supplied by the package scheme, not DNR.
            if prepared.policy.web_accessible_resources.is_empty() {
                status.unsupported(id, Some(rule.id), "Extension redirects require a matching non-dynamic web-accessible resource declaration.");
            }
        }
        if rule.condition.has_response_header_condition() {
            status.unsupported(id, Some(rule.id), "Response-header conditions are unavailable on Linux.");
            return false;
        }
        if rule.action.has_unsupported_response_header_modifications() {
            status.unsupported(id, Some(rule.id), "Response-header modification is unavailable on Linux.");
        }
        true
    }).collect();
    status.active_rule_count += rules.len();
    let source = serde_json::to_vec(&rules).map_err(|_| failed())?;
    let mut digest = Sha256::new();
    digest.update(prepared.install_digest);
    digest.update(INDEX_FORMAT_VERSION.to_le_bytes());
    digest.update(id.as_bytes());
    digest.update([kind as u8]);
    digest.update(&source);
    let key: [u8; 32] = digest.finalize().into();
    if let Some(bytes) = prepared.cache.get(&key) {
        return Ok(bytes.clone());
    }
    let cache_path = prepared
        .directory
        .as_ref()
        .filter(|_| persistent_cache)
        .map(|directory| {
            let name: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
            directory.join("dnr-cache").join(name)
        });
    let cached = cache_path
        .as_ref()
        .and_then(|path| read_bounded(path, MAX_INDEX_BYTES).ok())
        .filter(|bytes| CompiledRuleset::load(bytes).is_ok());
    let bytes = match cached {
        Some(bytes) => bytes,
        None => {
            let bytes = CompiledRuleset::compile(id, kind, rules)
                .map_err(|_| failed())?
                .to_bytes();
            if let Some(path) = cache_path {
                let _ = write_atomic(&path, &bytes);
            }
            bytes
        }
    };
    if kind == RulesetKind::Static {
        if prepared.cache.len() >= MAX_NUMBER_OF_STATIC_RULESETS
            || prepared
                .cache
                .values()
                .map(Vec::len)
                .sum::<usize>()
                .saturating_add(bytes.len())
                > MAX_INDEX_BYTES
        {
            prepared.cache.clear();
        }
        prepared.cache.insert(key, bytes.clone());
    }
    Ok(bytes)
}

fn read_bounded(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

struct PendingWrite {
    path: PathBuf,
    temporary: PathBuf,
    directory: File,
}

impl PendingWrite {
    fn prepare(path: &Path, bytes: &[u8]) -> std::io::Result<Self> {
        let parent = path.parent().ok_or(ErrorKind::InvalidInput)?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let pending = Self {
            path: path.to_owned(),
            temporary: path.with_extension("tmp"),
            directory: File::open(parent)?,
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&pending.temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(pending)
    }

    fn commit(self) -> std::io::Result<()> {
        std::fs::rename(&self.temporary, &self.path)?;
        self.directory.sync_all()
    }
}

impl Drop for PendingWrite {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temporary);
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    PendingWrite::prepare(path, bytes)?.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{Duration, Instant};
    use zephium_webext_runtime::permissions::{
        PermissionDeclaration, PermissionRequest, SiteAccess,
    };

    fn installation(root: &Path, directory: &Path) -> Install {
        let manifest = Manifest::from_value(json!({
            "manifest_version": 3, "name": "network fixture", "version": "1",
            "permissions": ["declarativeNetRequest"],
            "declarative_net_request": {"rule_resources": [
                {"id": "main", "path": "rules.json", "enabled": true}
            ]}
        }));
        let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
        Install {
            install: ExtensionInstallId::from(1),
            epoch: 1,
            extension_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            install_order: 1,
            root: root.to_owned(),
            manifest,
            permissions: InstallPermissions::new(
                declaration,
                &PermissionRequest::default(),
                SiteAccess::OnAllSites,
                false,
            ),
            state_directory: Some(directory.to_owned()),
            fresh: true,
            updated: false,
        }
    }

    fn completion(executor: &NetworkExecutor) -> Done {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut done = executor.take();
            if !done.is_empty() {
                assert_eq!(done.len(), 1);
                return done.remove(0);
            }
            assert!(Instant::now() < deadline, "network worker did not settle");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn package_updates_restore_dynamic_rules_without_obsolete_static_ids() {
        let root = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("rules.json"), b"[]").unwrap();
        let mut install = installation(root.path(), directory.path());
        let mut previous = DnrState::default();
        previous.dynamic_rules =
            vec![json!({"id": 1, "action": {"type": "block"}, "condition": {"urlFilter": "ads"}})];
        previous.static_rulesets.push(StaticRuleset {
            id: "obsolete".into(),
            enabled: true,
            rules: vec![],
        });
        std::fs::write(
            directory.path().join("dnr.json"),
            previous.snapshot().unwrap(),
        )
        .unwrap();
        install.fresh = false;
        assert!(prepare_install(install).is_err());
        let mut install = installation(root.path(), directory.path());
        install.fresh = false;
        install.updated = true;
        let prepared = prepare_install(install).unwrap();
        assert_eq!(prepared.state.dynamic_rules, previous.dynamic_rules);
        assert_eq!(prepared.state.static_rulesets[0].id, "main");
        assert!(prepared.state.static_rulesets[0].enabled);
    }

    #[test]
    fn refused_publication_preserves_durable_state_and_empty_removal_settles() {
        let root = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("rules.json"), b"[]").unwrap();
        let executor = NetworkExecutor::start().unwrap();
        assert!(executor.submit(Job::Install(installation(root.path(), directory.path()))));
        let prepared = completion(&executor);
        assert!(prepared.prepared && prepared.result.is_ok());
        assert!(executor.finish(prepared.revision, false));
        let refused = completion(&executor);
        assert!(refused.result.is_err());
        assert!(!directory.path().join("dnr.json").exists());
        assert!(executor.submit(Job::Remove {
            install: ExtensionInstallId::from(1)
        }));
        let removed = completion(&executor);
        assert!(removed.prepared && removed.removed);
        assert!(executor.finish(removed.revision, true));
        let removed = completion(&executor);
        assert!(removed.removed && removed.result.is_ok());
    }
}
