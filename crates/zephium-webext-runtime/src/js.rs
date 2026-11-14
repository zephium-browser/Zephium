//! The engine-neutral JavaScript binding, as source text for adapters.
//!
//! [`BUNDLE`] is one function expression. An adapter evaluates
//! `(BUNDLE)(entry, surface, bootstrap)` where `surface` is the generated
//! API surface (`js/surface.generated.js`, one JSON value) and `bootstrap`
//! the context's JSON blob:
//!
//! - `"page"`: extension pages and offscreen documents, in the frame's default
//!   world right after the web-process module defined `__zephiumPost` and
//!   `__zephiumRegisterReceiver`, before any page script;
//! - `"content"`: once per document in the install's isolated world, before
//!   any content script; script bundles then call the guard it publishes;
//! - `"native-content"`: eagerly creates the content API after the native
//!   adapter has matched the document; each file then runs at top level;
//! - `"worker"`: first thing in the background worker, before the
//!   extension's service-worker script;
//! - `"owner"`: in the hidden owner page, with the worker start data as
//!   `bootstrap`.
//!
//! [`MATCHER`] is `(data) => boolean` for MAIN-world script bundles, which
//! get no API.

macro_rules! core_modules {
    () => {
        concat!(
            "const Z={__proto__:null};Z.I=(",
            include_str!("../js/intrinsics.js"),
            ")(globalThis);Z.errors=(",
            include_str!("../js/errors.js"),
            ")(Z);Z.transport=(",
            include_str!("../js/transport.js"),
            ")(Z);Z.events=(",
            include_str!("../js/events.js"),
            ")(Z);Z.ports=(",
            include_str!("../js/ports.js"),
            ")(Z);Z.api=(",
            include_str!("../js/api.js"),
            ")(Z);"
        )
    };
}

/// Every context's binding; see the module documentation for the entries.
pub const BUNDLE: &str = concat!(
    "(function zephiumWebExtension(entry,surface,bootstrap){'use strict';",
    core_modules!(),
    "Z.content=(",
    include_str!("../js/content.js"),
    ")(Z);Z.workerScope=(",
    include_str!("../js/worker_scope.js"),
    ")(Z);Z.ownerRelay=(",
    include_str!("../js/owner_relay.js"),
    ")(Z);",
    "switch(entry){",
    "case'page':return Z.api.page(surface,bootstrap);",
    "case'content':return Z.content.boot(surface,bootstrap);",
    "case'native-content':if(!bootstrap||!['contentScript','userScript'].includes(bootstrap.contextKind))throw new TypeError('Invalid native content context');return Z.api.page(surface,bootstrap);",
    "case'worker':return Z.workerScope.worker(surface,bootstrap);",
    "case'owner':return Z.ownerRelay.owner(bootstrap);",
    "default:throw new TypeError('Unknown binding entry');}})"
);

/// The hidden owner's relay alone; it creates no extension API or schema.
pub const OWNER_BUNDLE: &str = concat!(
    "(function zephiumWorkerOwner(bootstrap){'use strict';const Z={__proto__:null};Z.I=(",
    include_str!("../js/intrinsics.js"),
    ")(globalThis);Z.transport=(",
    include_str!("../js/transport.js"),
    ")(Z);Z.ownerRelay=(",
    include_str!("../js/owner_relay.js"),
    ")(Z);return Z.ownerRelay.owner(bootstrap);})"
);

/// The worker binding, without page/content matching or owner relay code.
pub const WORKER_BUNDLE: &str = concat!(
    "(function zephiumWorker(surface,bootstrap){'use strict';",
    core_modules!(),
    "Z.workerScope=(",
    include_str!("../js/worker_scope.js"),
    ")(Z);return Z.workerScope.worker(surface,bootstrap);})"
);

/// The content-script match guard alone, for MAIN-world bundles.
pub const MATCHER: &str = concat!(
    "(function zephiumMatcher(data){'use strict';const Z={__proto__:null};Z.I=(",
    include_str!("../js/intrinsics.js"),
    ")(globalThis);Z.content=(",
    include_str!("../js/content.js"),
    ")(Z);return Z.content.matches(data);})"
);

const SURFACE_SCRIPT: &str = include_str!("../js/surface.generated.js");

/// The generated API surface as one JSON value, the `surface` argument of
/// [`BUNDLE`] entries.
pub fn surface() -> &'static str {
    SURFACE_SCRIPT
        .split_once("Object.freeze(")
        .and_then(|(_, rest)| rest.trim_end().strip_suffix(");"))
        .unwrap_or("{\"version\":1,\"namespaces\":{}}")
}

/// Background metadata for the namespaces and members this manifest can use.
/// Optional permissions stay in the projection so an existing worker can gain
/// and lose them through the binding's normal permission refresh.
pub fn background_surface(manifest: &zephium_webext::manifest::Manifest) -> String {
    use std::sync::OnceLock;

    use serde_json::{json, Map, Value};

    static GENERATED: OnceLock<Value> = OnceLock::new();
    let generated = GENERATED
        .get_or_init(|| serde_json::from_str(surface()).expect("generated extension API metadata"));
    let permissions: std::collections::BTreeSet<_> = manifest
        .permissions()
        .into_iter()
        .chain(manifest.optional_permissions())
        .collect();
    let available = |node: &Value| {
        node.get("contexts")
            .and_then(Value::as_array)
            .is_none_or(|kinds| kinds.iter().any(|kind| kind == "background"))
            && node
                .get("permissions")
                .and_then(Value::as_array)
                .is_none_or(|required| {
                    required.is_empty()
                        || required.iter().any(|permission| {
                            permission
                                .as_str()
                                .is_some_and(|name| permissions.contains(name))
                        })
                })
            && node
                .get("manifest")
                .and_then(Value::as_array)
                .is_none_or(|features| {
                    features.iter().all(|feature| {
                        feature
                            .as_str()
                            .and_then(|name| manifest.raw().get(name))
                            .is_some_and(|value| !value.is_null())
                    })
                })
    };
    let mut namespaces = Map::new();
    if let Some(nodes) = generated["namespaces"].as_object() {
        for (name, node) in nodes {
            if !available(node) {
                continue;
            }
            let mut node = node.clone();
            for members in ["functions", "events"] {
                if let Some(members) = node.get_mut(members).and_then(Value::as_object_mut) {
                    members.retain(|_, spec| available(spec));
                }
            }
            namespaces.insert(name.clone(), node);
        }
    }
    // A nested namespace cannot be exposed without its parent.
    let names: std::collections::BTreeSet<_> = namespaces.keys().cloned().collect();
    namespaces.retain(|name, _| {
        name.match_indices('.')
            .all(|(index, _)| names.contains(&name[..index]))
    });
    json!({"version": generated["version"], "namespaces": namespaces}).to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use zephium_webext::manifest::Manifest;

    #[test]
    fn surface_is_the_generated_json_value() {
        let value: serde_json::Value = serde_json::from_str(super::surface()).unwrap();
        assert_eq!(value["version"], 1);
        assert!(value["namespaces"]["runtime"].is_object());
    }

    #[test]
    fn background_metadata_omits_unavailable_features_and_keeps_optional_grants() {
        let project = |raw| -> Value {
            serde_json::from_str(&super::background_surface(&Manifest::from_value(raw))).unwrap()
        };
        let minimal = project(json!({"manifest_version":3}));
        assert!(minimal["namespaces"]["runtime"].is_object());
        assert!(minimal["namespaces"]["tabs"].is_object());
        assert!(minimal["namespaces"]["storage"].is_null());
        assert!(minimal["namespaces"]["bookmarks"].is_null());
        assert!(minimal["namespaces"]["action"].is_null());
        assert!(minimal["namespaces"]["runtime"]["functions"]["sendNativeMessage"].is_null());

        let optional = project(json!({
            "manifest_version":3, "permissions":["storage"],
            "optional_permissions":["bookmarks", "nativeMessaging"], "action":{}
        }));
        for namespace in ["storage", "storage.local", "bookmarks", "action"] {
            assert!(optional["namespaces"][namespace].is_object(), "{namespace}");
        }
        assert!(optional["namespaces"]["runtime"]["functions"]["sendNativeMessage"].is_object());
        assert!(optional.to_string().len() < super::surface().len());
    }
}
