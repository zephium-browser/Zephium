use super::*;
use crate::browser::{fake::FakeBrowser, BrowserRequest, BrowserSnapshot, ExtensionInfo};
use crate::context::tests::{admission, background_sender, committed, install, profile, EXTENSION};
use crate::context::{Authority, ContextKind, ExtensionScheme, ViewKind};
use crate::dispatch::{Dispatcher, Inbound};
use crate::envelope::{ApiName, CallId, Direction, Envelope};
use crate::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};
use proptest::prelude::*;
use zephium_webext::manifest::Manifest;

fn rule(id: u32) -> Value {
    json!({"id":id,"action":{"type":"block"},"condition":{"urlFilter":"ads","resourceTypes":["xmlhttprequest"]}})
}

fn update(kind: dnr::RulesetKind, remove: Vec<u32>, add: Vec<Value>) -> Request {
    Request::UpdateRules {
        kind,
        remove_rule_ids: remove,
        add_rules: add,
    }
}

fn permissions(names: &[&str], hosts: &[&str]) -> InstallPermissions {
    let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
        json!({"permissions":names,"host_permissions":hosts}),
    ));
    InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    )
}

fn unpacked_sender() -> Sender {
    let mut authority = Authority::new(profile(), false, ExtensionScheme::CHROME_EXTENSION);
    let mut admitted = admission(install(), EXTENSION, 1);
    admitted.unpacked = true;
    authority.admit(admitted).unwrap();
    let view = authority
        .open_view(install(), ViewKind::Background)
        .unwrap();
    authority
        .bind_view(
            &view,
            &committed(
                &format!("chrome-extension://{EXTENSION}/background.html"),
                1,
            ),
        )
        .unwrap()
}

struct Host {
    state: DnrState,
    browser: FakeBrowser,
    fail_publication: bool,
}

impl Host {
    fn new() -> Self {
        let mut browser = FakeBrowser::new(profile());
        let window = browser.add_window(false);
        browser.add_tab(window, "https://example.com/", true);
        Self {
            state: DnrState::default(),
            browser,
            fail_publication: false,
        }
    }
}

impl BrowserPort for Host {
    fn snapshot(&self, sender: &Sender) -> Result<BrowserSnapshot, ApiError> {
        self.browser.snapshot(sender)
    }
    fn extension_info(&self, sender: &Sender) -> Result<ExtensionInfo, ApiError> {
        self.browser.extension_info(sender)
    }
    fn request(&mut self, request: BrowserRequest, call: Invocation<'_>) -> Handled {
        self.browser.request(request, call)
    }
    fn user_gesture(&self, sender: &Sender) -> bool {
        self.browser.user_gesture(sender)
    }
    fn now_ms(&self) -> u64 {
        self.browser.now_ms()
    }
}

impl DnrHost for Host {
    fn dnr_state(&self, _: &Sender) -> Result<&DnrState, ApiError> {
        Ok(&self.state)
    }
    fn dnr_request(&mut self, request: Request, call: Invocation<'_>) -> Handled {
        let result = self.state.execute(&request).and_then(|(state, reply)| {
            if self.fail_publication {
                return Err(ApiError::other("Publication failed."));
            }
            self.state = state;
            Ok(reply)
        });
        call.reply(result)
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
    let Inbound::Reply(reply) = dispatcher.receive(host, permissions, sender, &bytes, 0) else {
        panic!("immediate test reply expected");
    };
    let Envelope::Reply { result, .. } =
        Envelope::decode(&reply, Direction::ToContext, u64::MAX).unwrap()
    else {
        panic!("reply expected");
    };
    result
        .map(|bytes| serde_json::from_slice(bytes).unwrap())
        .map_err(str::to_owned)
}

#[test]
fn atomic_duplicate_rejection_removal_replacement_and_publication_failure() {
    let sender = background_sender();
    let permissions = permissions(&["declarativeNetRequest"], &[]);
    let mut host = Host::new();
    let mut dispatcher = dispatcher();
    let add = "declarativeNetRequest.updateDynamicRules";
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            &permissions,
            add,
            json!([{"addRules":[rule(1)]}])
        ),
        Ok(Value::Null)
    );
    let before = host.state.snapshot().unwrap();
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            &permissions,
            add,
            json!([{"removeRuleIds":[1],"addRules":[rule(2),rule(2)]}])
        ),
        Err("Rule with id 2 does not have a unique ID.".into())
    );
    assert_eq!(host.state.snapshot().unwrap(), before);
    host.fail_publication = true;
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            &permissions,
            add,
            json!([{"removeRuleIds":[1],"addRules":[rule(3)]}])
        ),
        Err("Publication failed.".into())
    );
    assert_eq!(host.state.snapshot().unwrap(), before);
    host.fail_publication = false;
    let mut replacement = rule(1);
    replacement["priority"] = json!(7);
    assert!(call(
        &mut dispatcher,
        &mut host,
        &sender,
        &permissions,
        add,
        json!([{"removeRuleIds":[1,999],"addRules":[replacement]}])
    )
    .is_ok());
    assert_eq!(host.state.dynamic_rules[0]["priority"], 7);
}

#[test]
fn sessions_are_independent_and_tab_ids_are_rejected_in_dynamic_rules() {
    let state = DnrState::default()
        .prepare(&update(dnr::RulesetKind::Dynamic, vec![], vec![rule(1)]))
        .unwrap();
    let mut scoped = rule(1);
    scoped["condition"]["tabIds"] = json!([3]);
    assert!(state
        .prepare(&update(
            dnr::RulesetKind::Dynamic,
            vec![],
            vec![scoped.clone()]
        ))
        .is_err());
    let session = state
        .prepare(&update(dnr::RulesetKind::Session, vec![], vec![scoped]))
        .unwrap();
    assert_eq!(session.dynamic_rules[0]["id"], 1);
    assert_eq!(session.session_rules[0]["id"], 1);
    assert_eq!(
        session.rules(dnr::RulesetKind::Session).unwrap()[0]
            .condition
            .tab_ids,
        Some(vec![3])
    );
    assert!(state.session_rules.is_empty());
}

#[test]
fn generated_getter_filter_keeps_only_requested_rules_and_defaults_priority() {
    let sender = background_sender();
    let permissions = permissions(&["declarativeNetRequestWithHostAccess"], &[]);
    let mut host = Host::new();
    host.state = host
        .state
        .prepare(&update(
            dnr::RulesetKind::Dynamic,
            vec![],
            vec![rule(1), rule(2)],
        ))
        .unwrap();
    let mut dispatcher = dispatcher();
    let value = call(
        &mut dispatcher,
        &mut host,
        &sender,
        &permissions,
        "declarativeNetRequest.getDynamicRules",
        json!([{"ruleIds":[2,999]}]),
    )
    .unwrap();
    let mut expected = rule(2);
    expected["priority"] = json!(1);
    assert_eq!(value, json!([expected]));
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            &permissions,
            "declarativeNetRequest.getDynamicRules",
            json!([{"ruleIds":[]}])
        ),
        Ok(json!([]))
    );
}

fn static_state() -> DnrState {
    DnrState::from_static(vec![
        StaticRuleset {
            id: "first".into(),
            enabled: true,
            rules: parse_rules(&[rule(1), rule(2)], dnr::RulesetKind::Static).unwrap(),
        },
        StaticRuleset {
            id: "second".into(),
            enabled: false,
            rules: parse_rules(&[rule(3)], dnr::RulesetKind::Static).unwrap(),
        },
    ])
    .unwrap()
}

#[test]
fn static_choices_are_atomic_and_disabled_rules_apply_when_later_enabled() {
    let initial = static_state();
    let staged = initial
        .prepare(&Request::UpdateStaticRules {
            ruleset_id: "second".into(),
            enable: vec![],
            disable: vec![3, 999],
        })
        .unwrap();
    assert_eq!(staged.enabled_static_rule_count(), 2);
    let enabled = staged
        .prepare(&Request::UpdateEnabledRulesets {
            enable: vec!["second".into()],
            disable: vec!["first".into()],
        })
        .unwrap();
    assert_eq!(enabled.enabled_static_rule_count(), 0);
    assert!(enabled
        .prepare(&Request::UpdateEnabledRulesets {
            enable: vec!["first".into(), "missing".into()],
            disable: vec!["second".into()]
        })
        .is_err());
    let restored = enabled
        .prepare(&Request::UpdateStaticRules {
            ruleset_id: "second".into(),
            enable: vec![3],
            disable: vec![],
        })
        .unwrap();
    assert_eq!(restored.enabled_static_rule_count(), 1);
    let mut limited = initial.clone();
    limited.static_rule_limit = 2;
    assert!(limited
        .prepare(&Request::UpdateEnabledRulesets {
            enable: vec!["second".into()],
            disable: vec![]
        })
        .is_err());
    assert_eq!(initial.enabled_static_rule_count(), 2);
}

#[test]
fn durable_snapshot_excludes_sessions_feedback_and_badges_and_restore_is_atomic() {
    let mut state = static_state()
        .prepare(&update(dnr::RulesetKind::Dynamic, vec![], vec![rule(4)]))
        .unwrap()
        .prepare(&update(dnr::RulesetKind::Session, vec![], vec![rule(5)]))
        .unwrap();
    state = state
        .prepare(&Request::UpdateEnabledRulesets {
            enable: vec!["second".into()],
            disable: vec!["first".into()],
        })
        .unwrap();
    state = state
        .prepare(&Request::UpdateStaticRules {
            ruleset_id: "second".into(),
            enable: vec![],
            disable: vec![3],
        })
        .unwrap();
    state.record_match(dnr::DYNAMIC_RULESET_ID, 4, 1, 1.0);
    state.display_action_count_as_badge_text = true;
    let bytes = state.snapshot().unwrap();
    let mut restored = static_state();
    restored.restore(&bytes).unwrap();
    assert_eq!(restored.dynamic_rules, state.dynamic_rules);
    assert!(restored.session_rules.is_empty());
    assert!(restored.display_action_count_as_badge_text);
    assert!(restored.badge_counts.is_empty());
    assert!(restored.matched.is_empty());
    assert_eq!(restored.enabled_static_rule_count(), 0);
    let before = restored.snapshot().unwrap();
    assert!(restored.restore(b"{\"version\":1,\"dynamic_rules\":[],\"enabled_rulesets\":[\"missing\"],\"disabled_static_rules\":{}}").is_err());
    assert_eq!(restored.snapshot().unwrap(), before);
    assert!(restored.restore(b"{\"version\":2,\"dynamic_rules\":[],\"enabled_rulesets\":[],\"disabled_static_rules\":{}}").is_err());
}

#[test]
fn regex_grammar_matches_reference_and_memory_refusal_is_explicit() {
    assert_eq!(
        regex_support(&json!({"regex":"^https?://(example)\\.com/","requireCapturing":true})),
        json!({"isSupported":true})
    );
    for regex in ["(", "(a)\\1", "(?=a)", "é"] {
        assert_eq!(
            regex_support(&json!({"regex":regex})),
            json!({"isSupported":false,"reason":"syntaxError"})
        );
    }
    assert_eq!(
        regex_support(&json!({"regex":"a".repeat(dnr::MAX_REGEX_SOURCE_BYTES+1)})),
        json!({"isSupported":false,"reason":"memoryLimitExceeded"})
    );
}

#[test]
fn feedback_requires_feedback_permission_or_an_explicit_current_active_tab_grant() {
    let sender = background_sender();
    let mut host = Host::new();
    host.state = host
        .state
        .prepare(&update(dnr::RulesetKind::Dynamic, vec![], vec![rule(1)]))
        .unwrap();
    assert!(host
        .state
        .record_match(dnr::DYNAMIC_RULESET_ID, 1, 1, 100.0));
    let mut dispatcher = dispatcher();
    let mut active = permissions(&["declarativeNetRequest", "activeTab"], &["<all_urls>"]);
    let get = "declarativeNetRequest.getMatchedRules";
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            &active,
            get,
            json!([{"tabId":1}])
        ),
        Err(FEEDBACK_PERMISSION_ERROR.into())
    );
    let tab = host.browser.snapshot.tab(1).unwrap();
    active.grant_active_tab(tab.tab.id(), tab.document, &tab.url().unwrap());
    let result = call(
        &mut dispatcher,
        &mut host,
        &sender,
        &active,
        get,
        json!([{"tabId":1}]),
    )
    .unwrap();
    assert_eq!(result["rulesMatchedInfo"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(&mut dispatcher, &mut host, &sender, &active, get, json!([])),
        Err(FEEDBACK_PERMISSION_ERROR.into())
    );
    host.browser
        .navigate(1, "https://different.test/", &active)
        .unwrap();
    assert_eq!(
        call(
            &mut dispatcher,
            &mut host,
            &sender,
            &active,
            get,
            json!([{"tabId":1}])
        ),
        Err(FEEDBACK_PERMISSION_ERROR.into())
    );
    let feedback = permissions(
        &["declarativeNetRequest", "declarativeNetRequestFeedback"],
        &[],
    );
    assert!(call(
        &mut dispatcher,
        &mut host,
        &sender,
        &feedback,
        get,
        json!([])
    )
    .is_ok());
}

#[test]
fn feedback_is_bounded_quota_shared_and_untrusted_rule_reports_rejected() {
    let mut state = DnrState::default()
        .prepare(&update(dnr::RulesetKind::Dynamic, vec![], vec![rule(1)]))
        .unwrap();
    assert!(!state.record_match("missing", 1, 1, 0.0));
    assert!(!state.record_match(dnr::DYNAMIC_RULESET_ID, 2, 1, 0.0));
    assert!(!state.record_match(dnr::DYNAMIC_RULESET_ID, 1, 1, f64::NAN));
    for index in 0..MAX_MATCHED_RULES + 1 {
        assert!(state.record_match(dnr::DYNAMIC_RULESET_ID, 1, 1, index as f64));
    }
    assert_eq!(state.matched.len(), MAX_MATCHED_RULES);
    for _ in 0..MAX_GET_MATCHED_RULES_CALLS {
        state
            .matched_rules(Some(1), Some(9_999.0), 0, false)
            .unwrap();
    }
    assert!(state.matched_rules(None, None, 0, false).is_err());
    assert!(state.matched_rules(None, None, 0, true).is_ok());
    assert!(state
        .matched_rules(None, None, GET_MATCHED_RULES_INTERVAL_MS, false)
        .is_ok());
    state.navigation_committed(1);
    assert!(!state.badge_counts.contains_key(&1));
}

#[test]
fn debug_and_test_gates_keep_unpacked_and_feedback_requirements() {
    let packed = background_sender();
    let unpacked = unpacked_sender();
    for base in [
        "declarativeNetRequest",
        "declarativeNetRequestWithHostAccess",
    ] {
        let plain = permissions(&[base], &[]);
        let feedback = permissions(&[base, "declarativeNetRequestFeedback"], &[]);
        let debug =
            crate::schema::access("declarativeNetRequest.onRuleMatchedDebug.addListener").unwrap();
        assert!(debug.gate.check(&plain, &unpacked, None).is_err());
        assert!(debug.gate.check(&feedback, &packed, None).is_err());
        assert!(debug.gate.check(&feedback, &unpacked, None).is_ok());
        let test = crate::schema::access("declarativeNetRequest.testMatchOutcome").unwrap();
        assert!(test.gate.check(&plain, &packed, None).is_err());
        assert!(test.gate.check(&plain, &unpacked, None).is_ok());
        assert!(
            !crate::dispatch::permitted_contexts("declarativeNetRequest.updateStaticRules")
                .contains(ContextKind::ExtensionFrame)
        );
    }
}

#[test]
fn simulator_uses_priorities_permissions_tab_scope_and_registrable_domains() {
    let mut block = rule(1);
    block["condition"]["domainType"] = json!("firstParty");
    let mut allow = rule(2);
    allow["action"] = json!({"type":"allow"});
    allow["priority"] = json!(2);
    allow["condition"]["tabIds"] = json!([7]);
    let state = DnrState::default()
        .prepare(&update(dnr::RulesetKind::Dynamic, vec![], vec![block]))
        .unwrap()
        .prepare(&update(dnr::RulesetKind::Session, vec![], vec![allow]))
        .unwrap();
    let details = json!({"url":"https://cdn.example.co.uk/ads", "initiator":"https://www.example.co.uk/", "type":"xmlhttprequest", "tabId":7});
    let plain = permissions(&["declarativeNetRequest"], &[]);
    assert_eq!(
        state
            .test_match_outcome(&details, &plain, EXTENSION)
            .unwrap()["matchedRules"],
        json!([{"ruleId":2,"rulesetId":"_session"}])
    );
    let both = permissions(
        &[
            "declarativeNetRequest",
            "declarativeNetRequestWithHostAccess",
        ],
        &[],
    );
    assert_eq!(
        state
            .test_match_outcome(&details, &both, EXTENSION)
            .unwrap(),
        state
            .test_match_outcome(&details, &plain, EXTENSION)
            .unwrap()
    );
    let restricted = permissions(&["declarativeNetRequestWithHostAccess"], &[]);
    assert_eq!(
        state
            .test_match_outcome(&details, &restricted, EXTENSION)
            .unwrap()["matchedRules"],
        json!([])
    );
    let mut details = details;
    details["tabId"] = json!(8);
    assert_eq!(
        state
            .test_match_outcome(&details, &plain, EXTENSION)
            .unwrap()["matchedRules"],
        json!([{"ruleId":1,"rulesetId":"_dynamic"}])
    );
    details["initiator"] = json!("https://different.co.uk/");
    assert_eq!(
        state
            .test_match_outcome(&details, &plain, EXTENSION)
            .unwrap()["matchedRules"],
        json!([])
    );
}

#[test]
fn dnr_badge_overlay_restores_authored_text() {
    use crate::api::action::{ActionStore, Property, Update};
    let mut action = ActionStore::default();
    action
        .apply(Some(1), &Update::BadgeText(Some("custom".into())))
        .unwrap();
    action.set_dnr_badge(1, Some("12")).unwrap();
    assert_eq!(action.get(Some(1), Property::BadgeText), "12");
    action
        .apply(Some(1), &Update::BadgeText(Some("new".into())))
        .unwrap();
    action.set_dnr_badge(1, None).unwrap();
    assert_eq!(action.get(Some(1), Property::BadgeText), "new");
    action.set_dnr_badge(1, Some("1")).unwrap();
    action.reset_tab(1).unwrap();
    assert_eq!(action.get(Some(1), Property::BadgeText), "");
}

proptest! {
    #[test]
    fn duplicate_update_never_loses_existing_rules(ids in prop::collection::btree_set(1u32..1000,0..30), duplicate in 1001u32..2000) {
        let state=DnrState::default().prepare(&update(dnr::RulesetKind::Dynamic,vec![],ids.iter().copied().map(rule).collect())).unwrap();
        let before=state.snapshot().unwrap();
        let request=update(dnr::RulesetKind::Dynamic,ids.iter().copied().collect(),vec![rule(duplicate),rule(duplicate)]);
        prop_assert!(state.prepare(&request).is_err());
        prop_assert_eq!(state.snapshot().unwrap(),before);
    }
}

#[test]
fn extension_update_preserves_dynamic_rules_and_resets_static_choices() {
    let state = static_state()
        .prepare(&update(dnr::RulesetKind::Dynamic, vec![], vec![rule(4)]))
        .unwrap()
        .prepare(&update(dnr::RulesetKind::Session, vec![], vec![rule(5)]))
        .unwrap();
    let bytes = state.snapshot().unwrap();
    let mut updated = DnrState::from_static(vec![StaticRuleset {
        id: "replacement".into(),
        enabled: true,
        rules: vec![],
    }])
    .unwrap();
    assert!(updated.restore(&bytes).is_err());
    updated.restore_after_update(&bytes).unwrap();
    assert_eq!(updated.dynamic_rules, state.dynamic_rules);
    assert!(updated.session_rules.is_empty());
    assert_eq!(updated.static_rulesets[0].id, "replacement");
    assert!(updated.static_rulesets[0].enabled);
    let before = updated.snapshot().unwrap();
    assert!(updated.restore_after_update(b"{}").is_err());
    assert_eq!(updated.snapshot().unwrap(), before);
}
