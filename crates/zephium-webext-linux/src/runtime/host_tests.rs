use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{json, Value};
use url::Url;
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_runtime::context::{
    Authority, CommittedDocument, DocumentGeneration, ExtensionId, ExtensionScheme, ExtensionView,
    InstallAdmission, Sender, ViewKind,
};
use zephium_webext_runtime::envelope::{ApiName, CallId, Direction, Envelope};
use zephium_webext_runtime::i18n::{LocaleEnvironment, Messages};
use zephium_webext_runtime::lifecycle::{BackgroundState, IDLE_TIMEOUT_NANOS, MAX_CRASHES};
use zephium_webext_runtime::pattern::DocumentOrigin;
use zephium_webext_runtime::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};

use super::*;
use crate::background::BackgroundScript;
use crate::bridge::Outbox;

const ID: &str = "abcdefghijklmnopabcdefghijklmnop";
const SECOND: u64 = 1_000_000_000;

#[test]
fn cookie_authority_requires_live_context_and_settled_current_host_grants() {
    let mut h = Harness::new();
    let mut admission = admitted(h.install);
    admission.manifest = Manifest::from_value(
        json!({"manifest_version":3,"permissions":["cookies"],"host_permissions":["https://example.com/*"]}),
    );
    let (declaration, _) = PermissionDeclaration::from_manifest(&admission.manifest);
    admission.permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    );
    h.host
        .admit(admission, &Activation::Enabled, &mut Outbox::default());
    let (_, sender) = h.sender(ViewKind::Options, "options.html");
    let url = Url::parse("https://example.com/").unwrap();
    assert!(!h.host.cookies_allowed(&sender, Some(&url)));
    h.call(
        &sender,
        "cookies.get",
        json!([{"url":url.as_str(),"name":"x"}]),
    );
    assert!(h.host.cookies_allowed(&sender, Some(&url)));
    assert!(!h
        .host
        .cookies_allowed(&sender, Some(&Url::parse("https://other.test/").unwrap())));
    h.host
        .state
        .installs
        .get_mut(&h.install)
        .unwrap()
        .permission_pending = true;
    assert!(!h.host.cookies_allowed(&sender, Some(&url)));
    let state = h.host.state.installs.get_mut(&h.install).unwrap();
    state.permission_pending = false;
    state.cookie_revision = 1;
    assert_eq!(h.host.cookie_revision(&sender), Some(1));
    h.host
        .close_context(&sender.context_key(), &mut Outbox::default());
    assert!(!h.host.cookies_allowed(&sender, Some(&url)));
}

struct Harness {
    host: RuntimeHost,
    authority: Authority,
    now: Rc<Cell<u64>>,
    install: ExtensionInstallId,
    next_call: u32,
    documents: u64,
    background: Option<(ExtensionView, Sender)>,
}

fn manifest() -> Manifest {
    Manifest::from_value(json!({
        "manifest_version": 3,
        "name": "fixture",
        "version": "1.0",
        "background": {"service_worker": "sw.js"},
        "permissions": ["storage", "alarms", "offscreen", "clipboardWrite"],
        "options_page": "options.html"
    }))
}

fn admitted(install: ExtensionInstallId) -> Admitted {
    let manifest = manifest();
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    let id = ExtensionId::parse(ID).unwrap();
    Admitted {
        install,
        extension_id: id.clone(),
        messages: Messages::new(
            id,
            LocaleEnvironment {
                ui_language: "en-US".into(),
                accept_languages: vec![],
                right_to_left: false,
            },
            None,
            &BTreeMap::new(),
        )
        .unwrap()
        .into(),
        manifest_scripts: ScriptRegistry::from_manifest(&manifest, |_, _| Ok(())).unwrap(),
        manifest,
        root: PathBuf::from("/nonexistent"),
        background: Some(BackgroundScript {
            path: "sw.js".into(),
            module: false,
        }),
        permissions: InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::default(),
            false,
        ),
        unpacked: true,
        state_directory: None,
        listeners: None,
        alarms: None,
        imports: None,
        scripts: Ok(None),
    }
}

impl Harness {
    fn new() -> Self {
        let now = Rc::new(Cell::new(SECOND));
        let clock = now.clone();
        let host = RuntimeHost::new(HostConfig {
            profile: ProfileId::from(1),
            incognito: false,
            ui_language: "en-US".into(),
            clock: Rc::new(move || clock.get()),
            unix_millis: Rc::new(|| 1_700_000_000_000),
            scale: 1,
        })
        .unwrap();
        let mut authority =
            Authority::new(ProfileId::from(1), false, ExtensionScheme::CHROME_EXTENSION);
        let install = ExtensionInstallId::from(9);
        authority
            .admit(InstallAdmission {
                install,
                extension_id: ExtensionId::parse(ID).unwrap(),
                generation: ExtensionRuntimeGeneration::INITIAL,
                allowed_in_incognito: false,
                externally_connectable: None,
                unpacked: true,
            })
            .unwrap();
        Self {
            host,
            authority,
            now,
            install,
            next_call: 0,
            documents: 0,
            background: None,
        }
    }

    fn advance(&self, nanos: u64) {
        self.now.set(self.now.get() + nanos);
    }

    fn sender(&mut self, kind: ViewKind, path: &str) -> (ExtensionView, Sender) {
        let view = self.authority.open_view(self.install, kind).unwrap();
        self.documents += 1;
        let url = Url::parse(&format!("chrome-extension://{ID}/{path}")).unwrap();
        let sender = self
            .authority
            .bind_view(
                &view,
                &CommittedDocument {
                    generation: DocumentGeneration::new(self.documents).unwrap(),
                    origin: DocumentOrigin::of_url(&url),
                    url,
                },
            )
            .unwrap();
        (view, sender)
    }

    fn call(&mut self, sender: &Sender, name: &str, args: Value) -> Outbox {
        self.next_call += 1;
        let args = serde_json::to_vec(&args).unwrap();
        let frame = Envelope::Call {
            id: CallId::new(self.next_call).unwrap(),
            name: ApiName::parse(name).unwrap(),
            args: &args,
        }
        .encode(Direction::FromContext, u64::MAX)
        .unwrap();
        let mut out = Outbox::default();
        self.host.receive(sender, &frame, &mut out);
        out
    }

    fn ack(&mut self, sender: &Sender, seq: EventSeq) -> Outbox {
        let frame = Envelope::EventAck { seq }
            .encode(Direction::FromContext, u64::MAX)
            .unwrap();
        let mut out = Outbox::default();
        self.host.receive(sender, &frame, &mut out);
        out
    }

    fn tick(&mut self) -> Outbox {
        let mut out = Outbox::default();
        self.host.tick(&mut out);
        out
    }

    /// Native work other than content-script republication.
    fn ops(&mut self) -> Vec<NativeOp> {
        let mut ops = self.host.take_ops();
        ops.retain(|op| !matches!(op, NativeOp::ScriptsChanged));
        ops
    }

    fn state(&self) -> Option<BackgroundState> {
        self.host.status(self.install).map(|status| status.state)
    }

    /// Admits the install and plays a background through activation with
    /// the given listeners. Returns its sender and generation.
    fn running(&mut self, listeners: &[&str]) -> (Sender, u64, Outbox) {
        let mut out = Outbox::default();
        self.host
            .admit(admitted(self.install), &Activation::Installed, &mut out);
        self.boot(listeners)
    }

    fn boot(&mut self, listeners: &[&str]) -> (Sender, u64, Outbox) {
        if let Some((view, sender)) = self.background.take() {
            let mut out = Outbox::default();
            self.host
                .context_closed(&sender.context_key(), CloseReason::Detached, &mut out);
            self.authority.close_view(view);
        }
        let generation = self
            .ops()
            .iter()
            .find_map(|op| match op {
                NativeOp::StartBackground { generation, .. } => Some(*generation),
                _ => None,
            })
            .expect("a start");
        let (view, sender) = self.sender(ViewKind::Background, "_zephium/background.html");
        self.call(&sender, "zephium.workerReport", json!(["booted", null]));
        for (index, event) in listeners.iter().enumerate() {
            let out = self.call(
                &sender,
                "zephium.addListener",
                json!([event, index + 1, null, null]),
            );
            assert_eq!(replies(&out), vec![Ok(Value::Null)], "{event}");
        }
        let out = self.call(
            &sender,
            "zephium.workerLifecycle",
            json!(["activated", null]),
        );
        self.background = Some((view, sender.clone()));
        (sender, generation, out)
    }
}

fn decoded(out: &Outbox) -> Vec<(ContextKey, Envelope<'_>)> {
    out.frames
        .iter()
        .map(|(context, frame)| {
            (
                context.clone(),
                Envelope::decode(frame, Direction::ToContext, u64::MAX).unwrap(),
            )
        })
        .collect()
}

fn replies(out: &Outbox) -> Vec<Result<Value, String>> {
    decoded(out)
        .into_iter()
        .filter_map(|(_, envelope)| match envelope {
            Envelope::Reply { result, .. } => Some(
                result
                    .map(|bytes| serde_json::from_slice(bytes).unwrap())
                    .map_err(str::to_owned),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn management_reports_the_admitted_install_provenance() {
    let mut h = Harness::new();
    let (sender, _, _) = h.running(&[]);
    for (unpacked, expected) in [(true, "development"), (false, "normal")] {
        h.host.state.installs.get_mut(&h.install).unwrap().unpacked = unpacked;
        let out = h.call(&sender, "management.getSelf", json!([]));
        let values = replies(&out);
        let value = values[0].as_ref().unwrap();
        assert_eq!(value["id"], ID);
        assert_eq!(value["installType"], expected);
        assert_eq!(value["enabled"], true);
        assert_eq!(value["mayDisable"], true);
        assert_eq!(value["isApp"], false);
    }
    h.host.state.installs.remove(&h.install);
    assert!(h.host.state.extension_info(&sender).is_err());
}

#[test]
fn sessions_restore_refuses_before_queuing_a_browser_mutation() {
    let mut h = Harness::new();
    let mut install = admitted(h.install);
    let manifest = Manifest::from_value(json!({
        "manifest_version": 3, "name": "sessions", "version": "1.0",
        "permissions": ["sessions"], "background": {"service_worker": "sw.js"}
    }));
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    install.manifest = manifest;
    install.permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::default(),
        false,
    );
    h.host
        .admit(install, &Activation::Installed, &mut Outbox::default());
    let (sender, _, _) = h.boot(&[]);
    h.host.browser.take_outgoing();
    for args in [json!([]), json!(["closed-tab"])] {
        let out = h.call(&sender, "sessions.restore", args);
        assert_eq!(
            replies(&out),
            vec![Err("This function is not supported.".into())]
        );
        assert!(h.host.browser.take_outgoing().is_empty());
    }
}

#[test]
fn visible_capture_refuses_without_queuing_engine_work() {
    use crate::bridge::BridgeViewId;
    use zephium_core::extensions::{
        ExtensionBrowserSurface, ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab,
        ExtensionBrowserWindow,
    };
    use zephium_webext_runtime::context::FrameId;
    let mut h = Harness::new();
    let mut install = admitted(h.install);
    let manifest = Manifest::from_value(json!({
        "manifest_version": 3, "name": "capture", "version": "1.0",
        "host_permissions": ["<all_urls>"], "background": {"service_worker": "sw.js"}
    }));
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    install.manifest = manifest;
    install.permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    );
    h.host
        .admit(install, &Activation::Installed, &mut Outbox::default());
    let (sender, _, _) = h.boot(&[]);
    let url = Url::parse("https://capture.test/").unwrap();
    let item = ItemId::from(10);
    let tab =
        ExtensionBrowserTab::from_snapshot(None, item, true, "capture", Some(&url), false, false)
            .unwrap();
    let window = ExtensionBrowserWindow::new(7, false, Some(item), vec![tab]).unwrap();
    h.host.browser.publish_surface(
        ExtensionBrowserSurface::new(
            ProfileId::from(1),
            ExtensionBrowserSurfaceGeneration::new(1).unwrap(),
            Some(7),
            vec![window],
        )
        .unwrap(),
    );
    h.host.frame_lifecycle(
        &FrameLifecycle::DocumentCreated {
            view: BridgeViewId(99),
            owner: ViewRef::Tab(item),
            frame: FrameId::MAIN,
            parent: None,
            opaque: false,
            creator: None,
            document: DocumentGeneration::new(42).unwrap(),
            url,
        },
        &mut Outbox::default(),
    );
    h.host.browser.take_outgoing();
    let window_id = replies(&h.call(&sender, "windows.getCurrent", json!([])))[0]
        .as_ref()
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    for args in [
        json!([]),
        json!([window_id, {"format":"png"}]),
        json!([window_id, {"format":"jpeg","quality":80}]),
    ] {
        let out = h.call(&sender, "tabs.captureVisibleTab", args);
        assert_eq!(
            replies(&out),
            vec![Err("This function is not supported.".into())]
        );
        assert!(h.host.browser.take_outgoing().is_empty());
    }
    let out = h.call(
        &sender,
        "tabs.captureVisibleTab",
        json!([window_id, {"format":"invalid"}]),
    );
    let values = replies(&out);
    assert!(values[0].is_err());
    assert_ne!(values[0], Err("This function is not supported.".into()));
    assert!(h.host.browser.take_outgoing().is_empty());
}

fn events(out: &Outbox) -> Vec<(EventSeq, String, Value)> {
    decoded(out)
        .into_iter()
        .filter_map(|(_, envelope)| match envelope {
            Envelope::Event { seq, name, args } => Some((
                seq,
                name.as_str().to_owned(),
                serde_json::from_slice(args).unwrap(),
            )),
            _ => None,
        })
        .collect()
}

fn stops(ops: &[NativeOp]) -> Vec<u64> {
    ops.iter()
        .filter_map(|op| match op {
            NativeOp::StopBackground { generation, .. } => Some(*generation),
            _ => None,
        })
        .collect()
}

#[test]
fn a_fresh_install_delivers_on_installed_before_activation_and_waits_for_its_ack() {
    let mut harness = Harness::new();
    let mut out = Outbox::default();
    harness
        .host
        .admit(admitted(harness.install), &Activation::Installed, &mut out);
    assert!(events(&out).is_empty());
    harness.ops();
    let (view, stale) = harness.sender(ViewKind::Background, "_zephium/background.html");
    harness.documents += 1;
    let sender = harness
        .authority
        .bind_view(
            &view,
            &CommittedDocument {
                generation: DocumentGeneration::new(harness.documents).unwrap(),
                origin: DocumentOrigin::of_url(stale.url()),
                url: stale.url().clone(),
            },
        )
        .unwrap();
    harness.call(&sender, "zephium.workerReport", json!(["booted", null]));
    harness.call(
        &sender,
        "zephium.addListener",
        json!(["runtime.onInstalled", 1, null, null]),
    );
    assert!(harness.host.enqueue(
        harness.install,
        "runtime.onStartup",
        &json!([]),
        EventImportance::Critical,
    ));
    let out = harness.call(
        &sender,
        "zephium.workerLifecycle",
        json!(["installed", null]),
    );
    let delivered = events(&out);
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    let (seq, name, args) = &delivered[0];
    assert_eq!(name, "runtime.onInstalled");
    assert_eq!(args, &json!([{"reason": "install"}]));
    assert!(
        replies(&out).is_empty(),
        "installation awaits the event ack"
    );
    assert_eq!(harness.state(), Some(BackgroundState::Starting));
    assert!(!harness.host.status(harness.install).unwrap().ready);
    let duplicate = harness.call(
        &sender,
        "zephium.workerLifecycle",
        json!(["installed", null]),
    );
    assert!(replies(&duplicate)[0].is_err());
    assert!(replies(&harness.ack(&stale, *seq)).is_empty());
    let acknowledged = harness.ack(&sender, *seq);
    assert_eq!(replies(&acknowledged), vec![Ok(Value::Null)]);
    assert!(
        events(&acknowledged).is_empty(),
        "ordinary events still wait"
    );
    assert!(!harness.host.status(harness.install).unwrap().ready);
    let activated = harness.call(
        &sender,
        "zephium.workerLifecycle",
        json!(["activated", null]),
    );
    let status = harness.host.status(harness.install).unwrap();
    assert!(status.ready);
    assert_eq!(status.state, BackgroundState::Running);
    assert_eq!(events(&activated)[0].1, "runtime.onStartup");
    assert!(!harness.host.state.installs[&harness.install].fresh);
}

#[test]
fn the_worker_bootstrap_marks_only_fresh_versions_as_installing() {
    let mut harness = Harness::new();
    let mut out = Outbox::default();
    harness
        .host
        .admit(admitted(harness.install), &Activation::Enabled, &mut out);
    assert!(harness.ops().is_empty(), "an enabled install starts lazily");
    assert_eq!(harness.state(), Some(BackgroundState::Stopped));
}

#[test]
fn idle_termination_then_a_message_wakes_the_background_and_is_answered() {
    let mut harness = Harness::new();
    let (background, first, out) = harness.running(&["runtime.onInstalled", "runtime.onMessage"]);
    let (seq, ..) = events(&out)[0].clone();
    harness.ack(&background, seq);
    harness.advance(IDLE_TIMEOUT_NANOS - 1);
    harness.tick();
    assert!(stops(&harness.ops()).is_empty());
    harness.advance(1);
    harness.tick();
    assert_eq!(stops(&harness.ops()), vec![first]);
    assert_eq!(harness.state(), Some(BackgroundState::Terminating));
    let mut out = Outbox::default();
    harness
        .host
        .background_stopped(harness.install, first, &mut out);
    assert_eq!(harness.state(), Some(BackgroundState::Stopped));

    let (_options, options) = harness.sender(ViewKind::Options, "options.html");
    let out = harness.call(
        &options,
        "runtime.sendMessage",
        json!([null, "{\"type\":\"probe\"}", null]),
    );
    assert!(
        replies(&out).is_empty(),
        "the request waits for the background"
    );
    let (background, second, out) = harness.boot(&["runtime.onMessage"]);
    assert!(second > first);
    let delivered = events(&out);
    let (seq, name, args) = delivered
        .iter()
        .find(|(_, name, _)| name == "runtime.onMessage")
        .cloned()
        .expect("the parked message");
    assert_eq!(name, "runtime.onMessage");
    assert_eq!(args[0], "{\"type\":\"probe\"}");
    assert!(args[1]["url"].as_str().unwrap().ends_with("/options.html"));
    let token = args[2].as_str().unwrap().to_owned();
    harness.ack(&background, seq);
    let out = harness.call(
        &background,
        "zephium.respond",
        json!([token, args[3][0], {"response": "{\"ok\":true}"}]),
    );
    let answers = replies(&out);
    assert!(
        answers.contains(&Ok(json!({"response": "{\"ok\":true}"}))),
        "{answers:?}"
    );
}

#[test]
fn a_message_with_no_receiver_fails_like_chrome() {
    let mut harness = Harness::new();
    let (background, _, _) = harness.running(&["runtime.onInstalled"]);
    let out = harness.call(&background, "runtime.sendMessage", json!([null, "1", null]));
    assert_eq!(
        replies(&out),
        vec![Err(
            "Could not establish connection. Receiving end does not exist.".to_owned()
        )]
    );
}

#[test]
fn worker_self_messages_wait_for_first_pass_and_exclude_its_own_listener() {
    let mut harness = Harness::new();
    let mut out = Outbox::default();
    harness
        .host
        .admit(admitted(harness.install), &Activation::Installed, &mut out);
    harness.ops();
    let (_view, sender) = harness.sender(ViewKind::Background, "_zephium/background.html");
    harness.call(&sender, "zephium.workerReport", json!(["booted", null]));
    assert!(
        replies(&harness.call(&sender, "runtime.sendMessage", json!([null, "1", null]))).is_empty()
    );
    harness.call(
        &sender,
        "zephium.addListener",
        json!(["runtime.onMessage", 1, null, null]),
    );
    assert!(
        replies(&harness.call(&sender, "runtime.sendMessage", json!([null, "2", null]))).is_empty()
    );
    let ready = harness.call(
        &sender,
        "zephium.workerLifecycle",
        json!(["scriptReady", null]),
    );
    let errors: Vec<_> = replies(&ready)
        .into_iter()
        .filter_map(Result::err)
        .collect();
    assert_eq!(
        errors,
        vec!["Could not establish connection. Receiving end does not exist."; 2]
    );
    assert!(events(&ready).is_empty());
    assert_eq!(harness.state(), Some(BackgroundState::Starting));
    assert!(!harness.host.status(harness.install).unwrap().ready);
    let during_install = harness.call(&sender, "runtime.sendMessage", json!([null, "3", null]));
    assert_eq!(
        replies(&during_install),
        vec![Err(
            "Could not establish connection. Receiving end does not exist.".to_owned()
        )]
    );
}

#[test]
fn failed_first_pass_settles_parked_self_message_and_drops_its_authority() {
    let mut harness = Harness::new();
    let mut out = Outbox::default();
    harness
        .host
        .admit(admitted(harness.install), &Activation::Installed, &mut out);
    harness.ops();
    let (_view, sender) = harness.sender(ViewKind::Background, "_zephium/background.html");
    harness.call(&sender, "zephium.workerReport", json!(["booted", null]));
    assert!(
        replies(&harness.call(&sender, "runtime.sendMessage", json!([null, "1", null]))).is_empty()
    );
    let failed = harness.call(
        &sender,
        "zephium.workerLifecycle",
        json!(["failed", "script"]),
    );
    assert!(replies(&failed).iter().any(Result::is_err));
    assert!(harness.host.state.installs[&harness.install]
        .parked
        .is_empty());
    let stale = harness.call(
        &sender,
        "zephium.workerLifecycle",
        json!(["scriptReady", null]),
    );
    assert!(events(&stale).is_empty());
    assert!(!harness.host.state.installs[&harness.install]
        .live
        .as_ref()
        .is_some_and(|live| live.script_ready));
}

#[test]
fn pending_events_and_calls_block_idling_but_loaded_offscreen_documents_do_not() {
    let mut harness = Harness::new();
    let (background, _, out) = harness.running(&["runtime.onInstalled"]);
    let (seq, ..) = events(&out)[0].clone();
    harness.advance(IDLE_TIMEOUT_NANOS * 2);
    harness.tick();
    assert!(
        stops(&harness.ops()).is_empty(),
        "an unacknowledged event blocks idling"
    );
    harness.ack(&background, seq);
    let out = harness.call(
        &background,
        "offscreen.createDocument",
        json!([{"url": "off.html", "reasons": ["CLIPBOARD"], "justification": "copy"}]),
    );
    assert!(
        replies(&out).is_empty(),
        "creation settles when the document loads"
    );
    let ops = harness.ops();
    assert!(ops.iter().any(|op| matches!(
        op,
        NativeOp::CreateOffscreen { clipboard: false, url, .. } if url.ends_with("/off.html")
    )));
    harness.advance(IDLE_TIMEOUT_NANOS * 2);
    harness.tick();
    assert!(
        stops(&harness.ops()).is_empty(),
        "pending creation blocks idling"
    );
    let mut out = Outbox::default();
    harness
        .host
        .offscreen_loaded(harness.install, true, &mut out);
    assert_eq!(replies(&out), vec![Ok(Value::Null)]);
    harness.advance(IDLE_TIMEOUT_NANOS * 2);
    harness.tick();
    let ops = harness.ops();
    assert_eq!(stops(&ops).len(), 1);
    assert!(!ops
        .iter()
        .any(|op| matches!(op, NativeOp::CloseOffscreen { .. })));
    assert!(harness.host.has_offscreen(harness.install));
    let (_, popup) = harness.sender(ViewKind::Popup, "popup.html");
    let out = harness.call(&popup, "offscreen.closeDocument", json!([]));
    assert_eq!(replies(&out), vec![Ok(Value::Null)]);
    assert!(harness
        .ops()
        .iter()
        .any(|op| matches!(op, NativeOp::CloseOffscreen { .. })));
}

#[test]
fn crashes_restart_with_backoff_and_a_crash_loop_disables() {
    let mut harness = Harness::new();
    let (background, mut generation, out) =
        harness.running(&["runtime.onInstalled", "runtime.onMessage"]);
    let (seq, ..) = events(&out)[0].clone();
    harness.ack(&background, seq);
    let (_options, options) = harness.sender(ViewKind::Options, "options.html");
    for crash in 0..MAX_CRASHES {
        let mut out = Outbox::default();
        harness
            .host
            .background_crashed(harness.install, generation, &mut out);
        let ops = harness.ops();
        assert!(ops
            .iter()
            .any(|op| matches!(op, NativeOp::DiscardBackground { crashed: true, .. })));
        if crash + 1 == MAX_CRASHES {
            assert!(ops
                .iter()
                .any(|op| matches!(op, NativeOp::Notify(Notice::Disabled(_)))));
            assert_eq!(harness.state(), Some(BackgroundState::Disabled));
            let out = harness.call(&options, "runtime.sendMessage", json!([null, "1", null]));
            assert_eq!(replies(&out).len(), 1, "no wait for a disabled background");
            break;
        }
        assert_eq!(harness.state(), Some(BackgroundState::Crashed));
        harness.call(&options, "runtime.sendMessage", json!([null, "1", null]));
        assert!(
            harness.ops().is_empty(),
            "the restart waits for its backoff"
        );
        harness.advance(30 * SECOND);
        harness.tick();
        let (_sender, next, _) = harness.boot(&["runtime.onMessage"]);
        assert!(next > generation);
        generation = next;
    }
    let kinds: Vec<_> = harness
        .host
        .errors(harness.install)
        .into_iter()
        .map(|error| error.kind)
        .collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == ExtensionErrorKind::Crash)
            .count(),
        MAX_CRASHES
    );
    assert!(kinds.contains(&ExtensionErrorKind::Disabled));
}

#[test]
fn startup_failures_are_errors_and_stop_the_worker() {
    let mut harness = Harness::new();
    let mut out = Outbox::default();
    harness
        .host
        .admit(admitted(harness.install), &Activation::Installed, &mut out);
    harness.ops();
    let (_view, background) = harness.sender(ViewKind::Background, "_zephium/background.html");
    harness.call(
        &background,
        "zephium.workerError",
        json!([{"message": "SyntaxError: Unexpected token", "filename": "chrome-extension://x/sw.js", "lineno": 3, "colno": 7}]),
    );
    assert_eq!(harness.state(), Some(BackgroundState::Terminating));
    let errors = harness.host.errors(harness.install);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind, ExtensionErrorKind::Startup);
    assert_eq!(errors[0].line, 3);
    assert_eq!(errors[0].column, 7);
    assert_eq!(errors[0].extension_id, ID);
    let ops = harness.ops();
    assert_eq!(stops(&ops).len(), 1, "a live failed worker is stopped");
    let mut out = Outbox::default();
    harness
        .host
        .background_stopped(harness.install, stops(&ops)[0], &mut out);
    assert_eq!(
        harness.state(),
        Some(BackgroundState::Crashed),
        "and counted like a crash"
    );
}

#[test]
fn worker_reports_after_boot_are_recorded_bounded_and_rate_limited() {
    let mut harness = Harness::new();
    let (background, ..) = harness.running(&[]);
    for index in 0..200 {
        harness.call(
            &background,
            "zephium.workerReport",
            json!(["console", format!("{index}{}", "x".repeat(4096))]),
        );
    }
    let errors = harness.host.errors(harness.install);
    assert!(!errors.is_empty() && errors.len() < 64, "{}", errors.len());
    assert!(errors
        .iter()
        .all(|error| error.kind == ExtensionErrorKind::Console
            && error.message.len() <= MAX_ERROR_MESSAGE_BYTES));
    assert_eq!(harness.state(), Some(BackgroundState::Running));
}

#[test]
fn unimplemented_methods_and_events_are_counted() {
    let mut harness = Harness::new();
    let (background, ..) = harness.running(&[]);
    harness.call(&background, "windows.create", json!([{}]));
    harness.call(&background, "windows.create", json!([{}]));
    let out = harness.call(
        &background,
        "zephium.addListener",
        json!(["contextMenus.onClicked", 1, null, null]),
    );
    assert!(replies(&out)[0].is_err());
    let unhandled = harness.host.unhandled(harness.install);
    assert_eq!(unhandled.get("windows.create"), Some(&2));
    assert_eq!(unhandled.get("contextMenus.onClicked"), Some(&1));
}

#[test]
fn alarms_wake_a_sleeping_background_with_a_listener() {
    let mut harness = Harness::new();
    let (background, first, out) = harness.running(&["runtime.onInstalled", "alarms.onAlarm"]);
    let (seq, ..) = events(&out)[0].clone();
    harness.ack(&background, seq);
    let out = harness.call(
        &background,
        "alarms.create",
        json!(["wake", {"delayInMinutes": 1}]),
    );
    assert_eq!(replies(&out), vec![Ok(Value::Null)]);
    harness.advance(IDLE_TIMEOUT_NANOS);
    harness.tick();
    assert_eq!(stops(&harness.ops()), vec![first]);
    let mut out = Outbox::default();
    harness
        .host
        .background_stopped(harness.install, first, &mut out);
    assert!(harness.host.next_deadline().is_some());
    harness.advance(30 * SECOND);
    harness.tick();
    let (_background, second, out) = harness.boot(&["alarms.onAlarm"]);
    assert!(second > first);
    let delivered = events(&out);
    assert_eq!(delivered[0].1, "alarms.onAlarm");
    assert_eq!(delivered[0].2[0]["name"], "wake");
}

#[test]
fn updates_deliver_the_previous_version_and_shutdown_stops_everything() {
    let mut harness = Harness::new();
    let mut out = Outbox::default();
    harness.host.admit(
        admitted(harness.install),
        &Activation::Updated {
            previous_version: "0.9".into(),
        },
        &mut out,
    );
    let (_background, generation, out) = harness.boot(&["runtime.onInstalled"]);
    assert_eq!(
        events(&out)[0].2,
        json!([{"reason": "update", "previousVersion": "0.9"}])
    );
    let mut out = Outbox::default();
    harness.host.shutdown(&mut out);
    assert_eq!(stops(&harness.ops()), vec![generation]);
}

#[test]
fn the_binding_catalog_applies_named_placeholders_only() {
    let messages = Messages::new(
        ExtensionId::parse(ID).unwrap(),
        LocaleEnvironment {
            ui_language: "en".into(),
            accept_languages: vec![],
            right_to_left: false,
        },
        Some("en"),
        &BTreeMap::from([(
            "en".to_owned(),
            json!({"Greeting": {"message": "Hi $Who$, $1 $$", "placeholders": {"who": {"content": "you"}}}}),
        )]),
    )
    .unwrap();
    assert_eq!(
        binding_catalog(&messages),
        json!({"greeting": "Hi you, $1 $$"})
    );
}

#[test]
fn script_snapshot_read_errors_surface_without_filesystem_details() {
    for kind in [
        std::io::ErrorKind::PermissionDenied,
        std::io::ErrorKind::InvalidData,
    ] {
        let mut harness = Harness::new();
        let mut admission = admitted(harness.install);
        admission.scripts = Err(std::io::Error::new(kind, "private filesystem details"));
        harness
            .host
            .admit(admission, &Activation::Enabled, &mut Outbox::default());
        let errors = harness.host.errors(harness.install);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind, ExtensionErrorKind::Startup);
        assert_eq!(
            errors[0].message,
            "Persisted script registrations could not be restored."
        );
    }
}

#[test]
fn dynamic_scripts_commit_before_reply_and_restore_only_persistent_entries() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("content.js"), "void 0;").unwrap();
    let durable = tempfile::tempdir().unwrap();
    let mut harness = Harness::new();
    let mut admission = admitted(harness.install);
    admission.root = root.path().to_owned();
    admission.state_directory = Some(durable.path().to_owned());
    let manifest = Manifest::from_value(json!({"manifest_version":3,"permissions":["scripting"]}));
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    admission.permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::default(),
        false,
    );
    harness
        .host
        .admit(admission, &Activation::Enabled, &mut Outbox::default());
    let (_, sender) = harness.sender(ViewKind::Options, "options.html");
    let request = |id: &str, persist| json!({"id":id,"matches":["https://example.com/*"],"js":["content.js"],"persistAcrossSessions":persist});
    let first = harness.call(
        &sender,
        "scripting.registerContentScripts",
        json!([[request("saved", true), request("temporary", false)]]),
    );
    assert!(replies(&first).is_empty());
    let second = harness.call(
        &sender,
        "scripting.registerContentScripts",
        json!([[request("second", true)]]),
    );
    assert!(replies(&second).is_empty());
    let _ = harness.host.take_ops();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut out = Outbox::default();
    while replies(&out).len() < 2 {
        assert!(std::time::Instant::now() < deadline);
        let done = harness.host.storage_take();
        harness.host.storage_done(done, &mut out);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(replies(&out), vec![Ok(Value::Null), Ok(Value::Null)]);
    assert!(
        harness
            .host
            .take_ops()
            .iter()
            .any(|op| matches!(op, NativeOp::ScriptsChanged)),
        "committed registrations reach future documents"
    );
    let bytes = std::fs::read(durable.path().join("scripts.json")).unwrap();
    let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(snapshot.as_array().unwrap().len(), 2);
    assert_eq!(
        harness.host.state.installs[&harness.install]
            .scripts
            .get_registered(None)
            .len(),
        3
    );
    harness
        .host
        .forget(harness.install, false, &mut Outbox::default());
    let mut admission = admitted(harness.install);
    admission.root = root.path().to_owned();
    admission.scripts = Ok(Some(bytes));
    harness
        .host
        .admit(admission, &Activation::Enabled, &mut Outbox::default());
    let restored = harness.host.state.installs[&harness.install]
        .scripts
        .get_registered(None);
    assert_eq!(
        restored.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["saved", "second"]
    );
    let state = harness
        .host
        .state
        .installs
        .get_mut(&harness.install)
        .unwrap();
    state.state_directory = Some(durable.path().to_owned());
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    harness.host.permissions.insert(
        harness.install,
        InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::default(),
            false,
        ),
    );
    let (_, sender) = harness.sender(ViewKind::Options, "options.html");
    let update = harness.call(
        &sender,
        "scripting.updateContentScripts",
        json!([[{"id":"saved", "persistAcrossSessions":false}]]),
    );
    assert!(replies(&update).is_empty());
    let remove = harness.call(
        &sender,
        "scripting.unregisterContentScripts",
        json!([{"ids":["second"]}]),
    );
    assert!(replies(&remove).is_empty());
    let mut out = Outbox::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while replies(&out).len() < 2 {
        assert!(std::time::Instant::now() < deadline);
        let done = harness.host.storage_take();
        harness.host.storage_done(done, &mut out);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(replies(&out), vec![Ok(Value::Null), Ok(Value::Null)]);
    let bytes = std::fs::read(durable.path().join("scripts.json")).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), json!([]));
}

#[test]
fn dynamic_scripts_failed_write_keeps_registry_and_malformed_restore_is_atomic() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("content.js"), "void 0;").unwrap();
    let mut harness = Harness::new();
    let mut admission = admitted(harness.install);
    admission.root = root.path().to_owned();
    admission.scripts = Ok(Some(br#"[{"id":"bad","matches":["invalid"]}]"#.to_vec()));
    harness
        .host
        .admit(admission, &Activation::Enabled, &mut Outbox::default());
    let (_, sender) = harness.sender(ViewKind::Options, "options.html");
    let state = harness
        .host
        .state
        .installs
        .get_mut(&harness.install)
        .unwrap();
    assert!(state.scripts.get_registered(None).is_empty());
    assert_eq!(
        state.errors.records()[0].message,
        "Persisted script registrations could not be restored."
    );
    let blocked = root.path().join("not-a-directory");
    std::fs::write(&blocked, "occupied").unwrap();
    state.state_directory = Some(blocked);
    let mut candidate = state.scripts.clone();
    candidate
        .register(
            serde_json::from_value(
                json!([{"id":"saved","matches":["https://example.com/*"],"js":["content.js"]}]),
            )
            .unwrap(),
            |_, _| Ok(()),
        )
        .unwrap();
    // Dispatch through the ordinary authenticated API to exercise deferred failure.
    let manifest = Manifest::from_value(json!({"manifest_version":3,"permissions":["scripting"]}));
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    harness.host.permissions.insert(
        harness.install,
        InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::default(),
            false,
        ),
    );
    let out = harness.call(
        &sender,
        "scripting.registerContentScripts",
        json!([candidate.get_registered(None)]),
    );
    assert!(replies(&out).is_empty());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut out = Outbox::default();
    while replies(&out).is_empty() {
        assert!(std::time::Instant::now() < deadline);
        let done = harness.host.storage_take();
        harness.host.storage_done(done, &mut out);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(replies(&out)[0].is_err());
    assert!(harness.host.state.installs[&harness.install]
        .scripts
        .get_registered(None)
        .is_empty());
}

fn admit_scripting(harness: &mut Harness, root: &std::path::Path) -> Sender {
    let mut admission = admitted(harness.install);
    admission.root = root.to_owned();
    let manifest =
        Manifest::from_value(json!({"manifest_version": 3, "permissions": ["scripting"]}));
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    admission.permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::default(),
        false,
    );
    harness
        .host
        .admit(admission, &Activation::Enabled, &mut Outbox::default());
    harness.sender(ViewKind::Options, "options.html").1
}

#[test]
fn closed_context_releases_queued_script_calls_before_the_durable_write_finishes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("content.js"), "void 0;").unwrap();
    let durable = tempfile::tempdir().unwrap();
    let mut harness = Harness::new();
    let sender = admit_scripting(&mut harness, root.path());
    harness
        .host
        .state
        .installs
        .get_mut(&harness.install)
        .unwrap()
        .state_directory = Some(durable.path().to_owned());
    let script = |id: &str| {
        json!([[
            {"id": id, "matches": ["https://example.com/*"], "js": ["content.js"]}
        ]])
    };
    for id in ["submitted", "queued"] {
        let out = harness.call(&sender, "scripting.registerContentScripts", script(id));
        assert!(replies(&out).is_empty());
    }
    let state = &harness.host.state.installs[&harness.install];
    assert!(state.script_pending.is_some());
    assert_eq!(state.script_calls.len(), 1);
    harness
        .host
        .close_context(&sender.context_key(), &mut Outbox::default());
    let state = &harness.host.state.installs[&harness.install];
    assert!(
        state.script_pending.is_some(),
        "submitted disk writes keep their serialization slot"
    );
    assert!(
        state.script_calls.is_empty(),
        "cancellation releases queued arguments and authority"
    );
    let mut out = Outbox::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while harness.host.has_pending_script_writes(harness.install) {
        assert!(std::time::Instant::now() < deadline);
        let done = harness.host.storage_take();
        harness.host.storage_done(done, &mut out);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        replies(&out).is_empty(),
        "the closed context receives no reply"
    );
    assert_eq!(
        harness.host.state.installs[&harness.install]
            .scripts
            .get_registered(None)
            .iter()
            .map(|script| script.id.as_str())
            .collect::<Vec<_>>(),
        vec!["submitted"]
    );
}

#[test]
fn script_write_failure_before_rename_preserves_the_existing_snapshot() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("content.js"), "void 0;").unwrap();
    let durable = tempfile::tempdir().unwrap();
    let snapshot = durable.path().join("scripts.json");
    std::fs::write(&snapshot, b"[]").unwrap();
    let mut harness = Harness::new();
    let sender = admit_scripting(&mut harness, root.path());
    harness
        .host
        .state
        .installs
        .get_mut(&harness.install)
        .unwrap()
        .state_directory = Some(durable.path().to_owned());
    std::fs::set_permissions(durable.path(), std::fs::Permissions::from_mode(0o300)).unwrap();
    let out = harness.call(
        &sender,
        "scripting.registerContentScripts",
        json!([[
            {"id": "rejected", "matches": ["https://example.com/*"], "js": ["content.js"]}
        ]]),
    );
    assert!(replies(&out).is_empty());
    let mut out = Outbox::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while replies(&out).is_empty() && std::time::Instant::now() < deadline {
        let done = harness.host.storage_take();
        harness.host.storage_done(done, &mut out);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    std::fs::set_permissions(durable.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(replies(&out).len(), 1, "write must settle");
    assert!(replies(&out)[0].is_err());
    assert_eq!(std::fs::read(snapshot).unwrap(), b"[]");
    assert!(harness.host.state.installs[&harness.install]
        .scripts
        .get_registered(None)
        .is_empty());
}

#[test]
fn retired_installs_keep_the_script_write_barrier_until_io_completion() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("content.js"), "void 0;").unwrap();
    let durable = tempfile::tempdir().unwrap();
    let mut harness = Harness::new();
    let sender = admit_scripting(&mut harness, root.path());
    harness
        .host
        .state
        .installs
        .get_mut(&harness.install)
        .unwrap()
        .state_directory = Some(durable.path().to_owned());
    let out = harness.call(
        &sender,
        "scripting.registerContentScripts",
        json!([[
            {"id": "submitted", "matches": ["https://example.com/*"], "js": ["content.js"]}
        ]]),
    );
    assert!(replies(&out).is_empty());
    assert!(harness.host.has_pending_script_writes(harness.install));
    let mut out = Outbox::default();
    harness.host.forget(harness.install, false, &mut out);
    assert!(harness.host.has_pending_script_writes(harness.install));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while harness.host.has_pending_script_writes(harness.install) {
        assert!(std::time::Instant::now() < deadline);
        let done = harness.host.storage_take();
        harness.host.storage_done(done, &mut out);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(replies(&out).is_empty());
    assert!(!harness.host.state.installs.contains_key(&harness.install));
    let snapshot: Value =
        serde_json::from_slice(&std::fs::read(durable.path().join("scripts.json")).unwrap())
            .unwrap();
    assert_eq!(snapshot[0]["id"], "submitted");
}

#[test]
fn extensions_see_their_manifest_strings_localized() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("manifest.json"),
        r#"{"manifest_version":3,"version":"1","default_locale":"en","name":"__MSG_name__",
            "description":"__MSG_description__ (__MSG_unknown__)","action":{"default_title":"__MSG_title__"},
            "commands":{"run":{"description":"__MSG_title__"}},"content_scripts":[{"js":["__MSG_name__.js"],"matches":["<all_urls>"]}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(directory.path().join("_locales/en")).unwrap();
    std::fs::write(
        directory.path().join("_locales/en/messages.json"),
        r#"{"name":{"message":"Named"},"description":{"message":"Described"},"title":{"message":"Titled"}}"#,
    )
    .unwrap();
    let manifest = localized_manifest(&Manifest::load(directory.path()).unwrap());
    assert_eq!(manifest["name"], "Named");
    assert_eq!(manifest["description"], "Described (__MSG_unknown__)");
    assert_eq!(manifest["action"]["default_title"], "Titled");
    assert_eq!(manifest["commands"]["run"]["description"], "Titled");
    assert_eq!(manifest["content_scripts"][0]["js"][0], "__MSG_name__.js");
}

#[test]
fn removal_waits_for_network_io_and_storage_erasure_before_reactivation() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("install");
    std::fs::create_dir(&state_path).unwrap();
    std::fs::write(
        state_path.join("dnr.json"),
        b"retained until network retirement",
    )
    .unwrap();
    let mut harness = Harness::new();
    let mut admission = admitted(harness.install);
    admission.state_directory = Some(state_path.clone());
    harness
        .host
        .admit(admission, &Activation::Enabled, &mut Outbox::default());
    harness
        .host
        .forget(harness.install, true, &mut Outbox::default());
    assert!(harness.host.has_pending_script_writes(harness.install));
    assert!(state_path.join("dnr.json").exists());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while harness.host.has_pending_script_writes(harness.install) {
        assert!(std::time::Instant::now() < deadline);
        for done in harness.host.network_take() {
            if done.prepared {
                assert!(harness.host.network_finish(done.revision, true));
            } else {
                harness.host.network_done(done, &mut Outbox::default());
            }
        }
        let done = harness.host.storage_take();
        harness.host.storage_done(done, &mut Outbox::default());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!state_path.exists());
}

#[cfg(feature = "native-tests")]
#[test]
fn diagnostic_calls_do_not_extend_worker_lifetime_and_unknown_calls_keep_only_shapes() {
    let directory = tempfile::tempdir().unwrap();
    let path = crate::diagnostics::begin(directory.path(), ID).unwrap();
    let mut harness = Harness::new();
    let (background, generation, out) = harness.running(&["runtime.onInstalled"]);
    let (seq, ..) = events(&out)[0].clone();
    harness.ack(&background, seq);
    let unknown = harness.call(
        &background,
        "runtime.diagnosticUnknown",
        json!([{"password":"secret"}]),
    );
    assert!(replies(&unknown)[0].is_err());
    harness.call(
        &background,
        "zephium.addListener",
        json!(["contextMenus.onClicked", 1, null, null]),
    );
    harness.advance(IDLE_TIMEOUT_NANOS - 1);
    let report = harness.call(
        &background,
        "zephium.diagnostic",
        json!([{"kind":"context-ready"}]),
    );
    assert_eq!(replies(&report), vec![Ok(Value::Null)]);
    harness.advance(1);
    harness.tick();
    assert_eq!(stops(&harness.ops()), vec![generation]);
    let evidence = std::fs::read_to_string(path).unwrap();
    assert!(evidence.contains("unknown-method") && evidence.contains("argumentShape"));
    assert!(!evidence.contains("secret") && !evidence.contains("password"));
    assert!(
        evidence.contains("dispatcher-refusal")
            && evidence.contains("contextMenus")
            && evidence.contains("onClicked")
    );
    harness.host.state.profile = ProfileId::from(99);
    let refused = harness.call(
        &background,
        "zephium.diagnostic",
        json!([{"kind":"foreign-profile"}]),
    );
    assert!(replies(&refused)[0].is_err());
    crate::diagnostics::end(ID);
}

#[test]
fn linux_dispatch_includes_menu_and_panel_apis() {
    let mut shared = ApiTable::<State>::new();
    zephium_webext_runtime::api::context_menus::register(&mut shared).unwrap();
    zephium_webext_runtime::api::side_panel::register(&mut shared).unwrap();
    let linux = table().unwrap();
    for name in shared.names() {
        assert_eq!(linux.contexts(name), shared.contexts(name), "{name}");
    }
}

#[test]
fn context_menu_registration_survives_background_stop_and_restart() {
    let mut harness = Harness::new();
    let mut install = admitted(harness.install);
    let mut raw = install.manifest.raw().clone();
    raw["permissions"]
        .as_array_mut()
        .unwrap()
        .push(json!("contextMenus"));
    install.manifest = Manifest::from_value(raw);
    let (declaration, _) = PermissionDeclaration::from_manifest(&install.manifest);
    install.permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    );
    let mut out = Outbox::default();
    harness
        .host
        .admit(install, &Activation::Installed, &mut out);
    let (background, first, out) = harness.boot(&["contextMenus.onClicked"]);
    for (seq, ..) in events(&out) {
        harness.ack(&background, seq);
    }
    let out = harness.call(
        &background,
        "contextMenus.create",
        json!([{"id":"retained","title":"Before restart"}]),
    );
    assert_eq!(replies(&out), vec![Ok(json!("retained"))]);
    harness.advance(IDLE_TIMEOUT_NANOS);
    harness.tick();
    assert_eq!(stops(&harness.ops()), vec![first]);
    harness
        .host
        .background_stopped(harness.install, first, &mut Outbox::default());
    harness.host.observe(
        Observation::Wake {
            install: harness.install,
            reason: StartReason::Event,
        },
        &mut Outbox::default(),
    );
    let (background, second, _) = harness.boot(&["contextMenus.onClicked"]);
    assert!(second > first);
    let out = harness.call(
        &background,
        "contextMenus.create",
        json!([{"id":"retained","title":"Duplicate"}]),
    );
    assert!(replies(&out)[0]
        .as_ref()
        .unwrap_err()
        .contains("duplicate id"));
    let out = harness.call(
        &background,
        "contextMenus.update",
        json!(["retained",{"title":"After restart"}]),
    );
    assert_eq!(replies(&out), vec![Ok(Value::Null)]);
    let out = harness.call(&background, "contextMenus.remove", json!(["retained"]));
    assert_eq!(replies(&out), vec![Ok(Value::Null)]);
}

#[test]
fn queued_request_facts_reproject_current_listener_ids_filters_and_permissions() {
    use zephium_core::extensions::{
        ExtensionBrowserSurface, ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab,
        ExtensionBrowserWindow,
    };
    let mut h = Harness::new();
    let (first_view, first) = h.sender(ViewKind::Background, "_zephium/background.html");
    h.authority.close_view(first_view);
    let (_, second) = h.sender(ViewKind::Background, "_zephium/background.html");
    let browser = &h.host.browser;
    let url = Url::parse("https://example.com/").unwrap();
    let tab = ExtensionBrowserTab::from_snapshot(
        None,
        ItemId::from(9),
        true,
        "fixture",
        Some(&url),
        false,
        false,
    )
    .unwrap();
    let window = ExtensionBrowserWindow::new(7, false, Some(ItemId::from(9)), vec![tab]).unwrap();
    browser.publish_surface(
        ExtensionBrowserSurface::new(
            ProfileId::from(1),
            ExtensionBrowserSurfaceGeneration::new(1).unwrap(),
            Some(7),
            vec![window],
        )
        .unwrap(),
    );
    let tab_id = browser.tab_id(ItemId::from(9)).unwrap();
    let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
        json!({"permissions":["webRequest"],"host_permissions":["https://example.com/*"]}),
    ));
    let permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    );
    let name = "webRequest.onHeadersReceived";
    let filter = json!({"urls":["https://example.com/*"]});
    let mut registry = zephium_webext_runtime::events::EventRegistry::default();
    let raw = serde_json::to_vec(&json!({"tabId":tab_id,"windowId":1,"url":"https://example.com/resource","top":"https://example.com/","initiator":null,"details":{"frameId":-1,"type":"other"},"headers":[["Content-Type","text/plain"],["Set-Cookie","private"]],"extra":["responseHeaders"]})).unwrap();
    registry
        .add_with_options(
            &first,
            ListenerId::new(1).unwrap(),
            name,
            Some(&filter),
            &["responseHeaders".into()],
        )
        .unwrap();
    let project = |registry: &zephium_webext_runtime::events::EventRegistry,
                   sender: &Sender,
                   permissions: Option<&InstallPermissions>|
     -> Value {
        serde_json::from_slice(
            &super::glue::request_arguments(registry, permissions, sender, name, &raw, browser)
                .unwrap(),
        )
        .unwrap()
    };
    let before = project(&registry, &first, Some(&permissions));
    assert_eq!(before[1], json!([1]));
    assert_eq!(before[0]["responseHeaders"].as_array().unwrap().len(), 1);
    registry.cancel_context(&first.context_key());
    registry
        .add_with_options(
            &second,
            ListenerId::new(99).unwrap(),
            name,
            Some(&filter),
            &["responseHeaders".into()],
        )
        .unwrap();
    assert_eq!(
        project(&registry, &second, Some(&permissions))[1],
        json!([99])
    );
    assert_eq!(project(&registry, &second, None), json!([{}, []]));
    registry.remove(&second, name, ListenerId::new(99).unwrap());
    registry
        .add_with_options(
            &second,
            ListenerId::new(100).unwrap(),
            name,
            Some(&json!({"urls":["https://foreign.test/*"]})),
            &["responseHeaders".into()],
        )
        .unwrap();
    assert_eq!(
        project(&registry, &second, Some(&permissions)),
        json!([{}, []])
    );
}

#[test]
fn native_table_registers_validated_unavailable_capabilities() {
    let table = table().unwrap();
    for method in [
        "cookies.getAll",
        "downloads.search",
        "identity.launchWebAuthFlow",
        "readingList.query",
        "tabGroups.query",
        "tts.speak",
        "desktopCapture.chooseDesktopMedia",
        "tabCapture.capture",
        "omnibox.setDefaultSuggestion",
        "userScripts.getScripts",
        "management.getAll",
    ] {
        assert!(table.names().any(|name| name == method), "{method}");
        let access = zephium_webext_runtime::schema::access(method).unwrap();
        assert_eq!(table.gate(method), Some(access.gate), "{method}");
    }
}

#[test]
fn shutdown_settles_pending_permission_publication() {
    let mut harness = Harness::new();
    let _ = harness.running(&[]);
    let update = PermissionUpdate {
        completion: Rc::new(std::cell::RefCell::new(None)),
    };
    harness.host.network_permission_tickets.insert(
        7,
        (
            harness.host.permissions[&harness.install].clone(),
            PermissionCompletion::Host(update.clone()),
        ),
    );
    harness.host.shutdown(&mut Outbox::default());
    assert_eq!(
        update.poll(),
        Some(Err(ApiError::context_invalidated().message().into()))
    );
    assert!(harness.host.network_permission_tickets.is_empty());
}

#[test]
fn offscreen_ready_requires_loaded_owned_current_document_and_binding_barrier() {
    use crate::bridge::{BridgeViewId, FrameLifecycle, ViewRef};
    use zephium_webext_runtime::context::FrameId;
    for authenticate_first in [false, true] {
        let mut harness = Harness::new();
        let (background, _, _) = harness.running(&[]);
        assert_eq!(
            replies(&harness.call(&background, "zephium.offscreenReady", json!([]))),
            vec![Err(ApiError::not_available_in_context(
                "zephium.offscreenReady"
            )
            .message()
            .into())]
        );
        harness.call(
            &background,
            "offscreen.createDocument",
            json!([{"url":"off.html","reasons":["DOM_PARSER"],"justification":"parse"}]),
        );
        let generation = harness
            .ops()
            .into_iter()
            .find_map(|op| match op {
                NativeOp::CreateOffscreen { generation, .. } => Some(generation),
                _ => None,
            })
            .unwrap();
        let view = BridgeViewId(99);
        assert!(harness
            .host
            .offscreen_attached(harness.install, generation, view));
        let (_, offscreen) = harness.sender(ViewKind::Offscreen, "off.html");
        let document = offscreen.document();
        let owner = ViewRef::Extension {
            install: harness.install,
            kind: ViewKind::Offscreen,
        };
        let created = |url| FrameLifecycle::DocumentCreated {
            view,
            owner,
            frame: FrameId::MAIN,
            parent: None,
            opaque: false,
            creator: None,
            document,
            url,
        };
        let loaded = FrameLifecycle::DocumentLoaded {
            view,
            owner,
            frame: FrameId::MAIN,
            document,
        };
        let mut out = Outbox::default();
        harness
            .host
            .frame_lifecycle(&created(Url::parse("about:blank").unwrap()), &mut out);
        harness.host.frame_lifecycle(&loaded, &mut out);
        assert!(!harness
            .ops()
            .iter()
            .any(|op| matches!(op, NativeOp::OffscreenReady { .. })));
        harness
            .host
            .frame_lifecycle(&created(offscreen.url().clone()), &mut out);
        if authenticate_first {
            assert_eq!(
                replies(&harness.call(&offscreen, "zephium.offscreenReady", json!([]))),
                vec![Ok(Value::Null)]
            );
        }
        harness.host.frame_lifecycle(&loaded, &mut out);
        if !authenticate_first {
            assert!(!harness
                .ops()
                .iter()
                .any(|op| matches!(op, NativeOp::OffscreenReady { .. })));
            assert_eq!(
                replies(&harness.call(&offscreen, "zephium.offscreenReady", json!([]))),
                vec![Ok(Value::Null)]
            );
        }
        assert!(harness.ops().iter().any(|op| matches!(op, NativeOp::OffscreenReady { generation: current, document: current_document, .. } if *current == generation && *current_document == document)));
        let removed = if authenticate_first {
            FrameLifecycle::DocumentRemoved {
                view,
                owner,
                frame: FrameId::MAIN,
                document,
            }
        } else {
            FrameLifecycle::ViewReset {
                view,
                owner,
                reason: CloseReason::Reattached,
            }
        };
        harness.host.frame_lifecycle(&removed, &mut out);
        assert!(!harness
            .host
            .offscreen_document_live(harness.install, document));
        harness.host.offscreen_gone(harness.install, &mut out);
        assert!(!harness.host.offscreen_current(harness.install, generation));
        harness.call(
            &background,
            "offscreen.createDocument",
            json!([{"url":"off.html","reasons":["DOM_PARSER"],"justification":"parse again"}]),
        );
        let next = harness
            .ops()
            .into_iter()
            .find_map(|op| match op {
                NativeOp::CreateOffscreen { generation, .. } => Some(generation),
                _ => None,
            })
            .unwrap();
        assert_ne!(generation, next);
        assert!(!harness
            .host
            .offscreen_attached(harness.install, generation, view));
        assert!(!harness
            .host
            .offscreen_document_live(harness.install, document));
    }
}
