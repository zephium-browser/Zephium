//! Permissions of the APIs the browser implements itself. WebKit drops
//! permissions it doesn't know from the manifest and never grants them, so
//! these grants live here.

use std::collections::BTreeSet;
use std::rc::Rc;

use objc2_web_kit::{WKWebExtension, WKWebExtensionContext};
use serde_json::{json, Value};

use crate::runtime::{Grants, Shared};
use crate::{events, json};

/// Kept in sync with `BROWSER_PERMISSIONS` in the compatibility layer.
pub const BROWSER_PERMISSIONS: &[&str] = &[
    "bookmarks",
    "downloads",
    "downloads.open",
    "history",
    "idle",
    "notifications",
    "privacy",
    "sessions",
    "topSites",
    "webRequestAuthProvider",
];

/// Native-app access an extension declares optional is granted here too:
/// WebKit holds `nativeMessaging` for the browser's own bridges, so only the
/// browser knows whether outside applications were allowed.
const NATIVE: &str = "nativeMessaging";

pub(crate) fn is_browser_permission(name: &str) -> bool {
    BROWSER_PERMISSIONS.contains(&name)
}

fn managed(name: &str) -> bool {
    is_browser_permission(name) || name == NATIVE
}

/// What the manifest asks for among the browser's permissions, required and
/// optional.
pub(crate) fn declared(extension: &WKWebExtension) -> (Vec<String>, Vec<String>) {
    let manifest = unsafe { extension.manifest() };
    let manifest = json::from_object(Some(&manifest));
    let list = |key: &str, keep: fn(&str) -> bool| -> Vec<String> {
        manifest
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|name| keep(name))
            .map(str::to_owned)
            .collect()
    };
    // The prepared manifest lists nativeMessaging as required for every
    // package; only its optional declaration is the extension's own.
    (
        list("permissions", is_browser_permission),
        list("optional_permissions", managed),
    )
}

pub(crate) fn initial(extension: &WKWebExtension, grants: &Grants) -> BTreeSet<String> {
    match grants {
        Grants::Requested => declared(extension).0.into_iter().collect(),
        Grants::Explicit { permissions, .. } => permissions
            .iter()
            .filter(|name| is_browser_permission(name))
            .cloned()
            .collect(),
    }
}

pub(crate) fn granted(shared: &Shared, extension: &str, permission: &str) -> bool {
    shared.loaded.borrow().get(extension).is_some_and(|loaded| {
        if permission == NATIVE {
            loaded.external_native_messaging.get()
        } else {
            loaded.browser_permissions.borrow().contains(permission)
        }
    })
}

pub(crate) fn extension_id(context: &WKWebExtensionContext) -> String {
    unsafe { context.uniqueIdentifier() }.to_string()
}

fn requested(message: &Value) -> Vec<String> {
    message
        .get("permissions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| managed(name))
        .take(BROWSER_PERMISSIONS.len() + 1)
        .map(str::to_owned)
        .collect()
}

/// `{ granted, optional }` for the compatibility layer's permission calls.
pub(crate) fn state(shared: &Shared, context: &WKWebExtensionContext) -> Value {
    let id = extension_id(context);
    let granted: Vec<String> = shared
        .loaded
        .borrow()
        .get(&id)
        .map(|loaded| {
            let mut granted: Vec<String> = loaded
                .browser_permissions
                .borrow()
                .iter()
                .cloned()
                .collect();
            if loaded.external_native_messaging.get() {
                granted.push(NATIVE.to_owned());
            }
            granted
        })
        .unwrap_or_default();
    let (required, optional) = declared(&*unsafe { context.webExtension() });
    json!({ "granted": granted, "declared": required.into_iter().chain(optional).collect::<Vec<_>>() })
}

thread_local! {
    /// Extensions with a permission prompt waiting on the person.
    static ASKING: std::cell::RefCell<std::collections::HashSet<String>> = Default::default();
}

/// Lets an unloaded extension ask again once it is back.
pub(crate) fn forget(extension: &str) {
    ASKING.with(|asking| asking.borrow_mut().remove(extension));
}

/// `permissions.request` for browser permissions: only ones the manifest
/// declares, asked through the same prompt as WebKit's own.
pub(crate) fn request(
    shared: &Rc<Shared>,
    context: &WKWebExtensionContext,
    message: &Value,
    done: Box<dyn FnOnce(Result<Value, String>)>,
) {
    let id = extension_id(context);
    let wanted = requested(message);
    let (required, optional) = declared(&*unsafe { context.webExtension() });
    if let Some(name) = wanted
        .iter()
        .find(|name| !required.contains(name) && !optional.contains(name))
    {
        return done(Err(format!(
            "Only permissions specified in the manifest may be requested. '{name}' was not."
        )));
    }
    let missing: Vec<String> = wanted
        .into_iter()
        .filter(|name| !granted(shared, &id, name))
        .collect();
    if missing.is_empty() {
        return done(Ok(Value::Bool(true)));
    }
    // Outside applications only for a package whose publisher was verified.
    let grantable = shared
        .loaded
        .borrow()
        .get(&id)
        .is_some_and(|loaded| loaded.native_messaging_grantable);
    if missing.iter().any(|name| name == NATIVE) && !grantable {
        return done(Ok(Value::Bool(false)));
    }
    // One question at a time: an extension can't stack prompts behind the
    // one the person is answering.
    if !ASKING.with(|asking| asking.borrow_mut().insert(id.clone())) {
        return done(Err(
            "A permission request is already waiting for an answer.".into(),
        ));
    }
    let weak = Rc::downgrade(shared);
    let extension = id.clone();
    crate::access::request(
        shared,
        &id,
        missing.clone(),
        Vec::new(),
        Box::new(move |allowed| {
            ASKING.with(|asking| asking.borrow_mut().remove(&extension));
            if allowed {
                if let Some(shared) = weak.upgrade() {
                    grant(&shared, &extension, &missing);
                }
            }
            done(Ok(Value::Bool(allowed)));
        }),
    );
}

fn grant(shared: &Rc<Shared>, extension: &str, permissions: &[String]) {
    let added: Vec<String> = match shared.loaded.borrow().get(extension) {
        Some(loaded) => {
            let mut set = loaded.browser_permissions.borrow_mut();
            permissions
                .iter()
                .filter(|name| {
                    if name.as_str() == NATIVE {
                        !loaded.external_native_messaging.replace(true)
                    } else {
                        set.insert((*name).clone())
                    }
                })
                .cloned()
                .collect()
        }
        None => return,
    };
    if !added.is_empty() {
        crate::data::refresh_interest(shared);
        events::emit(
            shared,
            extension,
            "permissions.onAdded",
            json!([{ "permissions": added, "origins": [] }]),
            None,
        );
    }
}

pub(crate) fn remove(
    shared: &Rc<Shared>,
    context: &WKWebExtensionContext,
    message: &Value,
) -> Value {
    let id = extension_id(context);
    let wanted = requested(message);
    let (required, _) = declared(&*unsafe { context.webExtension() });
    if wanted.iter().any(|name| required.contains(name)) {
        return Value::Bool(false);
    }
    let removed: Vec<String> = match shared.loaded.borrow().get(&id) {
        Some(loaded) => {
            let mut set = loaded.browser_permissions.borrow_mut();
            wanted
                .into_iter()
                .filter(|name| {
                    if name == NATIVE {
                        loaded.external_native_messaging.replace(false)
                    } else {
                        set.remove(name)
                    }
                })
                .collect()
        }
        None => Vec::new(),
    };
    if removed.iter().any(|name| name == NATIVE) {
        crate::native::close_extension(shared, &id);
    }
    if !removed.is_empty() {
        crate::data::refresh_interest(shared);
        shared.host().access_removed(&id, &removed, &[]);
        crate::privacy::forget_if_revoked(shared, &id);
        events::emit(
            shared,
            &id,
            "permissions.onRemoved",
            json!([{ "permissions": removed, "origins": [] }]),
            None,
        );
    }
    Value::Bool(true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_compatibility_layer_lists_the_same_permissions() {
        let names: Vec<String> = super::BROWSER_PERMISSIONS
            .iter()
            .map(|name| format!("{name:?}"))
            .collect();
        let line = format!(
            "const BROWSER_PERMISSIONS = new Set([{}]);",
            names.join(", ")
        );
        assert!(crate::compat::SCRIPT.contains(&line), "{line}");
    }
}
