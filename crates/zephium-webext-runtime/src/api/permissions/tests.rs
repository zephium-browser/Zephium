use super::*;
use crate::browser::fake::FakeBrowser;
use crate::browser::{BrowserSnapshot, ExtensionInfo};
use crate::context::{
    tests::{background_sender, profile},
    Sender,
};
use crate::dispatch::{Dispatcher, Inbound, Responder};
use crate::envelope::{ApiName, CallId, Direction, Envelope};
use crate::permissions::{PermissionDeclaration, SiteAccess, Teardown};
use zephium_webext::manifest::Manifest;

fn state() -> InstallPermissions {
    let (declaration, issues) = PermissionDeclaration::from_manifest(&Manifest::from_value(
        json!({
            "manifest_version":3,"permissions":["storage"],"optional_permissions":["tabs","nativeMessaging"],"optional_host_permissions":["https://example.com/*"]
        }),
    ));
    assert!(issues.is_empty());
    InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    )
}

fn dispatcher() -> Dispatcher<FakeBrowser> {
    let mut table = ApiTable::new();
    register(&mut table).unwrap();
    Dispatcher::new(table)
}

#[test]
fn malformed_undeclared_and_gestureless_requests_never_reach_the_prompt() {
    let mut host = FakeBrowser::new(profile());
    let mut dispatcher = dispatcher();
    let sender = background_sender();
    let state = state();
    for value in [
        json!({"origins":["https://bad host/*"]}),
        json!({"permissions":["bad permission"]}),
        json!({"permissions":["tabs"]}),
    ] {
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &state,
                "permissions.request",
                json!([value])
            )
            .is_err());
    }
    assert!(host.requests.is_empty());
    host.gesture = true;
    assert!(host
        .call(
            &mut dispatcher,
            &sender,
            &state,
            "permissions.request",
            json!([{"permissions":["history"]}])
        )
        .unwrap_err()
        .contains("manifest"));
    assert!(host.requests.is_empty());
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &state,
            "permissions.request",
            json!([{"permissions":["storage"]}])
        )
        .unwrap(),
        true
    );
    assert!(host.requests.is_empty());
    host.reply = Ok(json!(false));
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &state,
            "permissions.request",
            json!([{"permissions":["tabs"]}])
        )
        .unwrap(),
        false
    );
    assert!(matches!(
        host.requests.last(),
        Some(BrowserRequest::Permissions(Request::Prompt { .. }))
    ));
    assert!(!state.has_api("tabs"));
}

#[test]
fn get_all_contains_and_removal_preserve_required_grants_and_carry_teardown() {
    let mut host = FakeBrowser::new(profile());
    let mut dispatcher = dispatcher();
    let sender = background_sender();
    let mut state = state();
    let requested = parse(
        &json!({"permissions":["tabs","nativeMessaging"],"origins":["https://example.com/*"]}),
    )
    .unwrap();
    let effect = state.grant(&requested).unwrap();
    assert_eq!(effect.added_permissions.len(), 2);
    assert_eq!(
        to_value(&requested),
        json!({"permissions":["tabs","nativeMessaging"],"origins":["https://example.com/*"]})
    );
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &state,
            "permissions.contains",
            json!([to_value(&requested)])
        )
        .unwrap(),
        true
    );
    let all = host
        .call(
            &mut dispatcher,
            &sender,
            &state,
            "permissions.getAll",
            json!([]),
        )
        .unwrap();
    assert!(all["permissions"]
        .as_array()
        .unwrap()
        .contains(&json!("storage")));
    let count = host.requests.len();
    assert!(host
        .call(
            &mut dispatcher,
            &sender,
            &state,
            "permissions.remove",
            json!([{"permissions":["storage"]}])
        )
        .is_err());
    assert_eq!(host.requests.len(), count);
    host.reply = Ok(json!(true));
    host.call(
        &mut dispatcher,
        &sender,
        &state,
        "permissions.remove",
        json!([to_value(&requested)]),
    )
    .unwrap();
    let Some(BrowserRequest::Permissions(Request::Remove {
        updated, effect, ..
    })) = host.requests.last()
    else {
        panic!("typed removal")
    };
    assert!(!updated.has_api("tabs"));
    assert!(updated.has_api("storage"));
    assert!(state.has_api("tabs"));
    for required in [
        Teardown::RestartContexts,
        Teardown::RefreshContentScripts,
        Teardown::RepublishNetworkRules,
        Teardown::DisconnectNativeHosts,
        Teardown::NotifyRemoved,
    ] {
        assert!(effect.teardown.contains(&required), "{required:?}");
    }
}

struct DeferredHost {
    browser: FakeBrowser,
    committed: InstallPermissions,
    pending: Option<(Request, Responder)>,
    applied: Vec<RevocationEffect>,
    order: Vec<&'static str>,
}

impl BrowserPort for DeferredHost {
    fn snapshot(&self, sender: &Sender) -> Result<BrowserSnapshot, ApiError> {
        self.browser.snapshot(sender)
    }
    fn extension_info(&self, sender: &Sender) -> Result<ExtensionInfo, ApiError> {
        self.browser.extension_info(sender)
    }
    fn user_gesture(&self, _: &Sender) -> bool {
        true
    }
    fn request(&mut self, request: BrowserRequest, call: Invocation<'_>) -> Handled {
        let BrowserRequest::Permissions(request) = request else {
            return call.reply(Err(ApiError::unknown_method()));
        };
        let (handled, responder) = call.defer();
        self.pending = Some((request, responder));
        handled
    }
}

impl DeferredHost {
    fn finish(&mut self, persisted: bool) -> (Responder, Result<Value, ApiError>) {
        let (request, responder) = self.pending.take().unwrap();
        if !persisted {
            return (responder, Err(ApiError::other("Storage failure.")));
        }
        self.order.push("persist");
        match request {
            Request::Prompt { request } => {
                self.committed.grant(&request).unwrap();
                self.order.push("added");
            }
            request @ Request::Remove { .. } => {
                let (updated, effect) = request.rebase_removal(&self.committed).unwrap().unwrap();
                self.committed = updated;
                self.applied.push(effect);
                self.order.push("teardown");
            }
        }
        self.order.push("settle");
        (responder, Ok(json!(true)))
    }
}

fn receive(
    host: &mut DeferredHost,
    dispatcher: &mut Dispatcher<DeferredHost>,
    sender: &Sender,
    id: u32,
    method: &str,
    args: Value,
) {
    let payload = serde_json::to_vec(&args).unwrap();
    let wire = Envelope::Call {
        id: CallId::new(id).unwrap(),
        name: ApiName::parse(method).unwrap(),
        args: &payload,
    }
    .encode(Direction::FromContext, u64::MAX)
    .unwrap();
    let observed = host.committed.clone();
    assert!(matches!(
        dispatcher.receive(host, &observed, sender, &wire, 0),
        Inbound::Pending
    ));
}

#[test]
fn deferred_replies_wait_for_commit_and_apply_every_revocation_effect() {
    let sender = background_sender();
    let mut host = DeferredHost {
        browser: FakeBrowser::new(profile()),
        committed: state(),
        pending: None,
        applied: Vec::new(),
        order: Vec::new(),
    };
    let mut table = ApiTable::new();
    register(&mut table).unwrap();
    let mut dispatcher = Dispatcher::new(table);
    let request = json!([{"permissions":["nativeMessaging"],"origins":["https://example.com/*"]}]);
    receive(
        &mut host,
        &mut dispatcher,
        &sender,
        1,
        "permissions.request",
        request.clone(),
    );
    assert!(!host.committed.has_api("nativeMessaging"));
    let (responder, result) = host.finish(false);
    let reply = dispatcher.settle(responder, result).unwrap();
    assert!(matches!(
        Envelope::decode(&reply.frame, Direction::ToContext, u64::MAX).unwrap(),
        Envelope::Reply {
            result: Err("Storage failure."),
            ..
        }
    ));
    assert!(!host.committed.has_api("nativeMessaging"));
    assert!(host.order.is_empty());
    receive(
        &mut host,
        &mut dispatcher,
        &sender,
        2,
        "permissions.request",
        request.clone(),
    );
    let (responder, result) = host.finish(true);
    dispatcher.settle(responder, result).unwrap();
    assert_eq!(host.order, ["persist", "added", "settle"]);
    assert!(host.committed.has_api("nativeMessaging"));
    host.order.clear();
    receive(
        &mut host,
        &mut dispatcher,
        &sender,
        3,
        "permissions.remove",
        request,
    );
    assert!(host.committed.has_api("nativeMessaging"));
    assert!(host.applied.is_empty());
    // Another accepted prompt settles while this removal is queued.
    host.committed
        .grant(&parse(&json!({"permissions":["tabs"]})).unwrap())
        .unwrap();
    let (responder, result) = host.finish(true);
    dispatcher.settle(responder, result).unwrap();
    assert_eq!(host.order, ["persist", "teardown", "settle"]);
    assert!(!host.committed.has_api("nativeMessaging"));
    assert!(host.committed.has_api("tabs"));
    assert!(host.applied[0]
        .teardown
        .contains(&Teardown::RestartContexts));
    assert!(host.applied[0]
        .teardown
        .contains(&Teardown::DisconnectNativeHosts));
}
