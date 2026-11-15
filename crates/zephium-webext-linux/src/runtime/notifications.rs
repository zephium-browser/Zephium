//! Bounded session-service notifications, scoped to a profile and installation generation.
use super::*;
use futures_lite::{future, StreamExt};
use serde_json::{json, Value};
use zbus::zvariant::OwnedValue;
use zephium_webext_runtime::api::notifications::{self as api, Request};
use zephium_webext_runtime::context::Sender;
use zephium_webext_runtime::dispatch::Responder;
use zephium_webext_runtime::ApiError;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const MAX_PENDING: usize = 128;
const MAX_PROFILE: usize = 256;
const MAX_INSTALL: usize = 32;
const MAX_OPTIONS: usize = 32 * 1024;

struct Job {
    sender: Box<Sender>,
    root: PathBuf,
    request: Request,
    responder: Responder,
}
#[derive(Clone)]
struct Service {
    owner: String,
    proxy: zbus::Proxy<'static>,
}
struct Record {
    native: u32,
    generation: u64,
    options: Value,
    buttons: usize,
}
#[derive(Default)]
pub(super) struct Backend {
    installs: RefCell<HashMap<ExtensionInstallId, u64>>,
    records: RefCell<HashMap<(ExtensionInstallId, String), Record>>,
    service: RefCell<Option<Service>>,
    tasks: RefCell<Vec<glib::JoinHandle<()>>>,
    pending: RefCell<VecDeque<Job>>,
    running: Cell<bool>,
    next: Cell<u64>,
    retired: Cell<bool>,
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.retire();
    }
}

fn unavailable() -> ApiError {
    ApiError::other("The notification service is unavailable.")
}
async fn bounded<T>(
    future: impl std::future::Future<Output = zbus::Result<T>>,
) -> Result<T, ApiError> {
    future::race(async { future.await.map_err(|_| unavailable()) }, async {
        async_io::Timer::after(Duration::from_secs(3)).await;
        Err(unavailable())
    })
    .await
}
impl Backend {
    pub(super) fn install(&self, install: ExtensionInstallId, generation: u64) {
        self.installs.borrow_mut().insert(install, generation);
    }
    fn current(&self, install: ExtensionInstallId, generation: u64) -> bool {
        !self.retired.get() && self.installs.borrow().get(&install) == Some(&generation)
    }
    pub(super) fn forget(&self, install: ExtensionInstallId) {
        self.installs.borrow_mut().remove(&install);
        let mut records = self.records.borrow_mut();
        let native: Vec<u32> = records
            .iter()
            .filter(|((owner, _), _)| *owner == install)
            .map(|(_, row)| row.native)
            .collect();
        records.retain(|(owner, _), _| *owner != install);
        drop(records);
        if let Some(service) = self.service.borrow().clone() {
            glib::MainContext::default().spawn_local(async move {
                for id in native {
                    let _ =
                        bounded(service.proxy.call::<_, _, ()>("CloseNotification", &(id,))).await;
                }
            });
        }
    }
    pub(super) fn retire(&self) {
        self.retired.set(true);
        let installs: Vec<_> = self.installs.borrow().keys().copied().collect();
        for install in installs {
            self.forget(install);
        }
        for task in self.tasks.borrow_mut().drain(..) {
            task.abort();
        }
        self.service.borrow_mut().take();
    }
    fn same_owner(&self, owner: &str) -> bool {
        self.service
            .borrow()
            .as_ref()
            .is_some_and(|service| service.owner == owner)
    }

    async fn service(self: &Rc<Self>, inner: &Weak<Inner>) -> Result<Service, ApiError> {
        if let Some(service) = self.service.borrow().clone() {
            return Ok(service);
        }
        for task in self.tasks.borrow_mut().drain(..) {
            task.abort();
        }
        let connection = bounded(zbus::Connection::session()).await?;
        let dbus = bounded(zbus::Proxy::new_owned(
            connection.clone(),
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        ))
        .await?;
        let mut owners = bounded(dbus.receive_signal("NameOwnerChanged")).await?;
        let owner: String = bounded(dbus.call("GetNameOwner", &(NAME,))).await?;
        let proxy = bounded(zbus::Proxy::new_owned(
            connection,
            owner.clone(),
            PATH,
            NAME,
        ))
        .await?;
        let actions = bounded(proxy.receive_signal("ActionInvoked")).await?;
        let closed = bounded(proxy.receive_signal("NotificationClosed")).await?;
        let service = Service {
            owner: owner.clone(),
            proxy,
        };
        *self.service.borrow_mut() = Some(service.clone());
        let weak = Rc::downgrade(self);
        let task = glib::MainContext::default().spawn_local(async move {
            while let Some(message) = owners.next().await {
                if message.body().len() > 4096 {
                    continue;
                }
                let Ok((name, _, new)): Result<(String, String, String), _> =
                    message.body().deserialize()
                else {
                    continue;
                };
                if name != NAME {
                    continue;
                }
                let Some(backend) = weak.upgrade() else {
                    break;
                };
                if new != owner && backend.same_owner(&owner) {
                    backend.records.borrow_mut().clear();
                    backend.service.borrow_mut().take();
                    break;
                }
            }
        });
        self.tasks.borrow_mut().push(task);
        for (is_closed, mut stream) in [(false, actions), (true, closed)] {
            let weak = Rc::downgrade(self);
            let inner = inner.clone();
            let owner = service.owner.clone();
            let task = glib::MainContext::default().spawn_local(async move {
                while let Some(message) = stream.next().await {
                    let Some(backend) = weak.upgrade() else {
                        break;
                    };
                    if !backend.same_owner(&owner) || message.body().len() > 4096 {
                        continue;
                    }
                    if message.header().sender().map(|sender| sender.as_str())
                        != Some(owner.as_str())
                    {
                        continue;
                    }
                    if is_closed {
                        if let Ok((id, reason)) = message.body().deserialize::<(u32, u32)>() {
                            backend.signal(&inner, id, None, Some(reason));
                        }
                    } else if let Ok((id, action)) = message.body().deserialize::<(u32, String)>() {
                        backend.signal(&inner, id, Some(&action), None);
                    }
                }
            });
            self.tasks.borrow_mut().push(task);
        }
        Ok(service)
    }

    fn signal(&self, inner: &Weak<Inner>, native: u32, action: Option<&str>, closed: Option<u32>) {
        let event = {
            let mut records = self.records.borrow_mut();
            let Some(((install, id), row)) = records.iter().find(|(_, row)| row.native == native)
            else {
                return;
            };
            let (install, id, generation, buttons) =
                (*install, id.clone(), row.generation, row.buttons);
            if !self.current(install, generation) {
                return;
            }
            if let Some(reason) = closed {
                records.remove(&(install, id.clone()));
                (install, "notifications.onClosed", json!([id, reason == 2]))
            } else if action == Some("default") {
                (install, "notifications.onClicked", json!([id]))
            } else if let Some(index) = action
                .and_then(|action| action.strip_prefix("button-"))
                .and_then(|index| index.parse::<usize>().ok())
                .filter(|index| *index < buttons)
            {
                (install, "notifications.onButtonClicked", json!([id, index]))
            } else {
                return;
            }
        };
        if let Some(inner) = inner.upgrade().filter(|inner| !inner.retired.get()) {
            let _ = inner
                .bridge
                .with_runtime(|host, out| host.notification_event(event.0, event.1, &event.2, out));
            inner.schedule();
        }
    }

    async fn call(self: &Rc<Self>, inner: &Weak<Inner>, job: &Job) -> Result<Value, ApiError> {
        let install = job.sender.install();
        let generation = job.sender.generation().get();
        if !self.current(install, generation) {
            return Err(ApiError::context_invalidated());
        }
        let service = match self.service(inner).await {
            Ok(service) => service,
            Err(_) => {
                return match &job.request {
                    Request::Create { .. } | Request::GetPermissionLevel => Err(unavailable()),
                    _ => api::unavailable(job.request.clone()),
                }
            }
        };
        if !self.current(install, generation)
            || !inner.upgrade().is_some_and(|inner| {
                inner
                    .bridge
                    .with_runtime(|host, _| host.notification_allowed(install))
                    .unwrap_or(false)
            })
        {
            return Err(ApiError::context_invalidated());
        }
        match &job.request {
            Request::GetPermissionLevel => Ok(json!("granted")),
            Request::GetAll => Ok(Value::Object(
                self.records
                    .borrow()
                    .keys()
                    .filter(|(owner, _)| *owner == install)
                    .map(|(_, id)| (id.clone(), json!(true)))
                    .collect(),
            )),
            Request::Clear { notification_id } => {
                let native = self
                    .records
                    .borrow()
                    .get(&(install, notification_id.clone()))
                    .map(|row| row.native);
                let Some(native) = native else {
                    return Ok(json!(false));
                };
                bounded(
                    service
                        .proxy
                        .call::<_, _, ()>("CloseNotification", &(native,)),
                )
                .await?;
                if !self.same_owner(&service.owner) {
                    return Err(unavailable());
                }
                self.records
                    .borrow_mut()
                    .remove(&(install, notification_id.clone()));
                Ok(json!(true))
            }
            Request::Create {
                notification_id,
                options,
            } => {
                let id = match notification_id {
                    Some(id) => id.clone(),
                    None => {
                        let next = self
                            .next
                            .get()
                            .checked_add(1)
                            .ok_or_else(ApiError::too_many_pending)?;
                        self.next.set(next);
                        format!("notification-{next}")
                    }
                };
                self.show(inner, job, &service, id, options.clone(), false)
                    .await
            }
            Request::Update {
                notification_id,
                options,
            } => {
                let Some(mut merged) = self
                    .records
                    .borrow()
                    .get(&(install, notification_id.clone()))
                    .map(|row| row.options.clone())
                else {
                    return Ok(json!(false));
                };
                let object = merged
                    .as_object_mut()
                    .ok_or_else(ApiError::malformed_arguments)?;
                object.extend(
                    options
                        .as_object()
                        .ok_or_else(ApiError::malformed_arguments)?
                        .clone(),
                );
                self.show(inner, job, &service, notification_id.clone(), merged, true)
                    .await
            }
        }
    }

    async fn show(
        &self,
        inner: &Weak<Inner>,
        job: &Job,
        service: &Service,
        id: String,
        options: Value,
        update: bool,
    ) -> Result<Value, ApiError> {
        api::validate_options(&options, true)?;
        if !matches!(options["type"].as_str(), Some("basic" | "list")) {
            return Err(ApiError::other(
                "This notification template is not supported on Linux.",
            ));
        }
        if options.to_string().len() > MAX_OPTIONS {
            return Err(ApiError::message_too_large());
        }
        let install = job.sender.install();
        let generation = job.sender.generation().get();
        let key = (install, id.clone());
        let replaced = {
            let records = self.records.borrow();
            if !records.contains_key(&key)
                && (records.len() >= MAX_PROFILE
                    || records
                        .keys()
                        .filter(|(owner, _)| *owner == install)
                        .count()
                        >= MAX_INSTALL)
            {
                return Err(ApiError::too_many_pending());
            }
            records.get(&key).map(|row| row.native).unwrap_or(0)
        };
        if options.get("appIconMaskUrl").is_some()
            || options["buttons"]
                .as_array()
                .is_some_and(|buttons| buttons.iter().any(|button| button.get("iconUrl").is_some()))
        {
            return Err(ApiError::other(
                "Auxiliary notification icons are not supported on Linux.",
            ));
        }
        let raw = options["iconUrl"]
            .as_str()
            .ok_or_else(ApiError::malformed_arguments)?;
        let icon = icon(&job.root, &job.sender, raw)?;
        let mut hints: HashMap<&str, OwnedValue> = HashMap::new();
        use zephium_core::extensions::{
            EXTENSION_ACTION_ICON_HEIGHT as HEIGHT, EXTENSION_ACTION_ICON_WIDTH as WIDTH,
        };
        hints.insert(
            "urgency",
            OwnedValue::from(if options["priority"].as_i64().unwrap_or(0) > 0 {
                2u8
            } else {
                1u8
            }),
        );
        if let Some(silent) = options["silent"].as_bool() {
            hints.insert("suppress-sound", OwnedValue::from(silent));
        }
        hints.insert(
            "image-data",
            zbus::zvariant::Value::from((
                WIDTH as i32,
                HEIGHT as i32,
                (WIDTH * 4) as i32,
                true,
                8i32,
                4i32,
                icon.rgba().to_vec(),
            ))
            .try_into()
            .map_err(|_| ApiError::malformed_arguments())?,
        );
        hints.insert(
            "urgency",
            OwnedValue::from(if options["priority"].as_i64().unwrap_or(0) > 0 {
                2u8
            } else {
                1u8
            }),
        );
        if options["silent"] == true {
            hints.insert("suppress-sound", OwnedValue::from(true));
        }
        let escape = |text: &str| glib::markup_escape_text(text).to_string();
        let title = options["title"].as_str().unwrap_or_default();
        let mut body = escape(options["message"].as_str().unwrap_or_default());
        if let Some(items) = options["items"].as_array() {
            if items.len() > 32 {
                return Err(ApiError::too_many_pending());
            }
            for item in items {
                body.push('\n');
                body.push_str(&escape(item["title"].as_str().unwrap_or_default()));
                body.push_str(": ");
                body.push_str(&escape(item["message"].as_str().unwrap_or_default()));
            }
        }
        if let Some(context) = options["contextMessage"]
            .as_str()
            .filter(|value| !value.is_empty())
        {
            body.push('\n');
            body.push_str(&escape(context));
        }
        let buttons = options["buttons"].as_array().map_or(0, Vec::len);
        if buttons > 2 {
            return Err(ApiError::malformed_arguments());
        }
        let reply = bounded(service.proxy.call_method("GetCapabilities", &())).await?;
        if reply.body().len() > 4096 {
            return Err(unavailable());
        }
        let capabilities: Vec<String> = reply.body().deserialize().map_err(|_| unavailable())?;
        if capabilities.len() > 64 {
            return Err(unavailable());
        }
        if buttons > 0 && !capabilities.iter().any(|value| value == "actions") {
            return Err(ApiError::other(
                "The notification service does not support buttons.",
            ));
        }
        let mut actions = vec!["default".to_owned(), "Open".to_owned()];
        for (index, button) in options["buttons"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            actions.push(format!("button-{index}"));
            actions.push(button["title"].as_str().unwrap_or_default().to_owned());
        }
        let native: u32 = bounded(service.proxy.call(
            "Notify",
            &(
                "Zephium",
                replaced,
                "",
                title,
                body,
                actions,
                hints,
                if options["requireInteraction"] == true {
                    0i32
                } else {
                    -1i32
                },
            ),
        ))
        .await?;
        let allowed = inner.upgrade().is_some_and(|inner| {
            inner
                .bridge
                .with_runtime(|host, _| host.notification_allowed(install))
                .unwrap_or(false)
        });
        if !self.current(install, generation) || !allowed || !self.same_owner(&service.owner) {
            let _ = bounded(
                service
                    .proxy
                    .call::<_, _, ()>("CloseNotification", &(native,)),
            )
            .await;
            return Err(ApiError::context_invalidated());
        }
        if native == 0
            || self
                .records
                .borrow()
                .iter()
                .any(|(other, row)| *other != key && row.native == native)
        {
            return Err(unavailable());
        }
        self.records.borrow_mut().insert(
            key,
            Record {
                native,
                generation,
                options,
                buttons,
            },
        );
        Ok(if update { json!(true) } else { json!(id) })
    }
}
fn icon(
    root: &Path,
    sender: &Sender,
    raw: &str,
) -> Result<zephium_core::extensions::ExtensionActionIcon, ApiError> {
    use std::io::Read;
    let invalid = || ApiError::other("Unable to successfully use the provided image.");
    let path = if let Ok(url) = url::Url::parse(raw) {
        if url.scheme() != "chrome-extension"
            || url.host_str() != Some(sender.extension_id().as_str())
        {
            return Err(invalid());
        }
        url.path().to_owned()
    } else {
        format!("/{}", raw.trim_start_matches('/'))
    };
    if path.to_ascii_lowercase().ends_with(".svg") {
        return Err(invalid());
    }
    let path = crate::package::ResourcePath::parse(&path).map_err(|_| invalid())?;
    let package = crate::package::PreparedPackage::open(root).map_err(|_| invalid())?;
    let resource = package.open_resource(&path).map_err(|_| invalid())?;
    if resource.length > 1024 * 1024 {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    resource
        .file
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid());
    }
    super::icons::decode(&bytes).ok_or_else(invalid)
}

pub(super) fn submit(
    inner: &Rc<Inner>,
    sender: Box<Sender>,
    root: PathBuf,
    request: Request,
    responder: Responder,
) {
    let backend = &inner.notifications;
    let oversized = match &request {
        Request::Create { options, .. } | Request::Update { options, .. } => {
            options.to_string().len() > MAX_OPTIONS
        }
        _ => false,
    };
    if oversized || backend.pending.borrow().len() >= MAX_PENDING {
        let _ = inner.bridge.with_runtime(|host, out| {
            host.settle_call(
                responder,
                Err(if oversized {
                    ApiError::message_too_large()
                } else {
                    ApiError::too_many_pending()
                }),
                out,
            )
        });
        return;
    }
    backend.pending.borrow_mut().push_back(Job {
        sender,
        root,
        request,
        responder,
    });
    if backend.running.replace(true) {
        return;
    }
    let backend = backend.clone();
    let weak = Rc::downgrade(inner);
    glib::MainContext::default().spawn_local(async move {
        loop {
            let Some(job) = backend.pending.borrow_mut().pop_front() else {
                backend.running.set(false);
                break;
            };
            let result = backend.call(&weak, &job).await;
            if let Some(inner) = weak.upgrade().filter(|inner| !inner.retired.get()) {
                let _ = inner
                    .bridge
                    .with_runtime(|host, out| host.settle_call(job.responder, result, out));
                inner.schedule();
            }
        }
    });
}
