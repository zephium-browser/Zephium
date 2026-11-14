//! The runtime host's side of the Linux browser port: tab messaging,
//! settlements from the engine, tab events and toolbar invocations.

use std::path::PathBuf;

use serde_json::{json, Value};
use zephium_core::extensions::ExtensionBrowserSurface;
use zephium_core::ids::{ExtensionInstallId, ItemId};
use zephium_webext_runtime::api::action::Activation as ActionActivation;
use zephium_webext_runtime::api::web_navigation::{self, NavigationEvent, NavigationObservation};
use zephium_webext_runtime::api::{permissions as permissions_api, tabs};
use zephium_webext_runtime::context::{ContextKind, DocumentGeneration, FrameId, Sender};
use zephium_webext_runtime::dispatch::Responder;
use zephium_webext_runtime::errors::ApiError;
use zephium_webext_runtime::events::{EventFacts, EventImportance};
use zephium_webext_runtime::lifecycle::StartReason;
use zephium_webext_runtime::pattern::ContentScriptMatcher;
use zephium_webext_runtime::permissions::SiteAccess;
use zephium_webext_runtime::ports::{no_receiver, MessageListener, MessageRoute};
use zephium_webext_runtime::scripting_registry::RegisteredScript;

use super::{At, Finished, RuntimeHost, TabEvent, TabRoute};
use crate::bridge::Outbox;
use crate::runtime::browser::{BrowserSettlement, Shared};

/// What the GTK side needs to publish one install's content scripts.
pub(crate) struct ContentInstall {
    pub install: ExtensionInstallId,
    pub extension_id: String,
    pub root: PathBuf,
    pub scripts: Vec<(RegisteredScript, ContentScriptMatcher)>,
    pub site_access: SiteAccess,
    pub origins: Vec<String>,
    pub file_access: bool,
    pub bootstrap: Value,
}

impl RuntimeHost {
    pub(crate) fn browser(&self) -> std::rc::Rc<Shared> {
        self.browser.clone()
    }

    /// WebKit CORS-allowlist patterns for the install's granted hosts.
    pub(crate) fn cors_patterns(&self, install: ExtensionInstallId) -> Vec<String> {
        let Some(permissions) = self.permissions.get(&install) else {
            return Vec::new();
        };
        let mut patterns: Vec<String> = permissions
            .snapshot()
            .origins
            .into_iter()
            .flat_map(|origin| {
                if origin == "<all_urls>" {
                    vec!["http://*/*".to_owned(), "https://*/*".to_owned()]
                } else if let Some(rest) = origin.strip_prefix("*://") {
                    vec![format!("http://{rest}"), format!("https://{rest}")]
                } else if origin.starts_with("http://") || origin.starts_with("https://") {
                    vec![origin]
                } else {
                    Vec::new()
                }
            })
            .collect();
        patterns.sort();
        patterns.dedup();
        patterns
    }

    pub(crate) fn take_pending_out(&mut self, out: &mut Outbox) {
        out.frames.append(&mut self.pending_out);
    }

    pub(crate) fn content_installs(&self) -> Vec<ContentInstall> {
        let mut installs: Vec<ContentInstall> = self
            .state
            .installs
            .iter()
            .filter_map(|(install, state)| {
                let permissions = self.permissions.get(install)?;
                Some(ContentInstall {
                    install: *install,
                    extension_id: state.extension_id.as_str().to_owned(),
                    root: state.root.clone(),
                    scripts: state
                        .scripts
                        .declarations()
                        .map(|injection| (injection.script.clone(), injection.matcher.clone()))
                        .collect(),
                    site_access: permissions.site_access().clone(),
                    origins: permissions.snapshot().origins,
                    file_access: permissions.file_access(),
                    bootstrap: state.bootstrap(
                        ContextKind::ContentScript,
                        &self.state.ui_language,
                        self.state.incognito,
                    ),
                })
            })
            .collect();
        installs.sort_by_key(|install| install.install);
        installs
    }

    pub(crate) fn finish(&mut self, finished: Finished, out: &mut Outbox) {
        match finished {
            Finished::Reply(responder, result) => self.settle(responder, result, out),
            Finished::Grant {
                responder,
                install,
                request,
            } => {
                let granted = self
                    .permissions
                    .get_mut(&install)
                    .ok_or_else(ApiError::context_invalidated)
                    .and_then(|permissions| permissions.grant(&request));
                match granted {
                    Ok(_) => {
                        self.settle(responder, Ok(Value::Bool(true)), out);
                        self.emit(
                            install,
                            "permissions.onAdded",
                            &json!([permissions_api::to_value(&request)]),
                            StartReason::Event,
                            out,
                        );
                        self.op(super::NativeOp::ScriptsChanged);
                    }
                    Err(error) => self.settle(responder, Err(error), out),
                }
            }
        }
        self.process_work(out);
        self.settle_blockers(out);
    }

    pub(crate) fn settle_browser(
        &mut self,
        request: u64,
        settlement: BrowserSettlement,
        out: &mut Outbox,
    ) {
        self.tick_clock();
        if let Some(finished) = self.browser.settle(request, settlement) {
            self.finish(finished, out);
        }
    }

    pub(crate) fn injection_failed(&mut self, pending: u64, frame: usize, out: &mut Outbox) {
        if let Some(finished) = self.browser.injection_failed(pending, frame) {
            self.finish(finished, out);
        }
    }

    pub(crate) fn publish_surface(&mut self, surface: ExtensionBrowserSurface, out: &mut Outbox) {
        self.tick_clock();
        let events = self.browser.publish_surface(surface);
        if events.is_empty() {
            return;
        }
        let installs: Vec<ExtensionInstallId> = self.state.installs.keys().copied().collect();
        for install in installs {
            let Some(permissions) = self.permissions.get(&install).cloned() else {
                continue;
            };
            for event in &events {
                let (name, args) = match event {
                    TabEvent::Created(tab) => {
                        ("tabs.onCreated", json!([tabs::project(tab, &permissions)]))
                    }
                    TabEvent::Removed { tab, window } => (
                        "tabs.onRemoved",
                        json!([tab, {"windowId": window, "isWindowClosing": false}]),
                    ),
                    TabEvent::Activated { tab, window } => (
                        "tabs.onActivated",
                        json!([{"tabId": tab, "windowId": window}]),
                    ),
                    TabEvent::Updated { previous, current } => {
                        let change = tabs::change_info(previous, current, &permissions);
                        if change.as_object().is_none_or(|change| change.is_empty()) {
                            continue;
                        }
                        (
                            "tabs.onUpdated",
                            json!([current.id, change, tabs::project(current, &permissions)]),
                        )
                    }
                };
                self.emit(install, name, &args, StartReason::Event, out);
            }
        }
        self.settle_blockers(out);
    }

    /// `webNavigation` events for one frame document, in order. Each live
    /// listening context gets them shaped and checked for its own install;
    /// a stopped background is not woken for them.
    pub(crate) fn navigation(
        &mut self,
        tab: ItemId,
        frame: FrameId,
        document: DocumentGeneration,
        events: &[(NavigationEvent, Option<&str>, &[&str])],
        out: &mut Outbox,
    ) {
        let Some((tab_id, record)) = self.browser.navigation_frame(tab, frame, document) else {
            return;
        };
        let port = crate::runtime::browser::Browser::new(self.browser.clone());
        let time_stamp = (self.unix_millis)() as f64;
        let installs: Vec<ExtensionInstallId> = self.state.installs.keys().copied().collect();
        for &(event, transition, qualifiers) in events {
            let observation = NavigationObservation {
                tab_id,
                frame: record.clone(),
                event,
                time_stamp,
                transition_type: transition.map(str::to_owned),
                transition_qualifiers: qualifiers.iter().map(|q| (*q).to_owned()).collect(),
                error: None,
            };
            let name = event.name();
            let facts = web_navigation::event_facts(&observation.frame, tab_id);
            for &install in &installs {
                let Some(permissions) = self
                    .permissions
                    .get(&install)
                    .filter(|permissions| permissions.has_api("webNavigation"))
                    .cloned()
                else {
                    continue;
                };
                if self
                    .state
                    .registry
                    .targets(install, name, &facts)
                    .is_empty()
                {
                    continue;
                }
                for key in self.state.registry.targets(install, name, &facts) {
                    let Some(sender) = self.router.contexts.get(&key).cloned() else {
                        continue;
                    };
                    let Ok(mut args) =
                        web_navigation::observation(&port, &sender, &permissions, &observation)
                    else {
                        continue;
                    };
                    let listeners: Vec<u64> = self
                        .state
                        .registry
                        .listeners(&sender, name, &facts)
                        .iter()
                        .map(|id| id.get())
                        .collect();
                    if let Some(args) = args.as_array_mut() {
                        args.push(json!(listeners));
                    }
                    if sender.kind() == ContextKind::Background {
                        if self.enqueue(install, name, &args, EventImportance::Droppable) {
                            self.observe(
                                super::Observation::Wake {
                                    install,
                                    reason: StartReason::Event,
                                },
                                out,
                            );
                        }
                    } else if let Some(frame) =
                        super::event_frame(sender.kind(), super::direct_sequence(), name, &args)
                    {
                        out.deliver(key, frame);
                    }
                }
            }
        }
    }

    /// The main frame of a tab view changed its address without a new
    /// document: history API or fragment navigation.
    pub(crate) fn same_document(
        &mut self,
        view: crate::bridge::BridgeViewId,
        url: url::Url,
        out: &mut Outbox,
    ) {
        self.tick_clock();
        let Some((tab, document, previous)) = self.browser.main_url_changed(view, url.clone())
        else {
            return;
        };
        let without = |mut url: url::Url| {
            url.set_fragment(None);
            url
        };
        let event = if without(previous) == without(url) {
            NavigationEvent::ReferenceFragmentUpdated
        } else {
            NavigationEvent::HistoryStateUpdated
        };
        self.navigation(
            tab,
            FrameId::MAIN,
            document,
            &[(event, Some("link"), &[])],
            out,
        );
        self.settle_blockers(out);
    }

    /// The person clicked the install's toolbar button on `tab`.
    pub(crate) fn invoke_action(
        &mut self,
        install: ExtensionInstallId,
        tab: ItemId,
        out: &mut Outbox,
    ) -> Result<ActionActivation, ApiError> {
        self.tick_clock();
        let permissions = self
            .permissions
            .get_mut(&install)
            .ok_or_else(ApiError::context_invalidated)?;
        let (activation, tab) = self.browser.invoke_action(install, tab, permissions)?;
        if activation == ActionActivation::Click {
            let args = json!([tabs::project(&tab, permissions)]);
            self.emit(install, "action.onClicked", &args, StartReason::Event, out);
        }
        self.op(super::NativeOp::ScriptsChanged);
        self.settle_blockers(out);
        Ok(activation)
    }

    fn tab_receivers(
        &self,
        sender: &Sender,
        route: &TabRoute,
    ) -> Result<(MessageRoute, Vec<Sender>), ApiError> {
        let targets: Vec<(ItemId, Option<FrameId>, Option<DocumentGeneration>)> = match route {
            TabRoute::Named {
                tab_id,
                frame_id,
                document_id,
            } => {
                let tab = self.browser.tab_item(*tab_id).ok_or_else(no_receiver)?;
                let document = match document_id {
                    Some(id) => Some(self.browser.document_of(tab, id).ok_or_else(no_receiver)?),
                    None => None,
                };
                vec![(tab, frame_id.map(FrameId::new), document)]
            }
            TabRoute::Resolved(routes) => routes
                .iter()
                .filter_map(|route| match route {
                    MessageRoute::Tab {
                        tab,
                        frame,
                        document,
                    } => Some((*tab, *frame, *document)),
                    _ => None,
                })
                .collect(),
        };
        let (tab, frame, document) = match targets.as_slice() {
            [] => return Err(no_receiver()),
            [single] => *single,
            [first, ..] => (first.0, None, None),
        };
        let receivers = self
            .router
            .contexts
            .values()
            .filter(|context| {
                context.install() == sender.install()
                    && matches!(context.kind(), ContextKind::ContentScript)
                    && targets.iter().any(|(tab, frame, document)| {
                        context.tab() == Some(*tab)
                            && frame.is_none_or(|frame| context.frame() == Some(frame))
                            && document.is_none_or(|document| context.document() == document)
                    })
            })
            .cloned()
            .collect();
        Ok((
            MessageRoute::Tab {
                tab,
                frame,
                document,
            },
            receivers,
        ))
    }

    pub(super) fn send_tab(
        &mut self,
        sender: Box<Sender>,
        payload: Vec<u8>,
        route: &TabRoute,
        responder: Responder,
        out: &mut Outbox,
    ) {
        let (route, receivers) = match self.tab_receivers(&sender, route) {
            Ok(found) => found,
            Err(error) => return self.settle(responder, Err(error), out),
        };
        let facts = EventFacts::default();
        let listeners: Vec<MessageListener> = receivers
            .iter()
            .flat_map(|context| {
                self.state
                    .registry
                    .listeners(context, "runtime.onMessage", &facts)
                    .into_iter()
                    .map(|listener| MessageListener {
                        sender: context.clone(),
                        listener,
                    })
            })
            .collect();
        let clock = At(self.state.now);
        let sent = self.router.messages.send(
            &sender,
            &route,
            &listeners,
            &payload,
            self.dispatcher.limiter_mut(),
            &clock,
        );
        match sent {
            Err(error) => self.settle(responder, Err(error), out),
            Ok((id, deliveries)) => {
                self.sends.insert(id.get(), (id, responder));
                for delivery in deliveries {
                    self.deliver_message(delivery, out);
                }
            }
        }
    }

    pub(super) fn connect_tab(
        &mut self,
        sender: Box<Sender>,
        name: &str,
        route: &TabRoute,
        responder: Responder,
        out: &mut Outbox,
    ) {
        let (route, receivers) = match self.tab_receivers(&sender, route) {
            Ok(found) => found,
            Err(error) => return self.settle(responder, Err(error), out),
        };
        let receivers: Vec<Sender> = receivers
            .into_iter()
            .filter(|context| {
                self.state
                    .registry
                    .has_listeners(context, "runtime.onConnect")
            })
            .collect();
        let (port, _) = match self.router.ports.begin_connect(&sender, route, name) {
            Ok(begun) => begun,
            Err(error) => return self.settle(responder, Err(error), out),
        };
        self.settle(responder, Ok(Value::String(port.get().to_string())), out);
        let clock = At(self.state.now);
        let limiter = self.dispatcher.limiter_mut();
        let actions = if receivers.is_empty() {
            self.router.ports.fail_connect(port, no_receiver(), limiter)
        } else {
            match self
                .router
                .ports
                .complete_connect(port, &receivers, limiter, &clock)
            {
                Ok(actions) => actions,
                Err(error) => self.router.ports.fail_connect(port, error, limiter),
            }
        };
        self.router.port_actions(actions, limiter, out);
    }
}
