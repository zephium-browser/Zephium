use super::*;
use serde_json::json;
use webkit2gtk::{URIRequestExt, URIResponseExt, WebResourceExt};
use zephium_webext_runtime::api::web_request::Observation;

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
            let Some(view) = entry.view.upgrade() else {
                continue;
            };
            if let Some(handler) = entry.requests.take() {
                view.disconnect(handler);
            }
            if wanted {
                if let Some(tab) = self.network_tabs.borrow().get(id) {
                    entry.requests = Some(attach(self, *tab, &view));
                }
            }
        }
    }
}

fn interested(inner: &Weak<Inner>) -> Option<Rc<Inner>> {
    inner.upgrade().filter(|inner| {
        !inner.retired.get()
            && inner
                .bridge
                .with_runtime(|host, _| host.request_interest())
                .unwrap_or(false)
    })
}

fn deliver(inner: &Weak<Inner>, tab: ItemId, observation: &Observation) {
    if let Some(inner) = interested(inner) {
        let _ = inner
            .bridge
            .with_runtime(|host, out| host.request_observed(tab, observation, out));
    }
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

pub(super) fn attach(
    inner: &Rc<Inner>,
    tab: ItemId,
    view: &webkit2gtk::WebView,
) -> glib::SignalHandlerId {
    let weak = Rc::downgrade(inner);
    view.connect_resource_load_started(move |view, resource, request| {
        let Some(inner) = interested(&weak) else {
            return;
        };
        // WebKit exposes no resource-to-frame join; only the main resource has a proven frame.
        if view.main_resource().as_ref() != Some(resource) {
            return;
        }
        let Some(url) = request
            .uri()
            .and_then(|uri| url::Url::parse(&uri).ok())
            .filter(|url| matches!(url.scheme(), "http" | "https"))
        else {
            return;
        };
        let id = inner.next_request.get().saturating_add(1);
        inner.next_request.set(id);
        let now = || {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                * 1000.0
        };
        let mut observation = Observation {
            event: "webRequest.onBeforeRequest",
            url,
            initiator: None,
            details: json!({
                "requestId":format!("ui-{id}"),
                "method":request.http_method().map(|method| method.to_string()).unwrap_or_default(),
                "type":"main_frame", "frameId":0, "parentFrameId":-1, "timeStamp":now()
            }),
            headers: Vec::new(),
        };
        deliver(&weak, tab, &observation);
        if interested(&weak).is_none() {
            return;
        }
        observation.event = "webRequest.onBeforeSendHeaders";
        observation.headers = headers(request.http_headers());
        deliver(&weak, tab, &observation);
        observation.headers.clear();
        let state = Rc::new(RefCell::new(observation));
        let done = Rc::new(Cell::new(false));
        let (state_response, weak_response) = (state.clone(), weak.clone());
        resource.connect_response_notify(move |resource| {
            if interested(&weak_response).is_none() {
                return;
            }
            let Some(response) = resource.response() else {
                return;
            };
            let mut observation = state_response.borrow_mut();
            if let Some(url) = response.uri().and_then(|uri| url::Url::parse(&uri).ok()) {
                observation.url = url;
            }
            observation.event = "webRequest.onHeadersReceived";
            observation.details["statusCode"] = response.status_code().into();
            observation.details["timeStamp"] = now().into();
            observation.headers = headers(response.http_headers());
            deliver(&weak_response, tab, &observation);
        });
        let (state_failed, weak_failed, failed) = (state.clone(), weak.clone(), done.clone());
        resource.connect_failed(move |_, _| {
            if failed.replace(true) || interested(&weak_failed).is_none() {
                return;
            }
            let mut observation = state_failed.borrow_mut();
            observation.event = "webRequest.onErrorOccurred";
            observation.details["error"] = "net::ERR_FAILED".into();
            observation.details["timeStamp"] = now().into();
            observation.headers.clear();
            deliver(&weak_failed, tab, &observation);
        });
        let weak_finished = weak.clone();
        resource.connect_finished(move |_| {
            if done.replace(true) || interested(&weak_finished).is_none() {
                return;
            }
            let mut observation = state.borrow_mut();
            observation.event = "webRequest.onCompleted";
            observation.details["timeStamp"] = now().into();
            deliver(&weak_finished, tab, &observation);
        });
    })
}
