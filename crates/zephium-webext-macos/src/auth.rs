//! `webRequest.onAuthRequired` answered by extensions that hold
//! `webRequestAuthProvider`: a site's sign-in challenge goes to each one that
//! listens and may read the site, in turn, before the browser asks the
//! person. WebKit's own webRequest can only watch.

use std::rc::Rc;

use objc2_foundation::{NSString, NSURL};
use serde_json::{json, Value};

use crate::runtime::Shared;
use crate::{api, events};

pub(crate) const EVENT: &str = "webRequest.onAuthRequired";
const PERMISSION: &str = "webRequestAuthProvider";

#[derive(Clone, Debug)]
pub struct AuthDetails {
    /// Unique per challenge; extensions use it to avoid answering twice.
    pub request_id: String,
    pub url: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub proxy: bool,
    pub tab: Option<u64>,
}

pub enum AuthAnswer {
    Credentials { username: String, password: String },
    Cancel,
}

pub(crate) fn ask(
    shared: &Rc<Shared>,
    details: AuthDetails,
    done: Box<dyn FnOnce(Option<AuthAnswer>)>,
) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&details.url)) else {
        return done(None);
    };
    let candidates: Vec<String> = shared
        .loaded
        .borrow()
        .iter()
        .filter(|(id, loaded)| {
            api::granted(shared, id, PERMISSION)
                && events::listened(shared, id, EVENT)
                && unsafe { loaded.context.hasAccessToURL(&url) }
        })
        .map(|(id, _)| id.clone())
        .collect();
    next(Rc::downgrade(shared), details, candidates, done);
}

fn next(
    shared: std::rc::Weak<Shared>,
    details: AuthDetails,
    mut candidates: Vec<String>,
    done: Box<dyn FnOnce(Option<AuthAnswer>)>,
) {
    let Some(shared_now) = shared.upgrade() else {
        return done(None);
    };
    if candidates.is_empty() {
        return done(None);
    }
    let extension = candidates.remove(0);
    let args = json!([{
        "requestId": details.request_id,
        "url": details.url,
        "method": "GET",
        "frameId": 0,
        "parentFrameId": -1,
        "tabId": details.tab.map_or(-1, |tab| tab as i64),
        "type": "main_frame",
        "timeStamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0),
        "scheme": details.scheme,
        "challenger": { "host": details.host, "port": details.port },
        "isProxy": details.proxy,
        "statusCode": if details.proxy { 407 } else { 401 },
        "statusLine": if details.proxy {
            "HTTP/1.1 407 Proxy Authentication Required"
        } else {
            "HTTP/1.1 401 Unauthorized"
        },
    }]);
    let followup = details.clone();
    events::emit(
        &shared_now,
        &extension,
        EVENT,
        args,
        Some(Box::new(move |answer| match parse(answer) {
            Some(answer) => done(Some(answer)),
            None => next(shared, followup, candidates, done),
        })),
    );
}

fn parse(answer: Option<Value>) -> Option<AuthAnswer> {
    let answer = answer?;
    if answer.get("cancel").and_then(Value::as_bool) == Some(true) {
        return Some(AuthAnswer::Cancel);
    }
    let credentials = answer.get("authCredentials")?;
    let field = |key: &str| {
        credentials
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Some(AuthAnswer::Credentials {
        username: field("username")?,
        password: field("password").unwrap_or_default(),
    })
}
