#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_lite::{future, StreamExt};
use serde::Serialize;
use zbus::names::OwnedUniqueName;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{DynamicType, ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::linux_shortcut::{LinuxLauncherShortcut, LAUNCHER_COMMAND_ID};

const PORTAL_DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const HOST_REGISTRY_INTERFACE: &str = "org.freedesktop.host.portal.Registry";
const GLOBAL_SHORTCUTS_INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";
const SESSION_INTERFACE: &str = "org.freedesktop.portal.Session";
const REQUEST_PATH_PREFIX: &str = "/org/freedesktop/portal/desktop/request";
const SESSION_PATH_PREFIX: &str = "/org/freedesktop/portal/desktop/session";
const PORTAL_DESKTOP_ID: &str = env!("ZEPHIUM_LINUX_APP_ID");
const SHORTCUT_ID: &str = LAUNCHER_COMMAND_ID;
const SHORTCUT_DESCRIPTION: &str = "Toggle Zephium launcher";
const CANCELLATION_POLL: Duration = Duration::from_millis(100);
const RECONNECT_BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];
const MAX_PORTAL_ATTEMPTS: usize = RECONNECT_BACKOFF.len() + 1;
const MAX_SHORTCUT_RESULTS: usize = 16;
const MAX_SHORTCUT_ID_BYTES: usize = 128;
const MAX_SHORTCUT_PROPERTIES: usize = 16;
const MAX_SHORTCUT_SIGNAL_BODY_BYTES: usize = 64 * 1024;
const MAX_PORTAL_RESPONSE_BODY_BYTES: usize = 64 * 1024;
const MAX_ACTIVATION_OPTIONS: usize = 16;
const MAX_ACTIVATION_TOKEN_BYTES: usize = 4096;
const MAX_ACTIVATION_SIGNAL_BODY_BYTES: usize = 64 * 1024;
const MAX_MALFORMED_SIGNAL_LOGS: usize = 8;
const PORTAL_TOKEN_NONCE_BYTES: usize = 16;
const PORTAL_CALL_DEADLINE: Duration = Duration::from_secs(5);
const PORTAL_CLOSE_DEADLINE: Duration = Duration::from_millis(500);

static NEXT_PORTAL_TOKEN: AtomicU64 = AtomicU64::new(1);
static MALFORMED_SIGNAL_LOGS_REMAINING: AtomicUsize = AtomicUsize::new(MAX_MALFORMED_SIGNAL_LOGS);

type Activation = Arc<dyn Fn(ActivationContext) + Send + Sync + 'static>;
type ShortcutList = Vec<(String, HashMap<String, OwnedValue>)>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ActivationContext {
    pub(crate) activation_token: Option<String>,
    pub(crate) timestamp: Option<u32>,
}

pub(crate) struct ActivationTarget {
    active: AtomicBool,
    activation: Mutex<Option<Activation>>,
}

#[derive(Clone, Default)]
pub(crate) struct GlobalRegistration {
    live: Arc<AtomicBool>,
}

impl GlobalRegistration {
    pub(crate) fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    pub(crate) fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Release);
    }
}

pub(crate) struct ClearRegistrationOnDrop(GlobalRegistration);

impl ClearRegistrationOnDrop {
    pub(crate) fn acquire(registration: &GlobalRegistration) -> Self {
        registration.set_live(true);
        Self(registration.clone())
    }
}

impl Drop for ClearRegistrationOnDrop {
    fn drop(&mut self) {
        self.0.set_live(false);
    }
}

impl ActivationTarget {
    pub(crate) fn new(activation: impl Fn(ActivationContext) + Send + Sync + 'static) -> Self {
        Self {
            active: AtomicBool::new(true),
            activation: Mutex::new(Some(Arc::new(activation))),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn activate(&self, context: ActivationContext) {
        // Hold the admission lock through the short scheduling callback. A
        // concurrent stop first revokes `active`, then takes this lock, so no
        // callback can begin after revocation and none remains in flight when
        // stop returns.
        let activation = lock_recover(&self.activation);
        if !self.is_active() {
            return;
        }
        if let Some(activation) = activation.as_ref() {
            activation(context);
        }
    }

    pub(crate) fn stop(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            lock_recover(&self.activation).take();
        }
    }
}

pub(crate) fn run(
    target: Weak<ActivationTarget>,
    registration: GlobalRegistration,
    shortcut: LinuxLauncherShortcut,
) -> Result<(), PortalError> {
    future::block_on(run_with_reconnect(
        target,
        registration,
        shortcut.portal_trigger(),
    ))
}

async fn run_with_reconnect(
    target: Weak<ActivationTarget>,
    registration: GlobalRegistration,
    trigger: String,
) -> Result<(), PortalError> {
    for (attempt, retry_delay) in RECONNECT_BACKOFF.iter().copied().enumerate() {
        match run_once(&target, &registration, &trigger).await {
            Ok(()) => return Ok(()),
            Err(PortalError::Stopped) => return Err(PortalError::Stopped),
            Err(error) if error.retryable() => {
                crate::write_diagnostic(format_args!(
                    "global shortcut: Wayland portal session ended ({error}); retrying {}/{}",
                    attempt + 2,
                    MAX_PORTAL_ATTEMPTS
                ));
                if !delay_until_retry(&target, retry_delay).await {
                    return Err(PortalError::Stopped);
                }
            }
            Err(error) => return Err(error),
        }
    }
    run_once(&target, &registration, &trigger).await
}

async fn run_once(
    target: &Weak<ActivationTarget>,
    registration: &GlobalRegistration,
    trigger: &str,
) -> Result<(), PortalError> {
    if !target_is_active(target) {
        return Err(PortalError::Stopped);
    }
    let connection = zbus::Connection::session()
        .await
        .map_err(|error| PortalError::transport("connect to the session bus", error))?;
    // Proxy construction is deliberately side-effect free. Install the
    // well-known-name watch and establish a stable owner checkpoint before
    // Registry is the first call made to the portal process.
    let portal_watch =
        portal_proxy(&connection, PORTAL_DESTINATION, GLOBAL_SHORTCUTS_INTERFACE).await?;
    let mut owner_changes = portal_watch
        .receive_owner_changed()
        .await
        .map_err(|error| PortalError::transport("watch the portal service owner", error))?;
    let owner_before_registration = stable_portal_owner(&connection, &mut owner_changes).await?;
    register_host_connection(&connection).await?;
    let owner_after_registration = current_portal_owner(&connection).await?;
    let transitions = drain_owner_changes(&mut owner_changes).await?;
    if !registration_owner_is_current(
        owner_before_registration.as_ref(),
        owner_after_registration.as_ref(),
        &transitions,
    ) {
        return Err(PortalError::Transport(
            "portal service owner changed while registering the host connection".to_owned(),
        ));
    }
    let owner = owner_after_registration.ok_or_else(|| {
        PortalError::Transport("portal service has no owner after host registration".to_owned())
    })?;
    ensure_portal_owner(&connection, &mut owner_changes, &owner).await?;

    // Pin every portal object call to the exact unique owner proven above.
    // Even if the well-known name changes in the final instruction window,
    // calls fail against the retired peer instead of crossing to an owner on
    // which this connection's Registry attempt never ran.
    let portal = portal_proxy(&connection, owner.as_str(), GLOBAL_SHORTCUTS_INTERFACE).await?;
    let version: u32 = portal_call_with_deadline(
        "read the global-shortcuts version",
        portal.get_property("version"),
    )
    .await?;
    if version < 1 {
        return Err(PortalError::UnsupportedVersion(version));
    }

    let create_token = next_portal_token("create")?;
    let session_token = next_portal_token("session")?;
    let session_path = predicted_portal_path(&connection, SESSION_PATH_PREFIX, &session_token)?;
    let create_options = HashMap::from([
        ("handle_token", Value::new(create_token.as_str())),
        ("session_handle_token", Value::new(session_token.as_str())),
    ]);
    let create_response = match portal_request(
        &connection,
        &portal,
        "CreateSession",
        &create_token,
        &create_options,
        &mut owner_changes,
        target,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            close_portal_object(
                &connection,
                portal.destination().as_str(),
                &session_path,
                SESSION_INTERFACE,
            )
            .await;
            return Err(error);
        }
    };
    let returned_session = match decode_session_handle(&create_response.results) {
        Ok(session) => session,
        Err(error) => {
            close_portal_object(
                &connection,
                portal.destination().as_str(),
                &session_path,
                SESSION_INTERFACE,
            )
            .await;
            return Err(error);
        }
    };
    if returned_session != session_path {
        close_portal_object(
            &connection,
            portal.destination().as_str(),
            &returned_session,
            SESSION_INTERFACE,
        )
        .await;
        close_portal_object(
            &connection,
            portal.destination().as_str(),
            &session_path,
            SESSION_INTERFACE,
        )
        .await;
        return Err(PortalError::Malformed(
            "CreateSession returned a session other than the exact requested handle",
        ));
    }
    if !target_is_active(target) {
        close_portal_object(
            &connection,
            portal.destination().as_str(),
            &session_path,
            SESSION_INTERFACE,
        )
        .await;
        return Err(PortalError::Stopped);
    }

    let result = run_session(
        &connection,
        &portal,
        &session_path,
        &mut owner_changes,
        target,
        registration,
        trigger,
    )
    .await;
    // Cleanup is pinned to the proven unique owner and bounded. It can never
    // cross to a replacement portal process.
    close_portal_object(
        &connection,
        portal.destination().as_str(),
        &session_path,
        SESSION_INTERFACE,
    )
    .await;
    result
}

async fn register_host_connection(connection: &zbus::Connection) -> Result<(), PortalError> {
    let registry = portal_proxy(connection, PORTAL_DESTINATION, HOST_REGISTRY_INTERFACE).await?;
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    let result = complete_before(
        PORTAL_CALL_DEADLINE,
        registry.call::<_, _, ()>("Register", &(PORTAL_DESKTOP_ID, options)),
    )
    .await
    .ok_or(PortalError::TimedOut("register the host portal connection"))?;
    // Zephium is a host application and therefore owns one exact Registry
    // identity. Falling back to automatic cgroup inference after rejection can
    // attribute a raw development launch to its terminal (or another scope),
    // persisting consent under the wrong application. Keep the focused GTK
    // shortcut active unless the portal accepted this exact identity.
    result.map_err(classify_host_registry_error)
}

fn classify_host_registry_error(error: zbus::Error) -> PortalError {
    let rendered = error.to_string();
    match error {
        // Registry has processed the exact identity and returned a portal
        // policy/application error. Repeating the same call against the same
        // owner cannot establish missing desktop metadata or reverse a policy
        // decision, and produces misleading reconnect noise in raw dev runs.
        // Bus transport/owner failures deliberately remain retryable below.
        zbus::Error::MethodError(name, _, _)
            if terminal_host_registry_error_name(name.as_str()) =>
        {
            PortalError::HostIdentityRejected(format!(
                "register exact host portal identity: {rendered}"
            ))
        }
        _ => PortalError::Transport(format!("register exact host portal identity: {rendered}")),
    }
}

fn terminal_host_registry_error_name(name: &str) -> bool {
    name.starts_with("org.freedesktop.portal.Error.")
}

async fn run_session(
    connection: &zbus::Connection,
    portal: &zbus::Proxy<'static>,
    session_path: &OwnedObjectPath,
    owner_changes: &mut zbus::proxy::OwnerChangedStream<'static>,
    target: &Weak<ActivationTarget>,
    registration: &GlobalRegistration,
    trigger: &str,
) -> Result<(), PortalError> {
    let session = object_proxy(
        connection,
        portal.destination().as_str(),
        session_path.as_str(),
        SESSION_INTERFACE,
    )
    .await?;
    let mut closed = session
        .receive_signal("Closed")
        .await
        .map_err(|error| PortalError::transport("subscribe to shortcut session closure", error))?;
    let mut activations = portal
        .receive_signal_with_args("Activated", &[(0, session_path.as_str()), (1, SHORTCUT_ID)])
        .await
        .map_err(|error| PortalError::transport("subscribe to shortcut activation", error))?;
    // Subscribe before ListShortcuts so a compositor-side reassignment cannot
    // be lost between the authoritative response and event-loop admission.
    let mut shortcut_changes = portal
        .receive_signal_with_args("ShortcutsChanged", &[(0, session_path.as_str())])
        .await
        .map_err(|error| PortalError::transport("subscribe to shortcut changes", error))?;

    let list_token = next_portal_token("list")?;
    let list_options = HashMap::from([("handle_token", Value::new(list_token.as_str()))]);
    let list_response = portal_request(
        connection,
        portal,
        "ListShortcuts",
        &list_token,
        &(session_path, &list_options),
        owner_changes,
        target,
    )
    .await?;
    let already_bound = decode_shortcuts(&list_response.results, ShortcutResultKind::PriorSession)?;
    let mut authoritative_position = list_response.position;

    if !already_bound {
        let bind_token = next_portal_token("bind")?;
        let mut shortcut_properties =
            HashMap::from([("description", Value::new(SHORTCUT_DESCRIPTION))]);
        shortcut_properties.insert("preferred_trigger", Value::new(trigger));
        let shortcuts = vec![(SHORTCUT_ID, shortcut_properties)];
        let bind_options = HashMap::from([("handle_token", Value::new(bind_token.as_str()))]);
        let bind_response = portal_request(
            connection,
            portal,
            "BindShortcuts",
            &bind_token,
            &(session_path, &shortcuts, "", &bind_options),
            owner_changes,
            target,
        )
        .await?;
        if !decode_shortcuts(&bind_response.results, ShortcutResultKind::ExactBindSubset)? {
            return Err(PortalError::NoBinding);
        }
        authoritative_position = bind_response.position;
    }

    // Suppress the focused-window fallback only after the exact session and
    // fixed shortcut have been proven. Dropping this lease re-enables it on
    // every closure, transport failure, portal restart, or shutdown path.
    let _registration = ClearRegistrationOnDrop::acquire(registration);

    loop {
        match next_session_event(
            &mut shortcut_changes,
            &mut activations,
            &mut closed,
            owner_changes,
            target,
        )
        .await
        {
            SessionEvent::ShortcutsChanged(Some(message)) => {
                if message.recv_position() <= authoritative_position {
                    continue;
                }
                authoritative_position = message.recv_position();
                reconcile_shortcuts_changed(registration, session_path, &message)?;
            }
            SessionEvent::ShortcutsChanged(None) => {
                return Err(PortalError::Transport(
                    "shortcut-change stream ended".to_owned(),
                ));
            }
            SessionEvent::Activated(Some(message)) => {
                if message.body().len() > MAX_ACTIVATION_SIGNAL_BODY_BYTES {
                    log_malformed_signal("activation body exceeded its byte budget");
                    continue;
                }
                let body =
                    message
                        .body()
                        .deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>(
                        );
                match body {
                    Ok((activated_session, command, timestamp, options))
                        if activation_matches(session_path, &activated_session, &command) =>
                    {
                        if !registration.is_live() {
                            continue;
                        }
                        if let Some(target) = target.upgrade() {
                            target.activate(decode_activation_context(timestamp, &options));
                        } else {
                            return Ok(());
                        }
                    }
                    Ok(_) => {}
                    Err(error) => log_malformed_signal(&error),
                }
            }
            SessionEvent::Activated(None) => {
                return Err(PortalError::Transport(
                    "shortcut activation stream ended".to_owned(),
                ));
            }
            SessionEvent::Closed(Some(_)) => return Err(PortalError::SessionClosed),
            SessionEvent::Closed(None) => {
                return Err(PortalError::Transport(
                    "shortcut session stream ended".to_owned(),
                ));
            }
            SessionEvent::OwnerChanged => {
                return Err(PortalError::Transport(
                    "portal service owner changed".to_owned(),
                ));
            }
            SessionEvent::Stopped => return Err(PortalError::Stopped),
        }
    }
}

struct PortalResponse {
    results: HashMap<String, OwnedValue>,
    position: zbus::message::Sequence,
}

async fn portal_request<B>(
    connection: &zbus::Connection,
    portal: &zbus::Proxy<'static>,
    method: &'static str,
    handle_token: &str,
    body: &B,
    owner_changes: &mut zbus::proxy::OwnerChangedStream<'static>,
    target: &Weak<ActivationTarget>,
) -> Result<PortalResponse, PortalError>
where
    B: Serialize + DynamicType,
{
    let request_path = predicted_portal_path(connection, REQUEST_PATH_PREFIX, handle_token)?;
    let request = object_proxy(
        connection,
        portal.destination().as_str(),
        request_path.as_str(),
        REQUEST_INTERFACE,
    )
    .await?;
    // Subscribe before calling the method. Fast portal implementations may
    // emit Response before the method reply reaches the client.
    let mut responses = request
        .receive_signal("Response")
        .await
        .map_err(|error| PortalError::transport("subscribe to the portal response", error))?;
    let returned_path: OwnedObjectPath =
        match portal_call_with_deadline(method, portal.call(method, body)).await {
            Ok(path) => path,
            Err(error) => {
                close_portal_object(
                    connection,
                    portal.destination().as_str(),
                    &request_path,
                    REQUEST_INTERFACE,
                )
                .await;
                return Err(error);
            }
        };
    if returned_path != request_path {
        close_portal_object(
            connection,
            portal.destination().as_str(),
            &returned_path,
            REQUEST_INTERFACE,
        )
        .await;
        close_portal_object(
            connection,
            portal.destination().as_str(),
            &request_path,
            REQUEST_INTERFACE,
        )
        .await;
        return Err(PortalError::Malformed(
            "portal returned a request other than the exact requested handle",
        ));
    }
    let message = match next_request_event(&mut responses, owner_changes, target).await {
        RequestEvent::Response(Some(message)) => message,
        RequestEvent::Response(None) => {
            close_portal_object(
                connection,
                portal.destination().as_str(),
                &request_path,
                REQUEST_INTERFACE,
            )
            .await;
            return Err(PortalError::Transport(
                "portal response stream ended".to_owned(),
            ));
        }
        RequestEvent::OwnerChanged => {
            close_portal_object(
                connection,
                portal.destination().as_str(),
                &request_path,
                REQUEST_INTERFACE,
            )
            .await;
            return Err(PortalError::Transport(
                "portal service owner changed".to_owned(),
            ));
        }
        RequestEvent::Stopped => {
            close_portal_object(
                connection,
                portal.destination().as_str(),
                &request_path,
                REQUEST_INTERFACE,
            )
            .await;
            return Err(PortalError::Stopped);
        }
    };
    if message.body().len() > MAX_PORTAL_RESPONSE_BODY_BYTES {
        return Err(PortalError::Malformed("portal Response body is too large"));
    }
    let position = message.recv_position();
    let (response, results): (u32, HashMap<String, OwnedValue>) = message
        .body()
        .deserialize()
        .map_err(|_| PortalError::Malformed("portal Response has an invalid body"))?;
    match response {
        0 => Ok(PortalResponse { results, position }),
        1 => Err(PortalError::Denied),
        code => Err(PortalError::Rejected(code)),
    }
}

enum SessionEvent {
    ShortcutsChanged(Option<zbus::Message>),
    Activated(Option<zbus::Message>),
    Closed(Option<zbus::Message>),
    OwnerChanged,
    Stopped,
}

enum RequestEvent {
    Response(Option<zbus::Message>),
    OwnerChanged,
    Stopped,
}

async fn next_session_event(
    shortcut_changes: &mut zbus::proxy::SignalStream<'_>,
    activations: &mut zbus::proxy::SignalStream<'_>,
    closed: &mut zbus::proxy::SignalStream<'_>,
    owner_changes: &mut zbus::proxy::OwnerChangedStream<'static>,
    target: &Weak<ActivationTarget>,
) -> SessionEvent {
    future::or(
        async { SessionEvent::ShortcutsChanged(shortcut_changes.next().await) },
        future::or(
            async { SessionEvent::Activated(activations.next().await) },
            future::or(
                async { SessionEvent::Closed(closed.next().await) },
                future::or(
                    async {
                        let _ = owner_changes.next().await;
                        SessionEvent::OwnerChanged
                    },
                    async {
                        wait_until_stopped(target).await;
                        SessionEvent::Stopped
                    },
                ),
            ),
        ),
    )
    .await
}

async fn next_request_event(
    responses: &mut zbus::proxy::SignalStream<'_>,
    owner_changes: &mut zbus::proxy::OwnerChangedStream<'static>,
    target: &Weak<ActivationTarget>,
) -> RequestEvent {
    future::or(
        async { RequestEvent::Response(responses.next().await) },
        future::or(
            async {
                let _ = owner_changes.next().await;
                RequestEvent::OwnerChanged
            },
            async {
                wait_until_stopped(target).await;
                RequestEvent::Stopped
            },
        ),
    )
    .await
}

async fn wait_until_stopped(target: &Weak<ActivationTarget>) {
    while target_is_active(target) {
        async_io::Timer::after(CANCELLATION_POLL).await;
    }
}

async fn delay_until_retry(target: &Weak<ActivationTarget>, delay: Duration) -> bool {
    future::or(
        async {
            async_io::Timer::after(delay).await;
            true
        },
        async {
            wait_until_stopped(target).await;
            false
        },
    )
    .await
}

fn target_is_active(target: &Weak<ActivationTarget>) -> bool {
    target.upgrade().is_some_and(|target| target.is_active())
}

async fn close_portal_object(
    connection: &zbus::Connection,
    destination: &str,
    path: &OwnedObjectPath,
    interface: &'static str,
) {
    let proxy = object_proxy(connection, destination, path.as_str(), interface).await;
    if let Ok(proxy) = proxy {
        let _ = complete_before(PORTAL_CLOSE_DEADLINE, proxy.call::<_, _, ()>("Close", &())).await;
    }
}

async fn portal_proxy(
    connection: &zbus::Connection,
    destination: &str,
    interface: &'static str,
) -> Result<zbus::Proxy<'static>, PortalError> {
    object_proxy(connection, destination, PORTAL_PATH, interface).await
}

async fn object_proxy(
    connection: &zbus::Connection,
    destination: &str,
    path: &str,
    interface: &'static str,
) -> Result<zbus::Proxy<'static>, PortalError> {
    let builder = zbus::proxy::Builder::<zbus::Proxy<'static>>::new(connection)
        .destination(destination.to_owned())
        .map_err(|error| PortalError::transport("set the portal proxy destination", error))?
        .path(path.to_owned())
        .map_err(|error| PortalError::transport("set the portal proxy path", error))?
        .interface(interface.to_owned())
        .map_err(|error| PortalError::transport("set the portal proxy interface", error))?
        .cache_properties(CacheProperties::No);
    // CacheProperties::No makes build purely local; no portal call can happen
    // before the owner watch and Registry checkpoint are established.
    builder
        .build()
        .await
        .map_err(|error| PortalError::transport("build the portal proxy", error))
}

async fn complete_before<T>(duration: Duration, operation: impl Future<Output = T>) -> Option<T> {
    future::or(async { Some(operation.await) }, async {
        async_io::Timer::after(duration).await;
        None
    })
    .await
}

async fn portal_call_with_deadline<T>(
    context: &'static str,
    operation: impl Future<Output = Result<T, zbus::Error>>,
) -> Result<T, PortalError> {
    complete_before(PORTAL_CALL_DEADLINE, operation)
        .await
        .ok_or(PortalError::TimedOut(context))?
        .map_err(|error| PortalError::transport(context, error))
}

async fn current_portal_owner(
    connection: &zbus::Connection,
) -> Result<Option<OwnedUniqueName>, PortalError> {
    let dbus = zbus::fdo::DBusProxy::new(connection)
        .await
        .map_err(|error| PortalError::transport("create the D-Bus owner proxy", error))?;
    let name = PORTAL_DESTINATION
        .try_into()
        .map_err(|error| PortalError::transport("validate the portal bus name", error))?;
    let result = complete_before(PORTAL_CALL_DEADLINE, dbus.get_name_owner(name))
        .await
        .ok_or(PortalError::TimedOut("resolve the portal service owner"))?;
    match result {
        Ok(owner) => Ok(Some(owner)),
        Err(zbus::fdo::Error::NameHasNoOwner(_)) => Ok(None),
        Err(error) => Err(PortalError::transport(
            "resolve the portal service owner",
            error,
        )),
    }
}

async fn stable_portal_owner(
    connection: &zbus::Connection,
    owner_changes: &mut zbus::proxy::OwnerChangedStream<'static>,
) -> Result<Option<OwnedUniqueName>, PortalError> {
    let first = current_portal_owner(connection).await?;
    if !drain_owner_changes(owner_changes).await?.is_empty() {
        return Err(PortalError::Transport(
            "portal service owner changed while establishing the owner checkpoint".to_owned(),
        ));
    }
    let second = current_portal_owner(connection).await?;
    if first != second {
        return Err(PortalError::Transport(
            "portal service owner changed while establishing the owner checkpoint".to_owned(),
        ));
    }
    Ok(second)
}

async fn ensure_portal_owner(
    connection: &zbus::Connection,
    owner_changes: &mut zbus::proxy::OwnerChangedStream<'static>,
    expected: &OwnedUniqueName,
) -> Result<(), PortalError> {
    if !drain_owner_changes(owner_changes).await?.is_empty()
        || current_portal_owner(connection).await?.as_ref() != Some(expected)
    {
        return Err(PortalError::Transport(
            "portal service owner changed before shortcut session creation".to_owned(),
        ));
    }
    Ok(())
}

async fn drain_owner_changes(
    owner_changes: &mut zbus::proxy::OwnerChangedStream<'static>,
) -> Result<Vec<Option<String>>, PortalError> {
    const MAX_OWNER_TRANSITIONS: usize = 8;
    let mut transitions = Vec::with_capacity(MAX_OWNER_TRANSITIONS);
    for _ in 0..=MAX_OWNER_TRANSITIONS {
        match future::poll_once(owner_changes.next()).await {
            None => return Ok(transitions),
            Some(Some(transition)) if transitions.len() < MAX_OWNER_TRANSITIONS => {
                transitions.push(transition.map(|owner| owner.as_str().to_owned()));
            }
            Some(Some(_)) => {
                return Err(PortalError::Transport(
                    "portal owner transition queue exceeded its bound".to_owned(),
                ));
            }
            Some(None) => {
                return Err(PortalError::Transport(
                    "portal owner-change stream ended".to_owned(),
                ));
            }
        }
    }
    Ok(transitions)
}

fn registration_owner_is_current(
    before: Option<&OwnedUniqueName>,
    after: Option<&OwnedUniqueName>,
    transitions: &[Option<String>],
) -> bool {
    before.is_some() && before == after && transitions.is_empty()
}

fn predicted_portal_path(
    connection: &zbus::Connection,
    prefix: &str,
    handle_token: &str,
) -> Result<OwnedObjectPath, PortalError> {
    let unique_name = connection
        .unique_name()
        .ok_or(PortalError::Malformed("session bus has no unique name"))?;
    portal_path(prefix, unique_name.as_str(), handle_token)
}

fn portal_path(
    prefix: &str,
    unique_name: &str,
    handle_token: &str,
) -> Result<OwnedObjectPath, PortalError> {
    let unique = unique_name.trim_start_matches(':').replace('.', "_");
    OwnedObjectPath::try_from(format!("{prefix}/{unique}/{handle_token}"))
        .map_err(|_| PortalError::Malformed("generated an invalid portal object path"))
}

fn next_portal_token(purpose: &'static str) -> Result<String, PortalError> {
    next_portal_token_with_entropy(purpose, |bytes| getrandom::fill(bytes).map_err(|_| ()))
}

fn next_portal_token_with_entropy(
    purpose: &'static str,
    fill: impl FnOnce(&mut [u8]) -> Result<(), ()>,
) -> Result<String, PortalError> {
    if purpose.is_empty()
        || !purpose
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    {
        return Err(PortalError::Malformed("portal token purpose is invalid"));
    }
    let mut nonce = [0_u8; PORTAL_TOKEN_NONCE_BYTES];
    fill(&mut nonce).map_err(|()| PortalError::EntropyUnavailable)?;
    let sequence = NEXT_PORTAL_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |sequence| {
            sequence.checked_add(1)
        })
        .map_err(|_| PortalError::Malformed("portal token sequence exhausted"))?;
    Ok(portal_token_from_nonce(purpose, &nonce, sequence))
}

fn portal_token_from_nonce(
    purpose: &str,
    nonce: &[u8; PORTAL_TOKEN_NONCE_BYTES],
    sequence: u64,
) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut nonce_hex = String::with_capacity(PORTAL_TOKEN_NONCE_BYTES * 2);
    for byte in nonce {
        nonce_hex.push(char::from(HEX[usize::from(byte >> 4)]));
        nonce_hex.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    format!("zephium_{purpose}_{nonce_hex}_{sequence}")
}

fn decode_session_handle(
    results: &HashMap<String, OwnedValue>,
) -> Result<OwnedObjectPath, PortalError> {
    let value = results.get("session_handle").ok_or(PortalError::Malformed(
        "CreateSession omitted session_handle",
    ))?;
    if let Ok(path) = <&ObjectPath<'_>>::try_from(value) {
        return Ok(OwnedObjectPath::from(path.to_owned()));
    }
    if let Ok(path) = <&str>::try_from(value) {
        return OwnedObjectPath::try_from(path)
            .map_err(|_| PortalError::Malformed("CreateSession returned an invalid session path"));
    }
    Err(PortalError::Malformed(
        "CreateSession returned session_handle with the wrong type",
    ))
}

#[derive(Clone, Copy)]
enum ShortcutResultKind {
    PriorSession,
    ExactBindSubset,
}

fn decode_shortcuts(
    results: &HashMap<String, OwnedValue>,
    kind: ShortcutResultKind,
) -> Result<bool, PortalError> {
    let value = results.get("shortcuts").ok_or(PortalError::Malformed(
        "shortcut response omitted shortcuts",
    ))?;
    let value = value
        .try_clone()
        .map_err(|_| PortalError::Malformed("shortcut response value could not be copied"))?;
    let shortcuts = Vec::<(String, HashMap<String, OwnedValue>)>::try_from(value)
        .map_err(|_| PortalError::Malformed("shortcut response has the wrong type"))?;
    validate_shortcuts(shortcuts, kind)
}

fn reconcile_shortcuts_changed(
    registration: &GlobalRegistration,
    expected_session: &OwnedObjectPath,
    message: &zbus::Message,
) -> Result<(), PortalError> {
    // Revoke global capability before inspecting untrusted portal data. A
    // malformed or removal update immediately restores the focused fallback.
    registration.set_live(false);
    if !shortcut_signal_size_allowed(message.body().len()) {
        return Err(PortalError::Malformed("ShortcutsChanged body is too large"));
    }
    let (session, shortcuts): (OwnedObjectPath, ShortcutList) = message
        .body()
        .deserialize()
        .map_err(|_| PortalError::Malformed("ShortcutsChanged has an invalid body"))?;
    reconcile_decoded_shortcuts(registration, expected_session, &session, shortcuts)
}

fn shortcut_signal_size_allowed(body_bytes: usize) -> bool {
    body_bytes <= MAX_SHORTCUT_SIGNAL_BODY_BYTES
}

fn reconcile_decoded_shortcuts(
    registration: &GlobalRegistration,
    expected_session: &OwnedObjectPath,
    changed_session: &OwnedObjectPath,
    shortcuts: ShortcutList,
) -> Result<(), PortalError> {
    registration.set_live(false);
    if changed_session != expected_session {
        return Err(PortalError::Malformed(
            "ShortcutsChanged named a different session",
        ));
    }
    let launcher_is_bound = validate_shortcuts(shortcuts, ShortcutResultKind::PriorSession)?;
    registration.set_live(launcher_is_bound);
    Ok(())
}

fn validate_shortcuts(
    shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
    kind: ShortcutResultKind,
) -> Result<bool, PortalError> {
    // Both Request responses and ShortcutsChanged signals converge here. The
    // byte cap bounds wire allocation, while this independent cardinality cap
    // bounds per-entry maps and duplicate tracking after deserialization.
    if shortcuts.len() > MAX_SHORTCUT_RESULTS {
        return Err(PortalError::Malformed("shortcut response is too large"));
    }
    let mut ids = HashSet::with_capacity(shortcuts.len());
    for (id, properties) in shortcuts {
        if id.is_empty()
            || id.len() > MAX_SHORTCUT_ID_BYTES
            || properties.len() > MAX_SHORTCUT_PROPERTIES
            || !ids.insert(id)
        {
            return Err(PortalError::Malformed("shortcut response is invalid"));
        }
    }
    match kind {
        ShortcutResultKind::PriorSession => Ok(ids.contains(SHORTCUT_ID)),
        ShortcutResultKind::ExactBindSubset => {
            if ids.iter().any(|id| id != SHORTCUT_ID) {
                return Err(PortalError::Malformed(
                    "BindShortcuts returned an item outside the requested subset",
                ));
            }
            Ok(ids.len() == 1 && ids.contains(SHORTCUT_ID))
        }
    }
}

fn activation_matches(
    expected_session: &OwnedObjectPath,
    activated_session: &OwnedObjectPath,
    command: &str,
) -> bool {
    expected_session == activated_session && command == SHORTCUT_ID
}

fn decode_activation_context(
    timestamp: u64,
    options: &HashMap<String, OwnedValue>,
) -> ActivationContext {
    let activation_token = if options.len() <= MAX_ACTIVATION_OPTIONS {
        options
            .get("activation_token")
            .and_then(|value| <&str>::try_from(value).ok())
            .filter(|token| valid_activation_token(token))
            .map(ToOwned::to_owned)
    } else {
        None
    };
    ActivationContext {
        activation_token,
        timestamp: u32::try_from(timestamp).ok(),
    }
}

fn valid_activation_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_ACTIVATION_TOKEN_BYTES
        && !token.chars().any(char::is_control)
}

fn log_malformed_signal(error: impl fmt::Display) {
    if MALFORMED_SIGNAL_LOGS_REMAINING
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
    {
        crate::write_diagnostic(format_args!(
            "global shortcut: ignored malformed portal activation: {error}"
        ));
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Debug)]
pub(crate) enum PortalError {
    Transport(String),
    HostIdentityRejected(String),
    TimedOut(&'static str),
    UnsupportedVersion(u32),
    EntropyUnavailable,
    Denied,
    Rejected(u32),
    NoBinding,
    SessionClosed,
    Malformed(&'static str),
    Stopped,
}

impl PortalError {
    fn transport(context: &'static str, error: impl fmt::Display) -> Self {
        Self::Transport(format!("{context}: {error}"))
    }

    fn retryable(&self) -> bool {
        matches!(self, Self::Transport(_) | Self::TimedOut(_))
    }

    pub(crate) fn is_stopped(&self) -> bool {
        matches!(self, Self::Stopped)
    }
}

impl fmt::Display for PortalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => formatter.write_str(error),
            Self::HostIdentityRejected(error) => formatter.write_str(error),
            Self::TimedOut(operation) => write!(formatter, "{operation} timed out"),
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "global-shortcuts portal version {version} is unsupported"
                )
            }
            Self::EntropyUnavailable => {
                formatter.write_str("secure portal token entropy is unavailable")
            }
            Self::Denied => formatter.write_str("the shortcut request was denied or cancelled"),
            Self::Rejected(code) => write!(formatter, "the shortcut request failed ({code})"),
            Self::NoBinding => formatter.write_str("the portal did not bind the launcher shortcut"),
            Self::SessionClosed => formatter.write_str("the portal closed the shortcut session"),
            Self::Malformed(message) => formatter.write_str(message),
            Self::Stopped => formatter.write_str("shortcut service stopped"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_paths_are_exact_and_dbus_safe() {
        let path = portal_path(REQUEST_PATH_PREFIX, ":1.204", "zephium_bind_7_3")
            .expect("valid predicted request path");
        assert_eq!(
            path.as_str(),
            "/org/freedesktop/portal/desktop/request/1_204/zephium_bind_7_3"
        );
        assert!(portal_path(REQUEST_PATH_PREFIX, ":1.2", "bad.token").is_err());
    }

    #[test]
    fn activation_requires_the_exact_session_and_fixed_command() {
        let expected = OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_2/a")
            .expect("test session path");
        let other = OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_2/b")
            .expect("other test session path");
        assert!(activation_matches(&expected, &expected, SHORTCUT_ID));
        assert!(!activation_matches(&expected, &other, SHORTCUT_ID));
        assert!(!activation_matches(&expected, &expected, "tab.new"));
        assert!(!activation_matches(
            &expected,
            &expected,
            "launcher.toggle\0tab.new"
        ));
    }

    #[test]
    fn activation_metadata_is_bounded_and_never_truncates_time() {
        let mut options = HashMap::new();
        options.insert(
            "activation_token".to_owned(),
            OwnedValue::from(zbus::zvariant::Str::from("opaque-token")),
        );
        assert_eq!(
            decode_activation_context(42, &options),
            ActivationContext {
                activation_token: Some("opaque-token".to_owned()),
                timestamp: Some(42),
            }
        );
        assert_eq!(
            decode_activation_context(u64::from(u32::MAX) + 1, &options).timestamp,
            None
        );
        options.insert(
            "activation_token".to_owned(),
            OwnedValue::from(zbus::zvariant::Str::from("bad\0token")),
        );
        assert!(decode_activation_context(42, &options)
            .activation_token
            .is_none());
    }

    #[test]
    fn bind_result_must_be_the_exact_requested_subset() {
        let entry = |id: &str| (id.to_owned(), HashMap::new());
        assert!(validate_shortcuts(
            vec![entry(SHORTCUT_ID)],
            ShortcutResultKind::ExactBindSubset
        )
        .expect("exact fixed shortcut"));
        assert!(
            !validate_shortcuts(Vec::new(), ShortcutResultKind::ExactBindSubset)
                .expect("empty subset is a valid denial")
        );
        assert!(validate_shortcuts(
            vec![entry("legacy.command"), entry(SHORTCUT_ID)],
            ShortcutResultKind::PriorSession
        )
        .expect("prior-session list"));
        assert!(validate_shortcuts(
            vec![entry("other.command")],
            ShortcutResultKind::ExactBindSubset
        )
        .is_err());
        assert!(validate_shortcuts(
            vec![entry(SHORTCUT_ID), entry(SHORTCUT_ID)],
            ShortcutResultKind::ExactBindSubset
        )
        .is_err());
        let oversized = (0..=MAX_SHORTCUT_RESULTS)
            .map(|index| entry(&format!("shortcut.{index}")))
            .collect();
        assert!(validate_shortcuts(oversized, ShortcutResultKind::PriorSession).is_err());
    }

    #[test]
    fn denial_and_malformed_data_are_never_retried() {
        assert!(!PortalError::Denied.retryable());
        assert!(!PortalError::Rejected(2).retryable());
        assert!(
            !PortalError::HostIdentityRejected("missing desktop metadata".to_owned()).retryable()
        );
        assert!(!PortalError::EntropyUnavailable.retryable());
        assert!(!PortalError::Malformed("bad data").retryable());
        assert!(!PortalError::SessionClosed.retryable());
        assert!(PortalError::Transport("restart".to_owned()).retryable());
        assert!(PortalError::TimedOut("test operation").retryable());
        assert_eq!(MAX_PORTAL_ATTEMPTS, 3);
    }

    #[test]
    fn host_registry_rejection_is_terminal_but_owner_failures_remain_retryable() {
        assert!(terminal_host_registry_error_name(
            "org.freedesktop.portal.Error.Failed"
        ));
        assert!(terminal_host_registry_error_name(
            "org.freedesktop.portal.Error.NotAllowed"
        ));
        assert!(!terminal_host_registry_error_name(
            "org.freedesktop.DBus.Error.NameHasNoOwner"
        ));
        assert!(!terminal_host_registry_error_name(
            "org.freedesktop.DBus.Error.NoReply"
        ));
    }

    #[test]
    fn portal_tokens_have_bounded_path_safe_nonce_encoding_and_nonwrapping_sequence() {
        let token = portal_token_from_nonce("bind", &[0xab; PORTAL_TOKEN_NONCE_BYTES], 42);
        assert_eq!(token, "zephium_bind_abababababababababababababababab_42");
        assert!(token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
        assert_ne!(
            token,
            portal_token_from_nonce("bind", &[0xac; PORTAL_TOKEN_NONCE_BYTES], 42)
        );
        assert_ne!(
            token,
            portal_token_from_nonce("bind", &[0xab; PORTAL_TOKEN_NONCE_BYTES], 43)
        );
        assert!(matches!(
            next_portal_token_with_entropy("bind", |_| Err(())),
            Err(PortalError::EntropyUnavailable)
        ));
        assert!(next_portal_token_with_entropy("bad-purpose", |_| Ok(())).is_err());
    }

    #[test]
    fn portal_owner_registration_rejects_every_observed_transition() {
        let first = OwnedUniqueName::try_from(":1.40").expect("first owner");
        let second = OwnedUniqueName::try_from(":1.41").expect("second owner");
        assert!(registration_owner_is_current(
            Some(&first),
            Some(&first),
            &[]
        ));
        assert!(!registration_owner_is_current(
            None,
            Some(&first),
            &[Some(first.as_str().to_owned())]
        ));
        assert!(!registration_owner_is_current(
            Some(&first),
            Some(&second),
            &[Some(second.as_str().to_owned())]
        ));
        assert!(!registration_owner_is_current(
            Some(&first),
            Some(&first),
            &[None]
        ));
    }

    #[test]
    fn noninteractive_deadline_is_deterministic() {
        assert_eq!(
            future::block_on(complete_before(Duration::from_secs(1), future::ready(7))),
            Some(7)
        );
        assert_eq!(
            future::block_on(complete_before(
                Duration::from_millis(1),
                future::pending::<u8>()
            )),
            None
        );
    }

    #[test]
    fn authoritative_shortcut_removal_immediately_restores_focused_fallback() {
        let expected =
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_2/expected")
                .expect("expected session");
        let other = OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_2/other")
            .expect("other session");
        let entry = |id: &str| (id.to_owned(), HashMap::new());
        let registration = GlobalRegistration::default();

        reconcile_decoded_shortcuts(
            &registration,
            &expected,
            &expected,
            vec![entry(SHORTCUT_ID)],
        )
        .expect("authoritative add");
        assert!(registration.is_live());
        reconcile_decoded_shortcuts(&registration, &expected, &expected, Vec::new())
            .expect("authoritative removal");
        assert!(!registration.is_live());

        registration.set_live(true);
        assert!(reconcile_decoded_shortcuts(
            &registration,
            &expected,
            &other,
            vec![entry(SHORTCUT_ID)]
        )
        .is_err());
        assert!(!registration.is_live());
        assert!(shortcut_signal_size_allowed(MAX_SHORTCUT_SIGNAL_BODY_BYTES));
        assert!(!shortcut_signal_size_allowed(
            MAX_SHORTCUT_SIGNAL_BODY_BYTES + 1
        ));
    }

    #[test]
    fn stopping_target_drops_its_runtime_callback() {
        let called = Arc::new(AtomicUsize::new(0));
        let count = called.clone();
        let target = ActivationTarget::new(move |_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        target.activate(ActivationContext::default());
        target.stop();
        target.activate(ActivationContext::default());
        assert_eq!(called.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn stop_waits_for_an_admitted_callback_and_blocks_future_activation() {
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let callback_count = Arc::new(AtomicUsize::new(0));
        let target = Arc::new(ActivationTarget::new({
            let entered = entered.clone();
            let release = release.clone();
            let callback_count = callback_count.clone();
            move |_| {
                callback_count.fetch_add(1, Ordering::Relaxed);
                entered.wait();
                release.wait();
            }
        }));
        let activator = std::thread::spawn({
            let target = target.clone();
            move || target.activate(ActivationContext::default())
        });
        entered.wait();

        let (stopped_tx, stopped_rx) = std::sync::mpsc::sync_channel(1);
        let stopper = std::thread::spawn({
            let target = target.clone();
            move || {
                target.stop();
                let _ = stopped_tx.send(());
            }
        });
        assert!(stopped_rx.recv_timeout(Duration::from_millis(20)).is_err());
        release.wait();
        stopped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("stop completed after callback retired");
        activator.join().expect("activation thread");
        stopper.join().expect("stop thread");
        target.activate(ActivationContext::default());
        assert_eq!(callback_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn registration_lease_is_exact_and_fail_open_for_focused_fallback() {
        let registration = GlobalRegistration::default();
        assert!(!registration.is_live());
        {
            let _lease = ClearRegistrationOnDrop::acquire(&registration);
            assert!(registration.is_live());
        }
        assert!(!registration.is_live());
    }

    #[test]
    fn production_protocol_contains_no_panicking_data_access() {
        let source = include_str!("linux_shortcut_portal.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production module");
        assert!(!source.contains(".unwrap("));
        assert!(!source.contains(".expect("));
        assert!(!source.contains("panic!("));
        assert!(!source.contains("assert!("));
        assert!(!source.contains("eprintln!("));
        assert!(!source.contains("unreachable!("));
        assert!(!source.contains("todo!("));
        let session = source
            .split("async fn run_session(")
            .nth(1)
            .and_then(|source| source.split("async fn portal_request").next())
            .expect("portal session body");
        assert!(session
            .find("ShortcutsChanged")
            .is_some_and(|subscription| {
                session
                    .find("\"ListShortcuts\"")
                    .is_some_and(|list| subscription < list)
            }));
        let reconcile = source
            .split("fn reconcile_shortcuts_changed(")
            .nth(1)
            .and_then(|source| source.split("fn shortcut_signal_size_allowed").next())
            .expect("shortcut reconciliation body");
        assert!(reconcile.find("body().len()").is_some_and(|size| {
            reconcile
                .find(".deserialize()")
                .is_some_and(|decode| size < decode)
        }));
        let run_once = source
            .split("async fn run_once(")
            .nth(1)
            .and_then(|source| source.split("async fn register_host_connection").next())
            .expect("portal connection setup");
        let watch = run_once
            .find("receive_owner_changed")
            .expect("owner subscription");
        let register = run_once
            .find("register_host_connection")
            .expect("Registry call");
        let version = run_once
            .find("global-shortcuts version")
            .expect("version call");
        assert!(watch < register && register < version);
        let registry = source
            .split("async fn register_host_connection(")
            .nth(1)
            .and_then(|source| source.split("async fn run_session").next())
            .expect("bounded host Registry admission");
        assert!(registry.contains("register exact host portal identity"));
        assert!(!registry.contains("automatic app identification"));
        let decoder = source
            .split("fn validate_shortcuts(")
            .nth(1)
            .and_then(|source| source.split("fn activation_matches").next())
            .expect("shared bounded shortcut decoder");
        assert!(decoder.contains("shortcuts.len() > MAX_SHORTCUT_RESULTS"));
        assert!(source.contains("getrandom::fill"));
    }
}
