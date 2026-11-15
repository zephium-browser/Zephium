use super::*;
use serde_json::json;
use webkit2gtk::{URIRequestExt, URIResponseExt, WebResourceExt};
use zephium_webext_runtime::api::web_request::Observation;

const MAX_RESOURCES: usize = 1024;

pub(super) struct Watch {
    view: glib::WeakRef<webkit2gtk::WebView>,
    started: Option<glib::SignalHandlerId>,
    resources: Rc<RefCell<HashMap<u64, Resource>>>,
}
struct Resource {
    resource: glib::WeakRef<webkit2gtk::WebResource>,
    handlers: Vec<glib::SignalHandlerId>,
}
impl Drop for Resource {
    fn drop(&mut self) {
        if let Some(resource) = self.resource.upgrade() {
            for handler in self.handlers.drain(..) {
                resource.disconnect(handler);
            }
        }
    }
}
#[cfg(feature = "native-tests")]
impl Watch {
    pub(super) fn handlers(&self) -> usize {
        usize::from(self.started.is_some())
            + self
                .resources
                .borrow()
                .values()
                .map(|resource| resource.handlers.len())
                .sum::<usize>()
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        if let (Some(view), Some(handler)) = (self.view.upgrade(), self.started.take()) {
            view.disconnect(handler);
        }
        self.resources.borrow_mut().clear();
    }
}

impl Inner {
    pub(super) fn update_request_observation(self: &Rc<Self>) {
        let wanted = self
            .bridge
            .with_runtime(|host, _| host.request_interest())
            .unwrap_or(false);
        if self.observing_requests.replace(wanted) == wanted {
            return;
        }
        for (id, entry) in self.tabs.borrow_mut().iter_mut() {
            entry.requests.take();
            if wanted {
                if let (Some(view), Some(tab)) =
                    (entry.view.upgrade(), self.network_tabs.borrow().get(id))
                {
                    entry.requests = Some(attach(self, *tab, &view));
                }
            }
        }
    }
}

fn interested(inner: &Weak<Inner>) -> Option<Rc<Inner>> {
    inner
        .upgrade()
        .filter(|inner| !inner.retired.get() && inner.observing_requests.get())
}
fn deliver(inner: &Weak<Inner>, tab: ItemId, observation: &Observation) {
    if let Some(inner) = interested(inner) {
        let _ = inner
            .bridge
            .with_runtime(|host, out| host.request_observed(tab, observation, out));
        inner.schedule();
    }
}
fn now() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
fn headers(headers: Option<soup::MessageHeaders>) -> Vec<(String, String)> {
    let mut values = Vec::new();
    let mut bytes = 0;
    if let Some(headers) = headers {
        headers.foreach(|name, value| {
            bytes += name.len() + value.len();
            if values.len() < 128 && bytes <= 64 * 1024 {
                values.push((name.to_owned(), value.to_owned()));
            }
        });
    }
    values
}
fn response(observation: &mut Observation, response: &webkit2gtk::URIResponse) {
    if let Some(url) = response.uri().and_then(|uri| url::Url::parse(&uri).ok()) {
        observation.url = url;
    }
    observation.details["statusCode"] = response.status_code().into();
    observation.details["timeStamp"] = now().into();
    observation.headers = headers(response.http_headers());
}

pub(super) fn attach(inner: &Rc<Inner>, tab: ItemId, view: &webkit2gtk::WebView) -> Watch {
    let weak = Rc::downgrade(inner);
    let resources: Rc<RefCell<HashMap<u64, Resource>>> = Rc::default();
    let pending = Rc::downgrade(&resources);
    let started = view.connect_resource_load_started(move |view, resource, request| {
        let (Some(inner), Some(pending)) = (interested(&weak), pending.upgrade()) else { return; };
        if pending.borrow().len() >= MAX_RESOURCES { return; }
        let Some(url) = request.uri().filter(|uri| uri.len() <= 8192).and_then(|uri| url::Url::parse(&uri).ok()).filter(|url| matches!(url.scheme(), "http" | "https")) else { return; };
        let main = view.main_resource().as_ref() == Some(resource);
        let top_level = if main { Some(url.clone()) } else { inner.browser.sender_tab(tab).and_then(|tab| tab["url"].as_str().and_then(|url| url::Url::parse(url).ok())) };
        let mut observation = Observation {
            event: "webRequest.onBeforeRequest", url, initiator: None, top_level,
            details: json!({"type":if main { "main_frame" } else { "other" },"frameId":if main { 0 } else { -1 },"parentFrameId":-1}), headers: Vec::new(),
        };
        if !inner.bridge.with_runtime(|host, _| host.request_matches(tab, &observation)).unwrap_or(false) { return; }
        let Some(id) = inner.next_request.get().checked_add(1) else { return; };
        inner.next_request.set(id);
        observation.details["requestId"] = format!("ui-{id}").into();
        observation.details["method"] = request.http_method().map(|method| method.to_string()).unwrap_or_default().into();
        observation.details["timeStamp"] = now().into();
        deliver(&weak, tab, &observation);
        observation.event = "webRequest.onBeforeSendHeaders";
        observation.headers = headers(request.http_headers());
        deliver(&weak, tab, &observation);
        observation.headers.clear();
        let state = Rc::new(RefCell::new(observation));
        let mut handlers = Vec::new();
        let (state_response, weak_response) = (state.clone(), weak.clone());
        handlers.push(resource.connect_response_notify(move |resource| {
            if interested(&weak_response).is_none() { return; }
            let Some(native) = resource.response() else { return; };
            let mut observation = state_response.borrow_mut();
            response(&mut observation, &native);
            observation.event = "webRequest.onHeadersReceived";
            deliver(&weak_response, tab, &observation);
        }));
        let (state_redirect, weak_redirect) = (state.clone(), weak.clone());
        handlers.push(resource.connect_local("sent-request", false, move |values| {
            interested(&weak_redirect)?;
            let request = values.get(1)?.get::<webkit2gtk::URIRequest>().ok()?;
            let redirected = values.get(2)?.get::<Option<webkit2gtk::URIResponse>>().ok()??;
            let next = request.uri().filter(|uri| uri.len() <= 8192).and_then(|uri| url::Url::parse(&uri).ok())?;
            let mut observation = state_redirect.borrow_mut();
            response(&mut observation, &redirected);
            observation.event = "webRequest.onBeforeRedirect";
            observation.details["redirectUrl"] = next.as_str().into();
            deliver(&weak_redirect, tab, &observation);
            observation.url = next;
            observation.details.as_object_mut()?.remove("redirectUrl");
            observation.details.as_object_mut()?.remove("statusCode");
            observation.details["method"] = request.http_method().map(|method| method.to_string()).unwrap_or_default().into();
            observation.headers.clear();
            observation.event = "webRequest.onBeforeRequest";
            deliver(&weak_redirect, tab, &observation);
            observation.event = "webRequest.onBeforeSendHeaders";
            observation.headers = headers(request.http_headers());
            deliver(&weak_redirect, tab, &observation);
            observation.headers.clear();
            None
        }));
        let (state_failed, weak_failed, pending_failed) = (state.clone(), weak.clone(), Rc::downgrade(&pending));
        handlers.push(resource.connect_failed(move |_, _| {
            if let Some(pending) = pending_failed.upgrade() {
                let mut observation = state_failed.borrow_mut();
                observation.event = "webRequest.onErrorOccurred";
                observation.details["error"] = "net::ERR_FAILED".into();
                observation.details["timeStamp"] = now().into();
                observation.headers.clear();
                deliver(&weak_failed, tab, &observation);
                pending.borrow_mut().remove(&id);
            }
        }));
        let (weak_finished, pending_finished) = (weak.clone(), Rc::downgrade(&pending));
        handlers.push(resource.connect_finished(move |_| {
            if let Some(pending) = pending_finished.upgrade() {
                let mut observation = state.borrow_mut();
                observation.event = "webRequest.onCompleted";
                observation.details["timeStamp"] = now().into();
                deliver(&weak_finished, tab, &observation);
                pending.borrow_mut().remove(&id);
            }
        }));
        pending.borrow_mut().insert(id, Resource { resource: resource.downgrade(), handlers });
    });
    Watch {
        view: view.downgrade(),
        started: Some(started),
        resources,
    }
}
