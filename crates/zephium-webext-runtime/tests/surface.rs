//! The renderer projection must agree with host authorization and signatures.
use serde_json::Value;
use zephium_webext_runtime::context::{ContextKind, ContextMask};
use zephium_webext_runtime::dispatch::permitted_contexts;
use zephium_webext_runtime::permissions::Gate;
use zephium_webext_runtime::schema;

fn surface() -> Value {
    let source = include_str!("../js/surface.generated.js");
    let value = source
        .split_once("Object.freeze(")
        .expect("surface declaration")
        .1
        .trim()
        .strip_suffix(");")
        .expect("surface terminator");
    serde_json::from_str(value).expect("surface JSON")
}

fn kinds() -> [(ContextKind, &'static str); 9] {
    [
        (ContextKind::Background, "background"),
        (ContextKind::Popup, "popup"),
        (ContextKind::Options, "options"),
        (ContextKind::ExtensionTab, "extensionTab"),
        (ContextKind::Offscreen, "offscreen"),
        (ContextKind::ExtensionFrame, "extensionFrame"),
        (ContextKind::ContentScript, "contentScript"),
        (ContextKind::UserScript, "userScript"),
        (ContextKind::ExternalPage, "externalPage"),
    ]
}

#[test]
fn projected_members_match_host_access_and_invocation_signatures() {
    let surface = surface();
    let namespaces = surface["namespaces"].as_object().expect("namespaces");
    for namespace in [
        "runtime",
        "tabs",
        "windows",
        "action",
        "commands",
        "permissions",
        "scripting",
        "webNavigation",
        "history",
        "bookmarks",
        "sessions",
        "topSites",
        "search",
        "notifications",
        "extension",
        "management",
        "idle",
        "dom",
        "fontSettings",
    ] {
        assert!(
            namespaces.contains_key(namespace),
            "missing batch-one namespace: {namespace}"
        );
    }
    let mut checked = 0;
    for (namespace, node) in namespaces {
        for field in ["functions", "events"] {
            for (name, spec) in node[field].as_object().into_iter().flatten() {
                let public_name = format!("{namespace}.{name}");
                let host_name = if field == "events" {
                    format!("{public_name}.addListener")
                } else {
                    public_name.clone()
                };
                let host = schema::access(&host_name);
                let expected_contexts = host.map_or(ContextMask::NONE, |access| {
                    access.contexts.intersection(permitted_contexts(&host_name))
                });
                let mut ancestors = Vec::new();
                let mut ancestor = namespace.as_str();
                loop {
                    ancestors.push(&namespaces[ancestor]);
                    if let Some((parent, _)) = ancestor.rsplit_once('.') {
                        ancestor = parent;
                    } else {
                        break;
                    }
                }
                ancestors.push(spec);
                for (kind, label) in kinds() {
                    let exposed = ancestors.iter().all(|node| {
                        node.get("contexts").is_none_or(|contexts| {
                            contexts
                                .as_array()
                                .expect("contexts")
                                .iter()
                                .any(|value| value == label)
                        })
                    });
                    assert_eq!(
                        exposed,
                        expected_contexts.contains(kind),
                        "{public_name} in {label}"
                    );
                }
                let permissions: Vec<_> = ancestors
                    .iter()
                    .filter_map(|node| node["permissions"].as_array())
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let features: Vec<_> = ancestors
                    .iter()
                    .filter_map(|node| node["manifest"].as_array())
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                if let Some(host) = host {
                    match host.gate {
                        Gate::Open => {
                            assert!(
                                permissions.is_empty() && features.is_empty(),
                                "{public_name}: {permissions:?}, {features:?}"
                            )
                        }
                        Gate::Requires(required) => assert!(
                            !permissions.is_empty()
                                && features.is_empty()
                                && permissions.iter().all(|permission| *permission == required),
                            "{public_name}: {permissions:?}, expected {required}"
                        ),
                        Gate::RequiresManifest(required) => assert!(
                            permissions.is_empty()
                                && !features.is_empty()
                                && features.iter().all(|feature| *feature == required),
                            "{public_name}: {features:?}, expected {required}"
                        ),
                        other => panic!("unprojectable gate for {public_name}: {other:?}"),
                    }
                }
                if field == "functions" {
                    let signature = spec["signature"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            let mut parts: Vec<_> = spec["parameters"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|parameter| {
                                    let ty = if let Some(choices) = parameter["type"].as_array() {
                                        format!(
                                            "[{}]",
                                            choices
                                                .iter()
                                                .filter_map(Value::as_str)
                                                .collect::<Vec<_>>()
                                                .join("|")
                                        )
                                    } else {
                                        parameter["type"].as_str().expect("type").to_owned()
                                    };
                                    format!(
                                        "{}{} {}",
                                        if parameter["optional"] == true {
                                            "optional "
                                        } else {
                                            ""
                                        },
                                        ty,
                                        parameter["name"].as_str().expect("name")
                                    )
                                })
                                .collect();
                            if spec["callback"] != false && spec["returns"] != "sync" {
                                parts.push("optional function callback".into());
                            }
                            parts.join(", ")
                        });
                    assert_eq!(
                        Some(signature.as_str()),
                        schema::invocation_signature(&public_name),
                        "{public_name}"
                    );
                }
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 420, "302 public callables and 118 events");
    assert!(surface["namespaces"]["runtime"]["functions"]
        .get("markListenerRegistrationComplete")
        .is_none());
    assert!(surface["namespaces"].get("zephium").is_none());
}
