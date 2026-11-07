//! Browser data WebKit has no API for, which lives in the browser's stores. Calls go to the host; changes
//! come back through [`changed`], and the host hears which changes anyone
//! listens to so it describes nothing unheard.

use std::rc::Rc;

use serde_json::{json, Value};
use zephium_core::extension_data::{
    ExtensionDataCall, ExtensionDataChange, ExtensionDataInterest, ExtensionDataReply,
};

use crate::runtime::Shared;
use crate::{api, bookmarks, downloads, events, history, sessions, top_sites};

pub(crate) type Done = Box<dyn FnOnce(Result<Value, String>)>;
pub(crate) type Answer = Box<dyn FnOnce(ExtensionDataReply) -> Result<Value, String>>;

/// What one call needs: its permission, the browser's request, and how the
/// browser's answer reads to the extension.
pub(crate) struct Call {
    pub(crate) permission: &'static str,
    pub(crate) request: ExtensionDataCall,
    pub(crate) answer: Answer,
}

pub(crate) fn handles(api: &str) -> bool {
    [
        "history.",
        "bookmarks.",
        "topSites.",
        "sessions.",
        "downloads.",
    ]
    .iter()
    .any(|namespace| api.starts_with(namespace))
}

pub(crate) fn request(
    shared: &Rc<Shared>,
    extension: &str,
    api: &str,
    message: &Value,
    done: Done,
) {
    if let Some(op) = api.strip_prefix("downloads.") {
        return downloads::request(shared, extension, op, message, done);
    }
    let call = match api.split_once('.') {
        Some(("history", op)) => history::call(op, message),
        Some(("bookmarks", op)) => bookmarks::call(op, message),
        Some(("topSites", op)) => top_sites::call(op, message),
        Some(("sessions", op)) => sessions::call(op, message),
        _ => Err(format!("{api} is not supported")),
    };
    let Call {
        permission,
        request,
        answer,
    } = match call {
        Ok(call) => call,
        Err(message) => return done(Err(message)),
    };
    if !api::granted(shared, extension, permission) {
        return done(Err(format!(
            "The extension doesn't have the '{permission}' permission."
        )));
    }
    let restoring = api == "sessions.restore";
    shared.host().browser_data(
        extension,
        request,
        Box::new(move |reply| {
            done(match reply {
                ExtensionDataReply::Unavailable if restoring => {
                    Err("No closed tab can be reopened in this window.".into())
                }
                ExtensionDataReply::Unavailable => {
                    Err("This profile keeps no such data, or it can't be reached now.".into())
                }
                ExtensionDataReply::Busy => Err("Too many requests are waiting; try again.".into()),
                reply => answer(reply),
            })
        }),
    );
}

/// The browser's data changed: listeners holding the permission hear it.
pub(crate) fn changed(shared: &Rc<Shared>, change: ExtensionDataChange) {
    let (permission, raised): (&str, Vec<(&str, Value)>) = match &change {
        ExtensionDataChange::Visited(entry) => (
            history::PERMISSION,
            vec![(history::ON_VISITED, json!([history::item(entry)]))],
        ),
        ExtensionDataChange::VisitsRemoved { urls, all } => (
            history::PERMISSION,
            vec![(
                history::ON_VISIT_REMOVED,
                json!([{ "allHistory": all, "urls": if *all { &[][..] } else { &urls[..] } }]),
            )],
        ),
        ExtensionDataChange::Bookmark(change) => (bookmarks::PERMISSION, bookmarks::events(change)),
        ExtensionDataChange::Sessions => (
            sessions::PERMISSION,
            vec![(sessions::ON_CHANGED, json!([]))],
        ),
    };
    let listening: Vec<String> = shared
        .loaded
        .borrow()
        .keys()
        .filter(|id| api::granted(shared, id, permission))
        .cloned()
        .collect();
    for id in listening {
        for (name, args) in &raised {
            if events::listened(shared, &id, name) {
                events::emit(shared, &id, name, args.clone(), None);
            }
        }
    }
}

fn listens(shared: &Shared, permission: &str, names: &[&str]) -> bool {
    shared.loaded.borrow().keys().any(|id| {
        api::granted(shared, id, permission)
            && names.iter().any(|name| events::listened(shared, id, name))
    })
}

/// Tells the host when what extensions listen to changes.
pub(crate) fn refresh_interest(shared: &Rc<Shared>) {
    downloads::refresh(shared);
    let interest = ExtensionDataInterest {
        history: listens(shared, history::PERMISSION, history::EVENTS),
        bookmarks: listens(shared, bookmarks::PERMISSION, bookmarks::EVENTS),
        sessions: listens(shared, sessions::PERMISSION, &[sessions::ON_CHANGED]),
    };
    if shared.data_interest.replace(interest) != interest {
        shared.host().data_interest(interest);
    }
}

/// A non-negative integer argument, as Chrome's numeric fields accept.
pub(crate) fn number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
}

pub(crate) fn text<'a>(message: &'a Value, key: &str) -> Result<&'a str, String> {
    message
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("'{key}' must be a string."))
}

pub(crate) fn unexpected(reply: ExtensionDataReply) -> Result<Value, String> {
    let _ = reply;
    Err("The browser answered with something unexpected.".into())
}
