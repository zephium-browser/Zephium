use std::cell::Cell;

use serde_json::json;
use zephium_webext::manifest::Manifest;

use super::*;
use crate::context::tests::{background_sender, content_sender, EXTENSION};
use crate::dispatch::{Dispatcher, Inbound};
use crate::envelope::{ApiName, CallId, Direction, Envelope};
use crate::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};

struct Host {
    messages: Messages,
    reads: Cell<usize>,
}

impl Host {
    fn new(extension: &str) -> Self {
        Self {
            messages: Messages::new(
                ExtensionId::parse(extension).unwrap(),
                LocaleEnvironment { ui_language: "en-GB".into(), accept_languages: vec!["pl".into(), "en".into()], right_to_left: false },
                Some("en"),
                &BTreeMap::from([("en".into(), json!({"html": {"message": "$content$", "placeholders": {"content": {"content": "<b>$1</b>"}}}}))]),
            ).unwrap(),
            reads: Cell::new(0),
        }
    }
}

impl I18nHost for Host {
    fn messages(&self, _: &Sender) -> Result<&Messages, ApiError> {
        self.reads.set(self.reads.get() + 1);
        Ok(&self.messages)
    }
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
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(json!({})));
    let permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    );
    let args = serde_json::to_vec(&args).unwrap();
    let bytes = Envelope::Call {
        id: CallId::new(1).unwrap(),
        name: ApiName::parse(method).unwrap(),
        args: &args,
    }
    .encode(Direction::FromContext, u64::MAX)
    .unwrap();
    let inbound = dispatcher.receive(host, &permissions, sender, &bytes, 0);
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

#[test]
fn background_and_content_calls_need_no_permission_and_share_resolved_messages() {
    let mut dispatcher = dispatcher();
    let mut host = Host::new(EXTENSION);
    let background = background_sender();
    let content = content_sender("https://example.com/");
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &content,
            "i18n.getUILanguage",
            json!([])
        ),
        Ok(json!("en-GB"))
    );
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &background,
            "i18n.getAcceptLanguages",
            json!([])
        ),
        Ok(json!(["pl", "en"]))
    );
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &content,
            "i18n.getMessage",
            json!(["@@extension_id"])
        ),
        Ok(json!(EXTENSION))
    );
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &background,
            "i18n.getMessage",
            json!(["HTML", ["<i>$1</i>"], {"escapeLt": true}])
        ),
        Ok(json!("&lt;b><i>$1</i>&lt;/b>"))
    );
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &background,
            "i18n.getMessage",
            json!(["missing"])
        ),
        Ok(json!(""))
    );
}

#[test]
fn malformed_arguments_are_settled_before_reading_catalogs() {
    let mut dispatcher = dispatcher();
    let mut host = Host::new(EXTENSION);
    let sender = background_sender();
    for (method, args) in [
        ("i18n.getMessage", json!([])),
        ("i18n.getMessage", json!([3])),
        ("i18n.getMessage", json!(["html", vec!["x"; 10]])),
        ("i18n.getMessage", json!(["html", [], {"escapeLt": 1}])),
        ("i18n.getMessage", json!(["html", [], {"unknown": true}])),
        ("i18n.getUILanguage", json!([true])),
        ("i18n.getAcceptLanguages", json!(["en"])),
    ] {
        assert!(call(&mut dispatcher, &mut host, &sender, method, args).is_err());
    }
    assert_eq!(host.reads.get(), 0);
    assert_eq!(dispatcher.pending_calls(), 0);
    assert_eq!(dispatcher.limiter().context_calls(&sender.context_key()), 0);
}

#[test]
fn a_host_cannot_return_another_extensions_catalog() {
    let mut dispatcher = dispatcher();
    let mut host = Host::new("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &background_sender(),
            "i18n.getMessage",
            json!(["html"])
        ),
        Err(ApiError::context_invalidated().message().to_owned())
    );
}

#[test]
fn css_file_replacement_preserves_dollars_and_stops_at_unknown_messages() {
    let messages = Messages::new(
        ExtensionId::parse(EXTENSION).unwrap(),
        LocaleEnvironment {
            ui_language: "en".into(),
            accept_languages: vec![],
            right_to_left: false,
        },
        Some("en"),
        &BTreeMap::from([(
            "en".into(),
            json!({
                "known": {"message":"ok"},
                "empty": {"message":""},
                "nested": {"message":"__MSG_known__"},
                "dollars": {"message":"$name$/$1/$$", "placeholders":{"name":{"content":"value$2"}}}
            }),
        )]),
    )
    .unwrap();
    assert_eq!(
        messages
            .localize_css("__MSG_KNOWN__ __MSG_dollars__ __MSG_nested__")
            .unwrap(),
        "ok value$2/$1/$$ __MSG_known__"
    );
    assert_eq!(
        messages
            .localize_css("__MSG_known__ __MSG_unknown__ __MSG_known__")
            .unwrap(),
        "ok __MSG_unknown__ __MSG_known__"
    );
    assert_eq!(
        messages
            .localize_css("__MSG_invalid-name__ __MSG_empty__ __MSG_known__")
            .unwrap(),
        "__MSG_invalid-name__  ok"
    );
    assert_eq!(
        messages
            .localize_css("__MSG_@@bidi_dir__ __MSG_@@extension_id__")
            .unwrap(),
        format!("ltr {EXTENSION}")
    );
    assert_eq!(
        messages.localize_css("__MSG_unclosed").unwrap(),
        "__MSG_unclosed"
    );
    assert!(messages
        .localize_css(&"x".repeat(MAX_LOCALE_BYTES + 1))
        .is_err());
}

#[test]
fn long_ascii_message_names_use_the_catalog_budget_not_a_name_limit() {
    let name = "long_message_".repeat(40);
    let messages = Messages::new(
        ExtensionId::parse(EXTENSION).unwrap(),
        LocaleEnvironment {
            ui_language: "en".into(),
            accept_languages: vec!["en".into()],
            right_to_left: false,
        },
        Some("en"),
        &BTreeMap::from([("en".into(), json!({name.clone(): {"message": "loaded"}}))]),
    )
    .unwrap();
    assert_eq!(messages.get_message(&name, &[], false).unwrap(), "loaded");
    assert!(!valid_name("invalid-name"));
    assert!(!valid_name(""));
    assert!(!valid_name("mésage"));
}
