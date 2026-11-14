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
use crate::ApiError;

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

impl crate::browser::BrowserPort for Host {
    fn snapshot(&self, _: &Sender) -> Result<crate::browser::BrowserSnapshot, ApiError> {
        Err(self.reached())
    }
    fn extension_info(&self, _: &Sender) -> Result<crate::browser::ExtensionInfo, ApiError> {
        Err(self.reached())
    }
    fn request(
        &mut self,
        _: crate::browser::BrowserRequest,
        invocation: Invocation<'_>,
    ) -> Handled {
        invocation.reply(Err(self.reached()))
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
    for event in crate::events::EVENT_APIS {
        for operation in [
            "addListener",
            "removeListener",
            "hasListener",
            "hasListeners",
        ] {
            methods.push(format!("{event}.{operation}"));
        }
    }
    methods.extend(super::tabs::METHODS.iter().map(|name| (*name).to_owned()));
    methods.extend(
        super::windows::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::runtime::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(super::action::METHODS.iter().map(|name| (*name).to_owned()));
    methods.extend(
        super::commands::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::permissions::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::extension::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::management::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(super::idle::METHODS.iter().map(|name| (*name).to_owned()));
    methods.extend(super::dom::METHODS.iter().map(|name| (*name).to_owned()));
    methods.extend(
        super::font_settings::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::notifications::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::scripting::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::web_navigation::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::history::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::bookmarks::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::sessions::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(
        super::top_sites::METHODS
            .iter()
            .map(|name| (*name).to_owned()),
    );
    methods.extend(super::search::METHODS.iter().map(|name| (*name).to_owned()));
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

#[test]
fn registered_methods_use_generated_metadata_with_the_policy_ceiling() {
    let mut table = ApiTable::<Host>::new();
    register(&mut table).unwrap();
    for name in table.names() {
        assert!(
            crate::schema::contains(name),
            "missing generated member: {name}"
        );
        let access =
            crate::schema::access(name).unwrap_or_else(|| panic!("missing metadata: {name}"));
        assert_eq!(table.gate(name), Some(access.gate), "{name}");
        assert_eq!(
            table.contexts(name),
            Some(
                access
                    .contexts
                    .intersection(crate::dispatch::permitted_contexts(name))
            ),
            "{name}"
        );
    }
    for (name, permission) in [
        ("storage.session.get", "storage"),
        ("storage.local.onChanged.addListener", "storage"),
        ("alarms.create", "alarms"),
        ("alarms.onAlarm.addListener", "alarms"),
        ("scripting.registerContentScripts", "scripting"),
        ("runtime.connectNative", "nativeMessaging"),
        ("webNavigation.onCommitted.addListener", "webNavigation"),
        ("webRequest.onBeforeRequest.addListener", "webRequest"),
    ] {
        assert_eq!(table.gate(name), Some(crate::Gate::Requires(permission)));
    }
    assert_eq!(table.gate("tabs.connect"), Some(crate::Gate::Open));
    assert_eq!(
        table.gate("windows.onCreated.addListener"),
        Some(crate::Gate::Open)
    );
    assert!(!table
        .contexts("storage.local.get")
        .unwrap()
        .contains(ContextKind::UserScript));
    assert!(!table
        .contexts("alarms.get")
        .unwrap()
        .contains(ContextKind::Offscreen));
    assert!(table
        .contexts("runtime.connect")
        .unwrap()
        .contains(ContextKind::UserScript));
}

#[test]
fn normalized_optional_arguments_reach_services_and_invalid_shapes_do_not() {
    let mut table = ApiTable::<Host>::new();
    register(&mut table).unwrap();
    let mut dispatcher = Dispatcher::new(table);
    let mut host = Host::default();
    let sender = background_sender();
    let granted = permissions(&[
        "storage",
        "alarms",
        "scripting",
        "nativeMessaging",
        "webNavigation",
        "webRequest",
    ]);
    for (method, args) in [
        ("storage.local.get", json!([null])),
        ("storage.local.getBytesInUse", json!([])),
        ("alarms.create", json!([{"delayInMinutes":1}])),
        ("alarms.get", json!([null])),
        ("alarms.clear", json!([])),
        ("i18n.getMessage", json!(["hello"])),
        ("i18n.getMessage", json!(["hello", null, null])),
        ("scripting.getRegisteredContentScripts", json!([null])),
        ("scripting.unregisterContentScripts", json!([])),
        ("runtime.connect", json!([])),
        ("runtime.connect", json!([{"name":"port"}])),
        ("runtime.sendMessage", json!([{"nested":null}])),
        (
            "runtime.sendMessage",
            json!([EXTENSION, {"nested":null}, null]),
        ),
        ("tabs.connect", json!([1])),
        ("tabs.sendMessage", json!([1, null])),
        (
            "runtime.sendNativeMessage",
            json!(["org.example.host", {"nested":null}]),
        ),
        ("webNavigation.onCommitted.addListener", json!([1])),
        (
            "webRequest.onBeforeRequest.addListener",
            json!([1, {"urls":["https://example.com/*"]}]),
        ),
    ] {
        assert_eq!(
            call(&mut dispatcher, &mut host, &sender, &granted, method, args),
            Err(HOST_REACHED.into()),
            "{method}"
        );
    }
    let count = host.calls.get();
    for (method, args) in [
        ("storage.local.set", json!([[]])),
        ("alarms.create", json!([{"delayInMinutes":"soon"}])),
        ("i18n.getMessage", json!(["hello", [], {"escapeLt":1}])),
        (
            "scripting.registerContentScripts",
            json!([[{"id":"a","world":"BAD"}]]),
        ),
        ("runtime.connect", json!([{"unknown":true}])),
        ("tabs.sendMessage", json!([1, {}, {"frameId":-1}])),
        ("storage.onChanged.addListener", json!([1.5])),
        (
            "webRequest.onBeforeRequest.addListener",
            json!([1, {"urls":["https://example.com/*"], "types":["invalid"]}]),
        ),
        (
            "webNavigation.onCommitted.addListener",
            json!([1, {"url":[{"unknown":true}]}]),
        ),
    ] {
        let expected = crate::schema::normalize(method, args.as_array().unwrap()).unwrap_err();
        assert_eq!(
            call(&mut dispatcher, &mut host, &sender, &granted, method, args),
            Err(expected.message().to_owned()),
            "{method}"
        );
    }
    assert_eq!(host.calls.get(), count);
}

#[test]
fn generated_methods_without_handlers_are_an_explicit_backlog() {
    let mut table = ApiTable::<Host>::new();
    register(&mut table).unwrap();
    let implemented: std::collections::BTreeSet<_> = table.names().collect();
    let mut namespaces: std::collections::BTreeSet<_> = table
        .names()
        .filter_map(|name| {
            crate::schema::source_method(name)
                .split_once('.')
                .map(|(namespace, _)| namespace)
        })
        .collect();
    namespaces.insert("dom");
    let missing: Vec<_> = generated::METHOD_VALIDATORS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| {
            namespaces.contains(name.split_once('.').unwrap().0) && !implemented.contains(name)
        })
        .collect();
    assert_eq!(
        missing,
        [
            "dom.openOrClosedShadowRoot",
            "extension.sendRequest",
            "fontSettings.clearDefaultFixedFontSize",
            "fontSettings.clearDefaultFontSize",
            "fontSettings.clearFont",
            "fontSettings.clearMinimumFontSize",
            "fontSettings.getDefaultFixedFontSize",
            "fontSettings.getDefaultFontSize",
            "fontSettings.getFont",
            "fontSettings.getMinimumFontSize",
            "fontSettings.setDefaultFixedFontSize",
            "fontSettings.setDefaultFontSize",
            "fontSettings.setFont",
            "fontSettings.setMinimumFontSize",
            "i18n.detectLanguage",
            "management.createAppShortcut",
            "management.generateAppForLink",
            "management.get",
            "management.getAll",
            "management.getPermissionWarningsById",
            "management.getPermissionWarningsByManifest",
            "management.installReplacementWebApp",
            "management.launchApp",
            "management.setEnabled",
            "management.setLaunchType",
            "management.uninstall",
            "management.uninstallSelf",
            "runtime.markListenerRegistrationComplete",
            "tabs.executeScript",
            "tabs.insertCSS",
            "tabs.removeCSS",
            "tabs.sendRequest",
            "webRequest.handlerBehaviorChanged",
        ]
    );
}
