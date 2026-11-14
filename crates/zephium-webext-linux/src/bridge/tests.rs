//! Core state-machine tests: claims, buffering, generations, delivery and
//! limits, without GTK.

use std::sync::Arc;

use serde_json::{json, Value};
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_runtime::context::{
    ContextKey, ContextKind, ContextMask, ExtensionId, ExtensionScheme, FrameId, InjectedWorld,
    InstallAdmission, Sender, ViewKind,
};
use zephium_webext_runtime::dispatch::{ApiTable, Dispatcher, MethodSpec};
use zephium_webext_runtime::envelope::{ApiName, CallId, Direction, Envelope};
use zephium_webext_runtime::limits::{ManualClock, MAX_MESSAGE_BYTES_IN_WEB_RENDERER, RATE_BURST};
use zephium_webext_runtime::permissions::{
    Gate, InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};
use zephium_webkit_web_process_protocol::protocol::{
    decode_ui, DeliveryStatus, Evaluation, Event, FrameEvent, FrameInfo, MatchRule, Post,
    UiMessage, DELIVER_MESSAGE,
};

use super::buffer::{BUFFER_TIMEOUT_MS, MAX_BUFFERED_POSTS_PER_VIEW};
use super::claims::Refusal;
use super::core::{AttachOutcome, Core, Effect, Outgoing, Phase, MAX_QUEUED_FRAMES_PER_CONTEXT};
use super::plan::{world_name, AttachPlan, ExtensionPages, WorldGrant};
use super::runtime::{
    CloseReason, DeliverError, DispatchRuntime, Outbox, PortTraffic, Ports, Runtime,
};
use super::ReplyToken;

const EXTENSION: &str = "abcdefghijklmnopabcdefghijklmnop";
const OTHER: &str = "ponmlkjihgfedcbaponmlkjihgfedcba";
const SITE: &str = "https://a.test";

fn install() -> ExtensionInstallId {
    ExtensionInstallId::from(10)
}

fn other_install() -> ExtensionInstallId {
    ExtensionInstallId::from(11)
}

fn admission(install: ExtensionInstallId, id: &str) -> InstallAdmission {
    InstallAdmission {
        install,
        extension_id: ExtensionId::parse(id).expect("id"),
        generation: ExtensionRuntimeGeneration::INITIAL,
        allowed_in_incognito: false,
        externally_connectable: None,
        unpacked: false,
    }
}

fn world() -> String {
    world_name(install(), &InjectedWorld::Isolated)
}

fn tab_plan(origins: &[&str]) -> AttachPlan {
    AttachPlan {
        worlds: vec![WorldGrant {
            install: install(),
            world: InjectedWorld::Isolated,
            matches: origins
                .iter()
                .map(|origin| MatchRule::for_origin(origin).expect("match rule"))
                .collect(),
            bootstrap: Arc::from("void 0"),
        }],
        extension_pages: vec![ExtensionPages {
            extension: ExtensionId::parse(EXTENSION).expect("id"),
            sandboxed_paths: vec!["/sandbox.html".into()],
        }],
        apply_policy: true,
        ..AttachPlan::default()
    }
}

fn own_plan() -> AttachPlan {
    AttachPlan {
        extension_pages: vec![ExtensionPages {
            extension: ExtensionId::parse(EXTENSION).expect("id"),
            sandboxed_paths: Vec::new(),
        }],
        ..AttachPlan::default()
    }
}

struct Harness {
    core: Core,
    reply: u64,
    now: u64,
}

impl Harness {
    fn new() -> Self {
        let mut core = Core::new(
            ProfileId::from(1),
            false,
            ExtensionScheme::CHROME_EXTENSION,
            "token".into(),
        );
        core.admit(admission(install(), EXTENSION)).expect("admit");
        core.admit(admission(other_install(), OTHER))
            .expect("admit");
        core.take_effects();
        Self {
            core,
            reply: 0,
            now: 0,
        }
    }

    /// Registers and accepts an attachment; returns view id and generation.
    fn tab(&mut self, tab: u128, plan: &AttachPlan) -> (u64, u64) {
        let id = self
            .core
            .register_tab(ItemId::from(tab), plan)
            .expect("register");
        let generation = self.accept(id);
        (id, generation)
    }

    fn owned(&mut self, kind: ViewKind) -> (u64, u64) {
        let id = self
            .core
            .register_owned(install(), kind, &own_plan())
            .expect("register");
        let generation = self.accept(id);
        (id, generation)
    }

    fn accept(&mut self, id: u64) -> u64 {
        let generation = self
            .core
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                Effect::Send {
                    view,
                    outgoing: Outgoing::Attach { generation },
                    ..
                } if view == id => Some(generation),
                _ => None,
            })
            .expect("attach sent");
        self.core
            .attach_settled(id, generation, AttachOutcome::Accepted);
        self.core.take_effects();
        generation
    }

    fn event(&mut self, id: u64, event: Event) -> Vec<Effect> {
        self.reply += 1;
        self.core.event(id, event, ReplyToken(self.reply), self.now);
        self.core.take_effects()
    }

    fn frames(&mut self, id: u64, generation: u64, mut events: Vec<FrameEvent>) -> Vec<Effect> {
        let evaluated: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                FrameEvent::Created(info) | FrameEvent::Committed(info) => {
                    Some(FrameEvent::Evaluated(Evaluation {
                        install_id: format!("{:032x}", 10),
                        script_id: String::new(),
                        world: world(),
                        frame_id: info.frame_id,
                        document_seq: info.document_seq,
                        table_generation: 0,
                        file_index: u16::MAX,
                        success: true,
                    }))
                }
                _ => None,
            })
            .collect();
        events.extend(evaluated);
        self.event(id, Event::Frames { generation, events })
    }

    fn page(&mut self, id: u64, generation: u64, uri: &str) {
        self.frames(
            id,
            generation,
            vec![FrameEvent::Created(info(1, 1, uri, true))],
        );
    }

    fn post(&mut self, id: u64, post: Post) -> Vec<Effect> {
        self.event(id, Event::Post(post))
    }

    fn refusals(&self, refusal: Refusal) -> u64 {
        self.core
            .stats()
            .refused
            .get(&refusal)
            .copied()
            .unwrap_or(0)
    }
}

fn info(frame_id: u64, document_seq: u64, uri: &str, is_main: bool) -> FrameInfo {
    FrameInfo {
        frame_id,
        document_seq,
        uri: uri.into(),
        is_main,
        ..FrameInfo::default()
    }
}

fn post(generation: u64, frame_id: u64, document_seq: u64, world: &str, uri: &str) -> Post {
    Post {
        external_install: None,
        generation,
        post_id: 1,
        frame_id,
        document_seq,
        world: world.into(),
        uri: uri.into(),
        bytes: b"{}".to_vec(),
    }
}

fn senders(effects: &[Effect]) -> Vec<Sender> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Receive { sender, .. } => Some(sender.clone()),
            _ => None,
        })
        .collect()
}

fn replied(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Reply(_)))
}

fn closed(effects: &[Effect]) -> Vec<(ContextKey, CloseReason)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Closed { context, reason } => Some((context.clone(), *reason)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_consistent_post_binds_the_emitting_tab_frame_and_world() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.frames(
        id,
        generation,
        vec![
            FrameEvent::Created(info(1, 1, "https://a.test/", true)),
            FrameEvent::Created(info(2, 2, "https://a.test/frame", false)),
        ],
    );
    let effects = h.post(
        id,
        post(generation, 2, 2, &world(), "https://a.test/frame#x"),
    );
    assert!(replied(&effects));
    let sender = senders(&effects).pop().expect("bound");
    assert_eq!(sender.kind(), ContextKind::ContentScript);
    assert_eq!(sender.tab(), Some(ItemId::from(5)));
    assert_ne!(sender.frame(), Some(FrameId::MAIN));
    assert_eq!(sender.install(), install());
    assert_eq!(sender.url().as_str(), "https://a.test/frame#x");
    let main = h.post(id, post(generation, 1, 1, &world(), "https://a.test/"));
    assert_eq!(senders(&main)[0].frame(), Some(FrameId::MAIN));
    assert_eq!(h.core.stats().admitted, 2);
}

#[test]
fn inconsistent_claims_are_refused_and_counted() {
    struct Case {
        name: &'static str,
        post: fn(u64) -> Post,
        refusal: Refusal,
    }
    let cases = [
        Case {
            name: "stale generation",
            post: |g| post(g - 1, 1, 1, &world(), "https://a.test/"),
            refusal: Refusal::StaleGeneration,
        },
        Case {
            name: "newer generation",
            post: |g| post(g + 1, 1, 1, &world(), "https://a.test/"),
            refusal: Refusal::StaleGeneration,
        },
        Case {
            name: "forged world name",
            post: |g| post(g, 1, 1, "zephium-webext-forged", "https://a.test/"),
            refusal: Refusal::ForgedWorld,
        },
        Case {
            name: "another install's world",
            post: |g| {
                post(
                    g,
                    1,
                    1,
                    &world_name(other_install(), &InjectedWorld::Isolated),
                    "https://a.test/",
                )
            },
            refusal: Refusal::ForgedWorld,
        },
        Case {
            name: "stale document",
            post: |g| post(g, 1, 1, &world(), "https://a.test/"),
            refusal: Refusal::StaleDocument,
        },
        Case {
            name: "uri outside the recorded origin",
            post: |g| post(g, 1, 4, &world(), "https://evil.test/"),
            refusal: Refusal::CrossOriginUri,
        },
        Case {
            name: "unparseable uri",
            post: |g| post(g, 1, 4, &world(), "::"),
            refusal: Refusal::InvalidUri,
        },
        Case {
            name: "world not granted for the origin",
            post: |g| post(g, 3, 5, &world(), "https://b.test/"),
            refusal: Refusal::NotAdmitted,
        },
        Case {
            name: "page world of a web page",
            post: |g| post(g, 1, 4, "", "https://a.test/"),
            refusal: Refusal::NotAdmitted,
        },
    ];
    for case in cases {
        let mut h = Harness::new();
        let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
        h.frames(
            id,
            generation,
            vec![
                FrameEvent::Created(info(1, 1, "https://a.test/", true)),
                FrameEvent::Created(info(2, 2, "https://a.test/f", false)),
                FrameEvent::Created(info(2, 3, "https://a.test/g", false)),
                FrameEvent::Created(info(1, 4, "https://a.test/", true)),
                FrameEvent::Created(info(3, 5, "https://b.test/", false)),
            ],
        );
        let effects = h.post(id, (case.post)(generation));
        assert!(replied(&effects), "{}", case.name);
        assert!(senders(&effects).is_empty(), "{}", case.name);
        assert_eq!(h.refusals(case.refusal), 1, "{}", case.name);
    }
}

#[test]
fn a_forged_claim_from_one_tab_cannot_bind_another_tabs_context() {
    let mut h = Harness::new();
    let (a, ga) = h.tab(5, &tab_plan(&[SITE]));
    let (b, gb) = h.tab(6, &tab_plan(&["https://b.test"]));
    h.frames(
        a,
        ga,
        vec![FrameEvent::Created(info(100, 1, "https://a.test/", true))],
    );
    h.frames(
        b,
        gb,
        vec![FrameEvent::Created(info(200, 2, "https://b.test/", true))],
    );
    // B's renderer names A's frame, document, world and URI.
    h.now = 0;
    let effects = h.post(b, post(gb, 100, 1, &world(), "https://a.test/"));
    assert!(senders(&effects).is_empty());
    h.now = BUFFER_TIMEOUT_MS;
    h.core.tick(h.now);
    let effects = h.core.take_effects();
    assert!(senders(&effects).is_empty());
    assert!(replied(&effects));
    assert_eq!(h.refusals(Refusal::UnknownFrame), 1);
    // A's generation on B's view, and B's own frame with A's origin.
    let effects = h.post(b, post(ga, 200, 2, &world(), "https://b.test/"));
    assert!(senders(&effects).is_empty());
    let effects = h.post(b, post(gb, 200, 2, &world(), "https://a.test/"));
    assert!(senders(&effects).is_empty());
    // Whatever binds on B is B's tab.
    let effects = h.post(b, post(gb, 200, 2, &world(), "https://b.test/x"));
    assert_eq!(senders(&effects)[0].tab(), Some(ItemId::from(6)));
}

#[test]
fn extension_owned_views_bind_only_their_main_page_world() {
    let mut h = Harness::new();
    let (id, generation) = h.owned(ViewKind::Popup);
    let page = format!("chrome-extension://{EXTENSION}/popup.html");
    h.core.load_committed(id, Some(&page), 0);
    h.frames(
        id,
        generation,
        vec![
            FrameEvent::Created(info(1, 1, &page, true)),
            FrameEvent::Created(info(2, 2, &page, false)),
        ],
    );
    let effects = h.post(id, post(generation, 1, 1, &world(), &page));
    assert!(senders(&effects).is_empty());
    assert_eq!(h.refusals(Refusal::ExtensionViewWorld), 1);
    let effects = h.post(id, post(generation, 2, 2, "", &page));
    assert!(senders(&effects).is_empty());
    assert_eq!(h.refusals(Refusal::ExtensionViewSubframe), 1);
    let effects = h.post(id, post(generation, 1, 1, "", &page));
    let sender = senders(&effects).pop().expect("bound");
    assert_eq!(sender.kind(), ContextKind::Popup);
    assert_eq!(sender.tab(), None);
}

#[test]
fn extension_pages_in_tabs_follow_the_plan_and_sandbox_paths() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    let options = format!("chrome-extension://{EXTENSION}/options.html");
    let sandbox = format!("chrome-extension://{EXTENSION}/sandbox.html");
    let other = format!("chrome-extension://{OTHER}/page.html");
    h.frames(
        id,
        generation,
        vec![
            FrameEvent::Created(info(1, 1, "https://a.test/", true)),
            FrameEvent::Created(info(2, 2, &options, false)),
            FrameEvent::Created(info(3, 3, &sandbox, false)),
            FrameEvent::Created(info(4, 4, &other, false)),
        ],
    );
    let effects = h.post(id, post(generation, 2, 2, "", &options));
    assert_eq!(senders(&effects)[0].kind(), ContextKind::ExtensionFrame);
    let effects = h.post(id, post(generation, 3, 3, "", &sandbox));
    assert!(senders(&effects).is_empty(), "sandboxed pages get no API");
    assert_eq!(h.refusals(Refusal::Unbound), 1);
    let effects = h.post(id, post(generation, 4, 4, "", &other));
    assert!(senders(&effects).is_empty());
    assert_eq!(h.refusals(Refusal::NotAdmitted), 1);
}

#[test]
fn posts_before_their_frame_record_wait_bounded_and_then_resolve() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    let effects = h.post(id, post(generation, 1, 1, &world(), "https://a.test/"));
    assert!(!replied(&effects), "held until reconciled");
    assert!(h.core.has_buffered());
    let effects = h.frames(
        id,
        generation,
        vec![FrameEvent::Created(info(1, 1, "https://a.test/", true))],
    );
    assert_eq!(senders(&effects).len(), 1);
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Reply(_)))
            .count(),
        2,
        "the frames event and the held post are both answered"
    );
    assert!(!h.core.has_buffered());

    for _ in 0..MAX_BUFFERED_POSTS_PER_VIEW {
        let effects = h.post(id, post(generation, 9, 9, &world(), "https://a.test/"));
        assert!(!replied(&effects));
    }
    let effects = h.post(id, post(generation, 9, 9, &world(), "https://a.test/"));
    assert!(replied(&effects));
    assert_eq!(h.refusals(Refusal::BufferFull), 1);
    h.core.tick(BUFFER_TIMEOUT_MS - 1);
    assert!(h.core.take_effects().is_empty());
    h.core.tick(BUFFER_TIMEOUT_MS);
    let effects = h.core.take_effects();
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Reply(_)))
            .count(),
        MAX_BUFFERED_POSTS_PER_VIEW
    );
    assert_eq!(
        h.refusals(Refusal::UnknownFrame),
        MAX_BUFFERED_POSTS_PER_VIEW as u64
    );
}

#[test]
fn main_frame_claims_must_agree_with_the_host_commit() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE, "https://b.test"]));
    h.core.load_committed(id, Some("https://a.test/"), 0);
    h.page(id, generation, "https://a.test/");
    h.core.load_committed(id, Some("https://b.test/"), 0);
    h.core.take_effects();
    // The old document posts after the host saw the new one commit.
    let effects = h.post(id, post(generation, 1, 1, &world(), "https://a.test/"));
    assert!(
        !replied(&effects),
        "waits for the frame records to catch up"
    );
    let effects = h.frames(
        id,
        generation,
        vec![
            FrameEvent::Destroyed {
                frame_id: 1,
                document_seq: 1,
            },
            FrameEvent::Created(info(1, 2, "https://b.test/", true)),
        ],
    );
    assert!(senders(&effects).is_empty());
    assert_eq!(h.refusals(Refusal::StaleDocument), 1);
    let effects = h.post(id, post(generation, 1, 2, &world(), "https://b.test/"));
    assert_eq!(senders(&effects).len(), 1);
}

#[test]
fn navigation_reattachment_and_crashes_end_contexts() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.page(id, generation, "https://a.test/");
    let sender = senders(&h.post(id, post(generation, 1, 1, &world(), "https://a.test/")))
        .pop()
        .expect("bound");
    let key = sender.context_key();
    assert_eq!(h.core.context_view(&key), Some(id));

    let effects = h.frames(
        id,
        generation,
        vec![FrameEvent::Created(info(1, 2, "https://a.test/next", true))],
    );
    assert_eq!(
        closed(&effects),
        vec![(key.clone(), CloseReason::DocumentGone)]
    );
    assert_eq!(h.core.deliver(&key, vec![1]), Err(DeliverError::Stale));
    let stale = h.post(id, post(generation, 1, 1, &world(), "https://a.test/"));
    assert!(senders(&stale).is_empty());

    let sender = senders(&h.post(id, post(generation, 1, 2, &world(), "https://a.test/next")))
        .pop()
        .expect("bound");
    h.core
        .update_plan(id, &tab_plan(&[SITE, "https://b.test"]), false)
        .expect("plan");
    assert!(
        h.core.take_effects().is_empty(),
        "widening waits for navigation"
    );
    h.core.load_started(id);
    let effects = h.core.take_effects();
    assert!(closed(&effects).contains(&(sender.context_key(), CloseReason::Reattached)));
    let next = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Send {
                outgoing: Outgoing::Attach { generation },
                ..
            } => Some(*generation),
            _ => None,
        })
        .expect("reattached");
    assert!(next > generation);
    h.core.attach_settled(id, next, AttachOutcome::Accepted);
    let late = h.post(id, post(generation, 1, 2, &world(), "https://a.test/next"));
    assert!(senders(&late).is_empty());
    assert!(h.refusals(Refusal::StaleGeneration) >= 1);

    h.page(id, next, "https://a.test/");
    let sender = senders(&h.post(id, post(next, 1, 1, &world(), "https://a.test/")))
        .pop()
        .expect("bound");
    h.core.process_gone(id);
    let effects = h.core.take_effects();
    assert!(closed(&effects).contains(&(sender.context_key(), CloseReason::ProcessGone)));
    assert_eq!(h.core.phase(id), Some(Phase::Pending));
    assert!(!effects
        .iter()
        .any(|effect| matches!(effect, Effect::Send { .. })));
    h.core.attach_settled(id, next, AttachOutcome::Accepted);
    h.core
        .update_plan(id, &tab_plan(&[SITE]), true)
        .expect("plan while dead");
    assert_eq!(h.core.phase(id), Some(Phase::Pending));
    assert!(!h
        .core
        .take_effects()
        .iter()
        .any(|effect| matches!(effect, Effect::Send { .. })));
    h.core.load_started(id);
    assert!(h.accept(id) > next);
}

#[test]
fn narrowing_a_plan_revokes_at_once() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.page(id, generation, "https://a.test/");
    let sender = senders(&h.post(id, post(generation, 1, 1, &world(), "https://a.test/")))
        .pop()
        .expect("bound");
    h.core
        .update_plan(id, &tab_plan(&["https://b.test"]), false)
        .expect("plan");
    let effects = h.core.take_effects();
    assert_eq!(
        closed(&effects),
        vec![(sender.context_key(), CloseReason::Revoked)]
    );
    let effects = h.post(id, post(generation, 1, 1, &world(), "https://a.test/"));
    assert!(
        senders(&effects).is_empty(),
        "the module's older plan does not count"
    );
    h.core.retire(install());
    let effects = h.post(id, post(generation, 1, 1, &world(), "https://b.test/"));
    assert!(senders(&effects).is_empty());
}

#[test]
fn deliveries_target_the_bound_frame_and_are_bounded() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.page(id, generation, "https://a.test/");
    let mut wire = post(generation, 1, 1, &world(), "https://a.test/");
    wire.document_seq = 1;
    let sender = senders(&h.post(id, wire)).pop().expect("bound");
    let key = sender.context_key();
    h.core.deliver(&key, b"reply".to_vec()).expect("deliver");
    let effects = h.core.take_effects();
    let Some(Effect::Send { name, bytes, .. }) = effects.first() else {
        panic!("deliver sent");
    };
    assert_eq!(*name, DELIVER_MESSAGE);
    let Ok(UiMessage::Deliver(deliver)) = decode_ui(DELIVER_MESSAGE, bytes) else {
        panic!("deliver message");
    };
    assert_eq!(
        (
            deliver.generation,
            deliver.frame_id,
            deliver.document_seq,
            deliver.world.as_str(),
            deliver.bytes.as_slice()
        ),
        (generation, 1, 1, world().as_str(), &b"reply"[..])
    );
    assert_eq!(
        h.core.deliver(
            &key,
            vec![0; MAX_MESSAGE_BYTES_IN_WEB_RENDERER as usize + 1]
        ),
        Err(DeliverError::TooLarge)
    );
    let mut accepted = 1;
    while h.core.deliver(&key, vec![1]).is_ok() {
        accepted += 1;
    }
    assert_eq!(
        accepted,
        super::core::MAX_INFLIGHT_PER_CONTEXT + MAX_QUEUED_FRAMES_PER_CONTEXT
    );
    assert_eq!(
        h.core.deliver(&key, vec![1]),
        Err(DeliverError::Backpressure)
    );
    h.core.take_effects();
    h.core
        .delivery_settled(id, &key, DeliveryStatus::NoReceiver);
    let effects = h.core.take_effects();
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Undeliverable {
            error: DeliverError::NoReceiver,
            ..
        }
    )));
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            Effect::Send {
                outgoing: Outgoing::Deliver { .. },
                ..
            }
        )),
        "a settled slot sends the next queued frame"
    );
}

#[test]
fn oversized_posts_are_refused_by_context_class() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.page(id, generation, "https://a.test/");
    let mut large = post(generation, 1, 1, &world(), "https://a.test/");
    large.bytes = vec![0; MAX_MESSAGE_BYTES_IN_WEB_RENDERER as usize + 1];
    let effects = h.post(id, large);
    assert!(senders(&effects).is_empty());
    assert_eq!(h.refusals(Refusal::Oversized), 1);
}

#[test]
fn missing_or_refused_modules_leave_all_views_unavailable() {
    for outcome in [AttachOutcome::Unhandled, AttachOutcome::Refused] {
        let mut h = Harness::new();
        let owned = h
            .core
            .register_owned(install(), ViewKind::Background, &own_plan())
            .expect("owned");
        let tab = h
            .core
            .register_tab(ItemId::from(9), &tab_plan(&[SITE]))
            .expect("tab");
        let attachments: Vec<_> = h
            .core
            .take_effects()
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Send {
                    view,
                    outgoing: Outgoing::Attach { generation },
                    ..
                } => Some((view, generation)),
                _ => None,
            })
            .collect();
        assert_eq!(attachments.len(), 2);
        for (id, generation) in attachments {
            h.core.attach_settled(id, generation, outcome);
            assert_eq!(h.core.phase(id), Some(Phase::Refused(generation)));
            let effects = h.core.take_effects();
            assert!(effects.iter().any(|effect| matches!(effect,
                Effect::Unavailable { view, reason, .. } if *view == id && *reason == if outcome == AttachOutcome::Unhandled {
                    super::UnavailableReason::ModuleUnhandled
                } else { super::UnavailableReason::ModuleRefused }
            )));
            assert!(senders(&h.post(id, post(generation, 1, 1, &world(), SITE))).is_empty());
        }
        assert!(h.core.phase(owned).is_some());
        assert!(h.core.phase(tab).is_some());
    }
}

#[test]
fn crash_then_detach_never_sends_to_a_dead_process() {
    let mut h = Harness::new();
    let (id, _) = h.owned(ViewKind::Background);
    h.core.process_gone(id);
    h.core.take_effects();
    h.core.detach(id);
    assert!(!h
        .core
        .take_effects()
        .iter()
        .any(|effect| matches!(effect, Effect::Send { .. })));
}

#[test]
fn policies_reach_live_views_and_every_later_attachment() {
    let mut h = Harness::new();
    let (id, _) = h.tab(5, &tab_plan(&[SITE]));
    let policy = std::rc::Rc::new(super::policy::SealedPolicy::new(b"policy").expect("sealed"));
    let first = h.core.publish_policy(policy.clone());
    let sends = |effects: Vec<Effect>| {
        effects
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Send {
                    outgoing: Outgoing::Policy { policy_generation },
                    policy: Some(_),
                    ..
                } => Some(policy_generation),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(sends(h.core.take_effects()), vec![first]);
    h.core.process_gone(id);
    h.core.load_started(id);
    let generation = h
        .core
        .take_effects()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Send {
                outgoing: Outgoing::Attach { generation },
                ..
            } => Some(generation),
            _ => None,
        })
        .expect("reattach");
    h.core
        .attach_settled(id, generation, AttachOutcome::Accepted);
    assert_eq!(sends(h.core.take_effects()), vec![first]);
    let second = h.core.publish_policy(policy);
    assert!(second > first);
}

#[test]
fn detach_and_shutdown_end_everything() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.page(id, generation, "https://a.test/");
    let sender = senders(&h.post(id, post(generation, 1, 1, &world(), "https://a.test/")))
        .pop()
        .expect("bound");
    h.post(id, post(generation, 7, 7, &world(), "https://a.test/"));
    h.core.shutdown();
    let effects = h.core.take_effects();
    assert!(closed(&effects).contains(&(sender.context_key(), CloseReason::Detached)));
    assert!(replied(&effects), "held posts are answered");
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Send {
            outgoing: Outgoing::Detach,
            ..
        }
    )));
    assert!(h.core.views().is_empty());
    assert_eq!(
        h.core.deliver(&sender.context_key(), vec![1]),
        Err(DeliverError::Closed)
    );
    assert!(h
        .core
        .register_tab(ItemId::from(6), &tab_plan(&[SITE]))
        .is_err());
}

#[derive(Default)]
struct NoPorts;

impl<S> Ports<S> for NoPorts {
    fn traffic(&mut self, _: &mut Dispatcher<S>, _: PortTraffic<'_>, _: &mut Outbox) {}
}

fn call(id: u32) -> Vec<u8> {
    Envelope::Call {
        id: CallId::new(id).expect("id"),
        name: ApiName::parse("test.echo").expect("name"),
        args: b"[1]",
    }
    .encode(Direction::FromContext, u64::MAX)
    .expect("encode")
}

#[test]
fn the_dispatcher_enforces_the_authoritative_rate_limit_on_bridge_posts() {
    let mut table = ApiTable::new();
    table
        .register(
            "test.echo",
            MethodSpec::new(Gate::Open, ContextMask::PAGE_SCRIPTS, |_: &mut (), call| {
                let args = call.args().to_vec();
                call.reply(Ok(Value::Array(args)))
            }),
        )
        .expect("register");
    let mut runtime = DispatchRuntime::new(Dispatcher::new(table), (), NoPorts)
        .with_clock(ManualClock::default());
    let (declaration, _) =
        PermissionDeclaration::from_manifest(&Manifest::from_value(json!({"manifest_version": 3})));
    runtime.permissions.insert(
        install(),
        InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        ),
    );
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.page(id, generation, "https://a.test/");
    let mut replies = Vec::new();
    for n in 1..=RATE_BURST + 1 {
        let mut wire = post(generation, 1, 1, &world(), "https://a.test/");
        wire.bytes = call(n);
        let effects = h.post(id, wire);
        let sender = senders(&effects).pop().expect("bound");
        let mut out = Outbox::default();
        runtime.receive(&sender, &call(n), &mut out);
        assert_eq!(out.frames.len(), 1);
        assert_eq!(out.frames[0].0, sender.context_key());
        replies.push(out.frames.remove(0).1);
    }
    let decode = |frame: &[u8]| match Envelope::decode(frame, Direction::ToContext, u64::MAX) {
        Ok(Envelope::Reply { result, .. }) => result.map(<[u8]>::to_vec).map_err(str::to_owned),
        other => panic!("reply expected: {other:?}"),
    };
    assert_eq!(decode(&replies[0]), Ok(b"[1]".to_vec()));
    runtime.permissions.clear();
    let sender = senders(&h.post(id, post(generation, 1, 1, &world(), "https://a.test/")))
        .pop()
        .expect("bound");
    let mut out = Outbox::default();
    runtime.receive(&sender, &call(9_999), &mut out);
    assert!(
        decode(&out.frames[0].1).is_err(),
        "calls without permission state settle"
    );
    assert!(
        decode(&replies[RATE_BURST as usize]).is_err(),
        "burst exhausted"
    );
}

#[test]
fn bootstrap_posts_wait_for_verified_success_and_forged_evaluations_grant_nothing() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.event(
        id,
        Event::Frames {
            generation,
            events: vec![FrameEvent::Created(info(1, 1, "https://a.test/", true))],
        },
    );
    assert!(senders(&h.post(id, post(generation, 1, 1, &world(), "https://a.test/"))).is_empty());
    assert_eq!(h.core.stats().buffered, 1);
    let evaluation = Evaluation {
        install_id: format!("{:032x}", 10),
        script_id: String::new(),
        world: world(),
        frame_id: 1,
        document_seq: 1,
        table_generation: 0,
        file_index: u16::MAX,
        success: true,
    };
    for forged in [
        Evaluation {
            install_id: format!("{:032x}", 11),
            ..evaluation.clone()
        },
        Evaluation {
            table_generation: 7,
            script_id: "unknown".into(),
            file_index: 0,
            ..evaluation.clone()
        },
        Evaluation {
            document_seq: 2,
            ..evaluation.clone()
        },
        Evaluation {
            world: world_name(other_install(), &InjectedWorld::Isolated),
            ..evaluation.clone()
        },
        Evaluation {
            success: false,
            ..evaluation.clone()
        },
    ] {
        assert!(senders(&h.event(
            id,
            Event::Frames {
                generation,
                events: vec![FrameEvent::Evaluated(forged)]
            }
        ))
        .is_empty());
    }
    let effects = h.event(
        id,
        Event::Frames {
            generation,
            events: vec![FrameEvent::Evaluated(evaluation)],
        },
    );
    assert_eq!(senders(&effects).len(), 1);
    assert_eq!(h.core.stats().frame_records_refused, 4);
}

#[test]
fn immediate_structural_widening_keeps_the_live_context_and_generation() {
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.page(id, generation, "https://a.test/");
    let sender = senders(&h.post(id, post(generation, 1, 1, &world(), "https://a.test/")))
        .pop()
        .expect("sender");
    h.core
        .update_plan(id, &tab_plan(&[SITE, "https://b.test"]), true)
        .expect("widen");
    let effects = h.core.take_effects();
    assert!(closed(&effects).is_empty());
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Send { outgoing: Outgoing::Attach { generation: next }, .. } if *next == generation)));
    h.core
        .attach_settled(id, generation, AttachOutcome::Accepted);
    assert_eq!(h.core.context_view(&sender.context_key()), Some(id));
    assert_eq!(
        senders(&h.post(id, post(generation, 1, 1, &world(), "https://a.test/"))).len(),
        1
    );
}

#[test]
fn external_claims_require_both_the_marker_and_exact_host_match_path() {
    use zephium_webext_runtime::context::ExternallyConnectable;
    use zephium_webext_runtime::pattern::ContentScriptMatcher;
    use zephium_webkit_web_process_protocol::protocol::ExternalRule;
    let mut h = Harness::new();
    let mut admitted = admission(install(), EXTENSION);
    admitted.generation = ExtensionRuntimeGeneration::INITIAL
        .next()
        .expect("generation");
    admitted.externally_connectable =
        Some(ExternallyConnectable::parse(&["https://a.test/allowed/*"]).expect("external"));
    h.core.admit(admitted).expect("admit external");
    let plan = AttachPlan {
        external: vec![ExternalRule {
            install_id: format!("{:032x}", 10),
            matches: vec![MatchRule {
                matcher: ContentScriptMatcher::new(
                    &["https://a.test/allowed/*"],
                    &[],
                    &[],
                    &[],
                    Default::default(),
                )
                .expect("matcher"),
                all_frames: true,
                file_access: false,
            }],
            bootstrap: "void 0".into(),
        }],
        ..Default::default()
    };
    let (id, generation) = h.tab(5, &plan);
    h.event(
        id,
        Event::Frames {
            generation,
            events: vec![FrameEvent::Created(info(
                1,
                1,
                "https://a.test/allowed/page",
                true,
            ))],
        },
    );
    let mut claim = post(generation, 1, 1, "", "https://a.test/allowed/page");
    claim.external_install = Some(format!("{:032x}", 10));
    assert!(senders(&h.post(id, claim.clone())).is_empty());
    assert_eq!(h.core.stats().buffered, 1);
    let waiting = h.event(
        id,
        Event::Frames {
            generation,
            events: vec![FrameEvent::ExternalPage {
                install_id: format!("{:032x}", 10),
                frame_id: 1,
                document_seq: 1,
            }],
        },
    );
    assert_eq!(
        senders(&waiting).len(),
        1,
        "first external post settles after its native marker"
    );
    let external_senders = senders(&h.post(id, claim));
    assert_eq!(external_senders.len(), 1);
    assert_eq!(external_senders[0].kind(), ContextKind::ExternalPage);
    h.event(
        id,
        Event::Frames {
            generation,
            events: vec![
                FrameEvent::Created(info(1, 2, "https://a.test/elsewhere", true)),
                FrameEvent::ExternalPage {
                    install_id: format!("{:032x}", 10),
                    frame_id: 1,
                    document_seq: 2,
                },
            ],
        },
    );
    let mut claim = post(generation, 1, 2, "", "https://a.test/elsewhere");
    claim.external_install = Some(format!("{:032x}", 10));
    assert!(senders(&h.post(id, claim)).is_empty());
}

fn script_table(generation: u64) -> zephium_webkit_web_process_protocol::protocol::ScriptTable {
    use zephium_webkit_web_process_protocol::protocol::{
        RunAt, ScriptEntry, ScriptFile, ScriptTable,
    };
    ScriptTable {
        generation,
        entries: vec![ScriptEntry {
            install_id: format!("{:032x}", 10),
            script_id: "manifest-0".into(),
            world: world(),
            run_at: RunAt::DocumentStart,
            rule: MatchRule::for_origin(SITE).expect("rule"),
            files: vec![ScriptFile {
                source: "void 0".into(),
                url: format!("chrome-extension://{EXTENSION}/content.js"),
            }],
            css: Vec::new(),
            bootstrap: String::new(),
        }],
    }
}

#[test]
fn document_snapshot_claims_are_checked_against_sent_history_without_ack_races() {
    use super::scripts::{ScriptTableError, SealedScripts};
    use std::rc::Rc;
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.core
        .publish_scripts(Rc::new(SealedScripts::new(script_table(7)).expect("seal")))
        .expect("publish");
    h.core
        .publish_scripts(Rc::new(SealedScripts::new(script_table(8)).expect("seal")))
        .expect("publish");
    h.core.take_effects();
    // A document pinned 7 before publication 8, and its Created arrives after
    // the newer ACK. Its exact sent snapshot remains admissible.
    h.core.scripts_settled(id, 8, true);
    let mut facts = info(1, 1, "https://a.test/", true);
    facts.table_generation = 7;
    h.event(
        id,
        Event::Frames {
            generation,
            events: vec![FrameEvent::Created(facts.clone())],
        },
    );
    let evaluation = Evaluation {
        install_id: format!("{:032x}", 10),
        script_id: "manifest-0".into(),
        world: world(),
        frame_id: 1,
        document_seq: 1,
        table_generation: 7,
        file_index: 0,
        success: true,
    };
    h.event(
        id,
        Event::Frames {
            generation,
            events: vec![FrameEvent::Evaluated(evaluation)],
        },
    );
    assert_eq!(
        senders(&h.post(id, post(generation, 1, 1, &world(), "https://a.test/"))).len(),
        1
    );
    facts.document_seq = 2;
    facts.table_generation = 99;
    h.event(
        id,
        Event::Frames {
            generation,
            events: vec![FrameEvent::Created(facts)],
        },
    );
    assert_eq!(h.core.stats().frame_records_refused, 1);
    let mut foreign = script_table(9);
    foreign.entries[0].files[0].url = format!("chrome-extension://{OTHER}/content.js");
    assert!(matches!(
        h.core
            .publish_scripts(Rc::new(SealedScripts::new(foreign).expect("seal"))),
        Err(ScriptTableError::InvalidOwner)
    ));
}

#[test]
fn pending_programmatic_injections_verify_scope_and_settle_timeout() {
    use zephium_webkit_web_process_protocol::protocol::{
        InjectRequest, InjectionKind, InjectionTarget,
    };
    let mut h = Harness::new();
    let (id, generation) = h.tab(5, &tab_plan(&[SITE]));
    h.page(id, generation, "https://a.test/");
    let request = InjectRequest {
        install_id: String::new(),
        token: String::new(),
        generation: 0,
        request_id: 0,
        target: InjectionTarget::Frame {
            frame_id: 1,
            document_seq: 1,
        },
        world: world(),
        kind: InjectionKind::Function,
        source: "() => Promise.resolve(7)".into(),
        source_url: format!("chrome-extension://{EXTENSION}/execute.js"),
        args_json: "[]".into(),
        timeout_ms: 10,
    };
    let id_request = h
        .core
        .inject(id, install(), request.clone(), h.now)
        .expect("inject");
    assert!(h
        .core
        .inject(id, other_install(), request.clone(), h.now)
        .is_err());
    let mut stale = request.clone();
    stale.target = InjectionTarget::Frame {
        frame_id: 1,
        document_seq: 2,
    };
    assert!(h.core.inject(id, install(), stale, h.now).is_err());
    h.core.tick(1010);
    let results = h.core.take_effects();
    assert!(results.iter().any(|effect| matches!(effect, Effect::Injection { result, .. } if result.request_id == id_request && result.status == zephium_webkit_web_process_protocol::protocol::InjectionStatus::TimedOut)));
    assert!(h.core.inject(id, install(), request, 1011).is_ok());
}
