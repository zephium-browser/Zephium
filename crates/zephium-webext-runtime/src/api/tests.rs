use std::cell::Cell;

use serde_json::{json, Value};
use zephium_core::ids::ItemId;
use zephium_webext::manifest::Manifest;

use super::*;
use crate::alarms::AlarmHost;
use crate::context::tests::{
    admission, background_sender, committed, content_sender, install, profile, EXTENSION,
};
use crate::context::{
    Authority, ContextKind, ExtensionId, ExtensionScheme, ExternallyConnectable, FrameId, Sender,
    TabDocument,
};
use crate::dispatch::{Dispatcher, Handled, Inbound, Invocation};
use crate::envelope::{ApiName, CallId, Direction, Envelope};
use crate::events::{EventsHost, ListenerRequest};
use crate::i18n::{I18nHost, Messages};
use crate::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};
use crate::ports::{MessageRequest, MessagingHost};
use crate::scripting_registry::{ResourceKind, ScriptRegistry, ScriptingHost};
use crate::storage::{Storage, StorageChangeEvent, StorageHost};

const HOST_REACHED: &str = "Test host reached.";

#[derive(Default)]
struct Host {
    calls: Cell<usize>,
}

impl Host {
    fn reached(&self) -> ApiError {
        self.calls.set(self.calls.get() + 1);
        ApiError::other(HOST_REACHED)
    }
}

impl StorageHost for Host {
    fn storage(&mut self, _: &Sender) -> Result<&mut Storage, ApiError> {
        Err(self.reached())
    }
    fn storage_now_nanos(&self) -> u64 {
        0
    }
    fn storage_changed(&mut self, _: &Sender, _: StorageChangeEvent) {
        self.calls.set(self.calls.get() + 1);
    }
}

impl AlarmHost for Host {
    fn alarm_call(&mut self, _: &Sender, _: &str, _: &[Value]) -> Result<Value, ApiError> {
        Err(self.reached())
    }
}

impl EventsHost for Host {
    fn listener_request(&mut self, _: ListenerRequest, invocation: Invocation<'_>) -> Handled {
        invocation.reply(Err(self.reached()))
    }
}

impl MessagingHost for Host {
    fn message_request(&mut self, _: MessageRequest, invocation: Invocation<'_>) -> Handled {
        invocation.reply(Err(self.reached()))
    }
}

impl I18nHost for Host {
    fn messages(&self, _: &Sender) -> Result<&Messages, ApiError> {
        Err(self.reached())
    }
}

impl ScriptingHost for Host {
    fn script_registry(&self, _: &Sender) -> Result<&ScriptRegistry, ApiError> {
        Err(self.reached())
    }
    fn validate_script_resource(
        &self,
        _: &Sender,
        _: &str,
        _: ResourceKind,
    ) -> Result<(), ApiError> {
        Err(self.reached())
    }
    fn commit_script_registry(&mut self, _: &Sender, _: ScriptRegistry) -> Result<(), ApiError> {
        Err(self.reached())
    }
}

fn permissions(names: &[&str]) -> InstallPermissions {
    let (declaration, _) =
        PermissionDeclaration::from_manifest(&Manifest::from_value(json!({"permissions": names})));
    InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    )
}

fn methods() -> Vec<String> {
    let mut methods = Vec::new();
    for area in ["local", "sync", "session", "managed"] {
        for member in [
            "get",
            "set",
            "remove",
            "clear",
            "getBytesInUse",
            "getKeys",
            "setAccessLevel",
        ] {
            methods.push(format!("storage.{area}.{member}"));
        }
    }
    methods.extend(
        [
            "alarms.create",
            "alarms.get",
            "alarms.getAll",
            "alarms.clear",
            "alarms.clearAll",
            "i18n.getMessage",
            "i18n.getUILanguage",
            "i18n.getAcceptLanguages",
            "scripting.registerContentScripts",
            "scripting.updateContentScripts",
            "scripting.unregisterContentScripts",
            "scripting.getRegisteredContentScripts",
            "runtime.sendMessage",
            "runtime.connect",
            "runtime.connectNative",
            "runtime.sendNativeMessage",
            "tabs.sendMessage",
            "tabs.connect",
            "events.completeRegistration",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    for event in [
        "runtime.onMessage",
        "runtime.onConnect",
        "runtime.onMessageExternal",
        "runtime.onConnectExternal",
        "runtime.onInstalled",
        "runtime.onStartup",
        "runtime.onSuspend",
        "runtime.onSuspendCanceled",
        "storage.onChanged",
        "storage.local.onChanged",
        "storage.sync.onChanged",
        "storage.session.onChanged",
        "storage.managed.onChanged",
        "alarms.onAlarm",
        "tabs.onCreated",
        "tabs.onUpdated",
        "tabs.onRemoved",
        "tabs.onActivated",
        "windows.onCreated",
        "windows.onRemoved",
        "windows.onFocusChanged",
        "webNavigation.onBeforeNavigate",
        "webNavigation.onCommitted",
        "webNavigation.onDOMContentLoaded",
        "webNavigation.onCompleted",
        "webNavigation.onErrorOccurred",
        "webNavigation.onHistoryStateUpdated",
        "webNavigation.onReferenceFragmentUpdated",
        "webRequest.onBeforeRequest",
        "webRequest.onBeforeSendHeaders",
        "webRequest.onSendHeaders",
        "webRequest.onHeadersReceived",
        "webRequest.onResponseStarted",
        "webRequest.onCompleted",
        "webRequest.onErrorOccurred",
        "webRequest.onAuthRequired",
    ] {
        for operation in [
            "addListener",
            "removeListener",
            "hasListener",
            "hasListeners",
        ] {
            methods.push(format!("{event}.{operation}"));
        }
    }
    methods
}

fn call(
    dispatcher: &mut Dispatcher<Host>,
    host: &mut Host,
    sender: &Sender,
    permissions: &InstallPermissions,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let args = serde_json::to_vec(&args).unwrap();
    let bytes = Envelope::Call {
        id: CallId::new(1).unwrap(),
        name: ApiName::parse(method).unwrap(),
        args: &args,
    }
    .encode(Direction::FromContext, u64::MAX)
    .unwrap();
    let inbound = dispatcher.receive(host, permissions, sender, &bytes, 0);
    let Inbound::Reply(reply) = inbound else {
        panic!("expected reply: {inbound:?}");
    };
    let Envelope::Reply { result, .. } =
        Envelope::decode(&reply, Direction::ToContext, u64::MAX).unwrap()
    else {
        panic!("expected reply frame");
    };
    result
        .map(|bytes| serde_json::from_slice(bytes).unwrap())
        .map_err(str::to_owned)
}

fn external_sender() -> Sender {
    let mut authority = Authority::new(profile(), false, ExtensionScheme::CHROME_EXTENSION);
    let mut admitted = admission(install(), EXTENSION, 1);
    admitted.externally_connectable =
        Some(ExternallyConnectable::parse(&["https://example.com/*"]).unwrap());
    authority.admit(admitted).unwrap();
    authority
        .bind_external_page(
            &TabDocument {
                tab: ItemId::from(7),
                frame: FrameId::MAIN,
                committed: committed("https://example.com/", 1),
            },
            &ExtensionId::parse(EXTENSION).unwrap(),
        )
        .unwrap()
}

#[test]
fn aggregate_registration_includes_every_implemented_service_method() {
    let mut table = ApiTable::<Host>::new();
    register(&mut table).unwrap();
    let methods = methods();
    assert_eq!(table.len(), methods.len());
    for method in methods {
        let contexts = table
            .contexts(&method)
            .unwrap_or_else(|| panic!("missing {method}"));
        assert_eq!(
            contexts.contains(ContextKind::ExternalPage),
            matches!(method.as_str(), "runtime.sendMessage" | "runtime.connect"),
            "{method}"
        );
    }
}

#[test]
fn aggregate_dispatch_reaches_each_host_only_after_authorization() {
    let mut table = ApiTable::<Host>::new();
    register(&mut table).unwrap();
    let mut dispatcher = Dispatcher::new(table);
    let mut host = Host::default();
    let background = background_sender();
    let content = content_sender("https://example.com/");
    let none = permissions(&[]);
    let granted = permissions(&["storage", "alarms", "scripting", "nativeMessaging"]);
    for (method, args) in [
        ("storage.local.get", json!([])),
        ("alarms.get", json!([])),
        ("scripting.getRegisteredContentScripts", json!([])),
        ("storage.onChanged.addListener", json!([1])),
        ("alarms.onAlarm.addListener", json!([1])),
        ("runtime.connectNative", json!(["org.example.host"])),
    ] {
        assert_eq!(
            call(&mut dispatcher, &mut host, &background, &none, method, args),
            Err(ApiError::missing_permission().message().to_owned())
        );
    }
    for method in [
        "alarms.get",
        "scripting.getRegisteredContentScripts",
        "runtime.connectNative",
        "events.completeRegistration",
    ] {
        assert_eq!(
            call(
                &mut dispatcher,
                &mut host,
                &content,
                &granted,
                method,
                json!([])
            ),
            Err(ApiError::not_available_in_context(method)
                .message()
                .to_owned())
        );
    }
    assert_eq!(host.calls.get(), 0);
    for (method, args) in [
        ("storage.local.get", json!([])),
        ("alarms.get", json!([])),
        ("scripting.getRegisteredContentScripts", json!([])),
        ("i18n.getUILanguage", json!([])),
        ("runtime.onMessage.addListener", json!([1])),
        ("runtime.sendMessage", json!([{"ping": true}])),
    ] {
        assert_eq!(
            call(
                &mut dispatcher,
                &mut host,
                &background,
                &granted,
                method,
                args
            ),
            Err(HOST_REACHED.into()),
            "{method}"
        );
    }
    assert_eq!(host.calls.get(), 6);
    assert_eq!(dispatcher.pending_calls(), 0);
}

#[test]
fn external_pages_reach_only_the_two_explicit_messaging_entry_points() {
    let mut table = ApiTable::<Host>::new();
    register(&mut table).unwrap();
    let mut dispatcher = Dispatcher::new(table);
    let mut host = Host::default();
    let sender = external_sender();
    let none = permissions(&[]);
    for method in methods()
        .into_iter()
        .filter(|method| !matches!(method.as_str(), "runtime.sendMessage" | "runtime.connect"))
    {
        assert_eq!(
            call(
                &mut dispatcher,
                &mut host,
                &sender,
                &none,
                &method,
                json!([])
            ),
            Err(ApiError::not_available_in_context(&method)
                .message()
                .to_owned()),
            "{method}"
        );
    }
    assert_eq!(host.calls.get(), 0);
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            &none,
            "runtime.sendMessage",
            json!([EXTENSION, {"ping": true}])
        ),
        Err(HOST_REACHED.into())
    );
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            &none,
            "runtime.connect",
            json!([EXTENSION, {}])
        ),
        Err(HOST_REACHED.into())
    );
    assert_eq!(host.calls.get(), 2);
}
