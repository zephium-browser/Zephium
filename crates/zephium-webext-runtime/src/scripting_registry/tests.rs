use std::cell::Cell;

use serde_json::json;

use super::*;
use crate::context::tests::{background_sender, content_sender};
use crate::dispatch::{Dispatcher, Inbound};
use crate::envelope::{ApiName, CallId, Direction, Envelope};
use crate::errors::ApiErrorKind;
use crate::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};

#[derive(Default)]
struct Host {
    registry: ScriptRegistry,
    reads: Cell<usize>,
    validations: Cell<usize>,
    commits: usize,
    fail_commit: bool,
    fail_resource: bool,
}

impl ScriptingHost for Host {
    fn script_registry(&self, _: &Sender) -> Result<&ScriptRegistry, ApiError> {
        self.reads.set(self.reads.get() + 1);
        Ok(&self.registry)
    }

    fn validate_script_resource(
        &self,
        _: &Sender,
        _: &str,
        _: ResourceKind,
    ) -> Result<(), ApiError> {
        self.validations.set(self.validations.get() + 1);
        if self.fail_resource {
            Err(ApiError::other("Script resource cannot be read."))
        } else {
            Ok(())
        }
    }

    fn commit_script_registry(
        &mut self,
        _: &Sender,
        candidate: ScriptRegistry,
    ) -> Result<(), ApiError> {
        self.commits += 1;
        if self.fail_commit {
            return Err(ApiError::other("Registry persistence failed."));
        }
        self.registry = candidate;
        Ok(())
    }
}

fn permissions(granted: bool) -> InstallPermissions {
    let manifest = Manifest::from_value(if granted {
        json!({"permissions": ["scripting"]})
    } else {
        json!({})
    });
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    )
}

fn dispatcher() -> Dispatcher<Host> {
    let mut table = ApiTable::new();
    register(&mut table).unwrap();
    Dispatcher::new(table)
}

fn call(
    dispatcher: &mut Dispatcher<Host>,
    host: &mut Host,
    sender: &Sender,
    granted: bool,
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
    let inbound = dispatcher.receive(host, &permissions(granted), sender, &bytes, 0);
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

fn registration(id: &str) -> Value {
    json!({"id": id, "matches": ["https://example.com/*"], "js": ["content.js"]})
}

#[test]
fn dispatcher_checks_context_permission_and_arguments_before_host_access() {
    let mut dispatcher = dispatcher();
    let mut host = Host::default();
    let background = background_sender();
    let content = content_sender("https://example.com/");
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &content,
            true,
            "scripting.registerContentScripts",
            json!([[registration("a")]])
        ),
        Err(
            ApiError::not_available_in_context("scripting.registerContentScripts")
                .message()
                .to_owned()
        )
    );
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &background,
            false,
            "scripting.registerContentScripts",
            json!([[registration("a")]])
        ),
        Err(ApiError::missing_permission().message().to_owned())
    );
    for (method, args) in [
        ("scripting.registerContentScripts", json!([])),
        (
            "scripting.registerContentScripts",
            json!([[{"id": "a", "matches": [3]}]]),
        ),
        ("scripting.updateContentScripts", json!([3])),
        ("scripting.unregisterContentScripts", json!([{"ids": [3]}])),
        (
            "scripting.getRegisteredContentScripts",
            json!([{"other": true}]),
        ),
    ] {
        assert!(call(&mut dispatcher, &mut host, &background, true, method, args).is_err());
    }
    assert_eq!(host.reads.get(), 0);
    assert_eq!(host.validations.get(), 0);
    assert_eq!(host.commits, 0);
}

#[test]
fn failed_native_commit_and_failed_resource_validation_preserve_registry() {
    let mut dispatcher = dispatcher();
    let mut host = Host::default();
    let sender = background_sender();
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            true,
            "scripting.registerContentScripts",
            json!([[registration("a")]])
        ),
        Ok(Value::Null)
    );
    let before = host.registry.snapshot();
    host.fail_commit = true;
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            true,
            "scripting.updateContentScripts",
            json!([[{"id": "a", "world": "MAIN", "allFrames": true}]])
        ),
        Err("Registry persistence failed.".into())
    );
    assert_eq!(host.registry.snapshot(), before);
    let commits = host.commits;
    host.fail_commit = false;
    host.fail_resource = true;
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            true,
            "scripting.registerContentScripts",
            json!([[registration("b")]])
        ),
        Err("Script resource cannot be read.".into())
    );
    assert_eq!(host.registry.snapshot(), before);
    assert_eq!(host.commits, commits);
    let result = call(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "scripting.getRegisteredContentScripts",
        json!([{"ids": ["a", "missing"]}]),
    )
    .unwrap();
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["world"], "ISOLATED");
    host.fail_resource = false;
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            true,
            "scripting.unregisterContentScripts",
            json!([{"ids": ["a"]}])
        ),
        Ok(Value::Null)
    );
    assert!(host.registry.snapshot().is_empty());
}

#[test]
fn registered_projection_omits_empty_lists_without_changing_durable_snapshots() {
    let mut dispatcher = dispatcher();
    let mut host = Host::default();
    let sender = background_sender();
    call(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "scripting.registerContentScripts",
        json!([[registration("probe")]]),
    )
    .unwrap();
    let registered = call(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "scripting.getRegisteredContentScripts",
        json!([]),
    )
    .unwrap();
    assert_eq!(
        registered,
        json!([{
            "id": "probe", "matches": ["https://example.com/*"], "js": ["content.js"],
            "allFrames": false, "matchOriginAsFallback": false, "persistAcrossSessions": true,
            "runAt": "document_idle", "world": "ISOLATED"
        }])
    );
    let snapshot = serde_json::to_value(host.registry.snapshot()).unwrap();
    assert_eq!(snapshot[0]["css"], json!([]));
    assert_eq!(snapshot[0]["excludeMatches"], json!([]));

    call(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "scripting.updateContentScripts",
        json!([[{"id": "probe", "js": [], "css": ["content.css"], "excludeMatches": ["https://example.com/private/*"]}]]),
    )
    .unwrap();
    let registered = call(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "scripting.getRegisteredContentScripts",
        json!([]),
    )
    .unwrap();
    assert!(registered[0].get("js").is_none());
    assert_eq!(registered[0]["css"], json!(["content.css"]));
    assert_eq!(
        registered[0]["excludeMatches"],
        json!(["https://example.com/private/*"])
    );
}

#[test]
fn duplicate_and_missing_registration_errors_name_the_id_and_preserve_atomicity() {
    let mut dispatcher = dispatcher();
    let mut host = Host::default();
    let sender = background_sender();
    call(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "scripting.registerContentScripts",
        json!([[registration("probe")]]),
    )
    .unwrap();
    let before = host.registry.snapshot();
    for (method, args, message) in [
        (
            "scripting.registerContentScripts",
            json!([[registration("other"), registration("probe")]]),
            "Duplicate script ID 'probe'",
        ),
        (
            "scripting.updateContentScripts",
            json!([[{"id": "probe", "world": "MAIN"}, {"id": "probe", "world": "ISOLATED"}]]),
            "Duplicate script ID 'probe'",
        ),
        (
            "scripting.updateContentScripts",
            json!([[{"id": "probe", "world": "MAIN"}, {"id": "missing", "js": ["content.js"]}]]),
            "Script with ID 'missing' does not exist or is not fully registered",
        ),
        (
            "scripting.unregisterContentScripts",
            json!([{"ids": ["probe", "missing"]}]),
            "Script with ID 'missing' does not exist or is not fully registered",
        ),
    ] {
        assert_eq!(
            call(&mut dispatcher, &mut host, &sender, true, method, args),
            Err(message.into())
        );
        assert_eq!(host.registry.snapshot(), before);
        assert_eq!(host.commits, 1);
    }
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            true,
            "scripting.updateContentScripts",
            json!([[{"id": "x".repeat(257)}]]),
        ),
        Err(ApiError::malformed_arguments().message().into())
    );
    assert_eq!(host.registry.snapshot(), before);
    assert_eq!(host.commits, 1);
}

#[test]
fn static_glob_metadata_consumes_the_shared_registry_budget() {
    let glob = "?".repeat(crate::pattern::MAX_GLOB_BYTES);
    let globs = vec![glob; crate::pattern::MAX_GLOBS_PER_SCRIPT];
    let script =
        json!({"matches": ["https://example.com/*"], "include_globs": globs, "js": ["content.js"]});
    assert!(ScriptRegistry::from_manifest(
        &Manifest::from_value(json!({"content_scripts": [script.clone()]})),
        |_, _| Ok(())
    )
    .is_ok());
    let count = MAX_REGISTRY_BYTES
        / (crate::pattern::MAX_GLOB_BYTES * crate::pattern::MAX_GLOBS_PER_SCRIPT)
        + 1;
    let manifest = Manifest::from_value(json!({"content_scripts": vec![script; count]}));
    let error = ScriptRegistry::from_manifest(&manifest, |_, _| Ok(())).unwrap_err();
    assert_eq!(error.kind(), ApiErrorKind::Overloaded);
}
