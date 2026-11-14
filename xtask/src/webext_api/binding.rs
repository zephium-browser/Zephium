//! The injected binding needs a small projection, not the host's full validator schemas.
use super::{enum_values, resolve, type_index, Catalog};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const KINDS: [&str; 9] = [
    "background",
    "popup",
    "options",
    "extensionTab",
    "offscreen",
    "extensionFrame",
    "contentScript",
    "userScript",
    "externalPage",
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct Access {
    contexts: BTreeSet<String>,
    permission: Option<String>,
    manifest: Option<String>,
}

fn contexts(value: &Value) -> Option<BTreeSet<String>> {
    if value.as_str() == Some("all") {
        return Some(KINDS.into_iter().map(str::to_owned).collect());
    }
    let mut result = BTreeSet::new();
    for kind in value.as_array()? {
        let names: &[&str] = match kind.as_str()? {
            "privileged_extension" | "blessed_extension" => &[
                "background",
                "popup",
                "options",
                "extensionTab",
                "extensionFrame",
            ],
            "unprivileged_extension" | "unblessed_extension" => &["extensionFrame"],
            "offscreen_extension" => &["offscreen"],
            "content_script" => &["contentScript"],
            "user_script" => &["userScript"],
            "web_page" | "privileged_web_page" => &["externalPage"],
            "webui" | "webui_untrusted" => &[],
            _ => return None,
        };
        result.extend(names.iter().map(|name| (*name).to_owned()));
    }
    Some(result)
}

// Keep feature inheritance and fail-closed alternatives aligned with schema/access.rs.
fn access(rules: &[Value], namespaces: &BTreeMap<String, Value>, depth: usize) -> Option<Access> {
    if depth >= 16 || rules.is_empty() {
        return None;
    }
    let mut ordered: Vec<_> = rules.iter().collect();
    ordered.sort_by_key(|rule| rule["name"].as_str().map(str::len));
    let mut parent = Some(Map::new());
    let mut alternatives = Vec::new();
    for entry in ordered {
        let rule = entry.get("rules")?;
        let choices: Vec<_> = rule
            .as_array()
            .map_or_else(|| vec![rule], |v| v.iter().collect());
        alternatives.clear();
        for choice in &choices {
            let mut inherited = if choice["noparent"] == true {
                Map::new()
            } else {
                parent.clone()?
            };
            inherited.extend(choice.as_object()?.clone());
            alternatives.push(inherited);
        }
        let default = choices
            .iter()
            .position(|v| v["default_parent"] == true)
            .or_else(|| (choices.len() == 1).then_some(0));
        parent = default.map(|index| alternatives[index].clone());
    }
    let mut result: Option<Access> = None;
    for rule in alternatives {
        if rule
            .get("extension_types")
            .and_then(Value::as_array)
            .is_some_and(|types| !types.iter().any(|v| v == "extension"))
        {
            continue;
        }
        let mut current = Access {
            contexts: contexts(rule.get("contexts")?)?,
            permission: None,
            manifest: None,
        };
        if current.contexts.is_empty() {
            continue;
        }
        if let Some(dependencies) = rule.get("dependencies") {
            for dependency in dependencies.as_array()? {
                let dependency = dependency.as_str()?;
                let (permission, manifest) =
                    if let Some(permission) = dependency.strip_prefix("permission:") {
                        (Some(permission.to_owned()), None)
                    } else if let Some(feature) = dependency.strip_prefix("manifest:") {
                        (None, Some(feature.to_owned()))
                    } else if let Some(api) = dependency.strip_prefix("api:") {
                        let inherited =
                            access(namespaces.get(api)?.as_array()?, namespaces, depth + 1)?;
                        current
                            .contexts
                            .retain(|kind| inherited.contexts.contains(kind));
                        (inherited.permission, inherited.manifest)
                    } else {
                        return None;
                    };
                if (current.permission.is_some() || current.manifest.is_some())
                    && (permission.is_some() || manifest.is_some())
                    && (current.permission != permission || current.manifest != manifest)
                {
                    return None;
                }
                if permission.is_some() || manifest.is_some() {
                    current.permission = permission;
                    current.manifest = manifest;
                }
            }
        }
        if current.contexts.is_empty() {
            continue;
        }
        if let Some(result) = &mut result {
            if result.permission != current.permission || result.manifest != current.manifest {
                return None;
            }
            result.contexts.extend(current.contexts);
        } else {
            result = Some(current);
        }
    }
    result
}

fn apply_policy(name: &str, access: &mut Access) {
    if !matches!(name, "runtime.sendMessage" | "runtime.connect") {
        access.contexts.remove("externalPage");
    }
    let (namespace, member) = name.split_once('.').unwrap_or((name, ""));
    let privileged = match namespace {
        "runtime" => matches!(member, "connectNative" | "sendNativeMessage"),
        "permissions" => member == "request",
        "cookies" | "management" | "debugger" => true,
        "privacy" => member.ends_with(".set") || member.ends_with(".clear"),
        "declarativeNetRequest" => {
            member.starts_with("update") || member == "setExtensionActionOptions"
        }
        _ => false,
    };
    if privileged {
        access
            .contexts
            .retain(|kind| KINDS[..5].contains(&kind.as_str()));
    }
}

fn context_value(contexts: &BTreeSet<String>) -> Value {
    KINDS
        .iter()
        .filter(|kind| contexts.contains(**kind))
        .map(|kind| json!(kind))
        .collect()
}

fn add_access(target: &mut Value, access: &Access) {
    target["contexts"] = context_value(&access.contexts);
    if let Some(permission) = &access.permission {
        target["permissions"] = json!([permission]);
    }
    if let Some(feature) = &access.manifest {
        target["manifest"] = json!([feature]);
    }
}

fn resolved<'a>(
    namespace: &'a str,
    schema: &'a Value,
    types: &'a BTreeMap<String, Value>,
    depth: usize,
) -> Option<(&'a str, &'a Value)> {
    if depth >= 32 {
        return None;
    }
    if let Some(reference) = schema["$ref"].as_str() {
        let (qualified, ty) = resolve(namespace, reference, types)?;
        return resolved(qualified.rsplit_once('.')?.0, ty, types, depth + 1);
    }
    Some((namespace, schema))
}

fn parameter_type(namespace: &str, schema: &Value, types: &BTreeMap<String, Value>) -> Value {
    let Some((namespace, schema)) = resolved(namespace, schema, types, 0) else {
        return json!("any");
    };
    if let Some(choices) = schema["choices"].as_array() {
        return choices
            .iter()
            .map(|choice| parameter_type(namespace, choice, types))
            .collect();
    }
    schema.get("type").cloned().unwrap_or_else(|| json!("any"))
}

fn type_label(namespace: &str, schema: &Value) -> String {
    if let Some(reference) = schema["$ref"].as_str() {
        return if reference.contains('.') {
            reference.to_owned()
        } else {
            format!("{namespace}.{reference}")
        };
    }
    if let Some(choices) = schema["choices"].as_array() {
        return format!(
            "[{}]",
            choices
                .iter()
                .map(|choice| type_label(namespace, choice))
                .collect::<Vec<_>>()
                .join("|")
        );
    }
    schema
        .get("isInstanceOf")
        .or_else(|| schema.get("instanceOf"))
        .and_then(Value::as_str)
        .or_else(|| schema["type"].as_str())
        .unwrap_or("any")
        .to_owned()
}

fn parameters(namespace: &str, values: &[Value], types: &BTreeMap<String, Value>) -> Vec<Value> {
    values
        .iter()
        .map(|schema| {
            let mut parameter =
                json!({"name":schema["name"], "type":parameter_type(namespace, schema, types)});
            if schema["optional"] == true {
                parameter["optional"] = true.into();
            }
            if let Some((_, resolved)) = resolved(namespace, schema, types, 0) {
                let names: Vec<_> = resolved["properties"]
                    .as_object()
                    .into_iter()
                    .flat_map(|properties| properties.iter())
                    .filter(|(_, spec)| spec["serializableFunction"] == true)
                    .map(|(name, _)| name.clone())
                    .collect();
                if !names.is_empty() {
                    parameter["serializeFunctions"] = json!(names);
                }
            }
            parameter
        })
        .collect()
}

fn function(namespace: &str, schema: &Value, types: &BTreeMap<String, Value>) -> Value {
    let raw = schema["parameters"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let callback = schema
        .get("returns_async")
        .or_else(|| raw.last().filter(|p| p["type"] == "function"));
    let raw = if raw.last().is_some_and(|p| p["type"] == "function") {
        &raw[..raw.len() - 1]
    } else {
        raw
    };
    let mut output = json!({});
    if !raw.is_empty() {
        output["parameters"] = parameters(namespace, raw, types).into();
    }
    let mut parts: Vec<_> = raw
        .iter()
        .map(|parameter| {
            format!(
                "{}{} {}",
                if parameter["optional"] == true {
                    "optional "
                } else {
                    ""
                },
                type_label(namespace, parameter),
                parameter["name"].as_str().unwrap_or("")
            )
        })
        .collect();
    if let Some(callback) = callback {
        if schema.get("returns").is_some() {
            output["returns"] = "sync".into();
            output["callback"] = true.into();
        }
        let promise = callback.get("does_not_support_promises").is_none()
            && callback["supportsPromises"] != false;
        let optional = promise || callback["optional"] == true;
        parts.push(format!(
            "{}function callback",
            if optional { "optional " } else { "" }
        ));
        if !promise {
            output["promise"] = false.into();
        }
        if !optional {
            output["callbackRequired"] = true.into();
        }
        let arity = callback["parameters"].as_array().map_or(0, Vec::len);
        if arity == 0 {
            output["result"] = false.into();
        }
        if arity == 1
            && (callback["parameters"][0]["optional"] == true
                || callback["parameters"][0]["nullable"] == true)
        {
            output["optionalResult"] = true.into();
        }
    } else if schema.get("returns").is_some() {
        output["returns"] = "sync".into();
    } else {
        output["callback"] = false.into();
    }
    let simple: Vec<_> = output["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let ty = if let Some(choices) = p["type"].as_array() {
                format!(
                    "[{}]",
                    choices
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("|")
                )
            } else {
                p["type"].as_str().unwrap_or("any").to_owned()
            };
            format!(
                "{}{} {}",
                if p["optional"] == true {
                    "optional "
                } else {
                    ""
                },
                ty,
                p["name"].as_str().unwrap_or("")
            )
        })
        .chain(callback.map(|_| "optional function callback".to_owned()))
        .collect();
    let signature = parts.join(", ");
    if signature != simple.join(", ") || output["callback"] == true {
        output["signature"] = signature.into();
    }
    output
}

fn setting_schema(
    namespace: &str,
    schema: &Value,
    types: &BTreeMap<String, Value>,
    depth: usize,
) -> Result<Value, String> {
    if depth >= 32 {
        return Err("recursive privacy setting schema".into());
    }
    let (owner, resolved) =
        resolved(namespace, schema, types, 0).ok_or("unresolved privacy setting schema")?;
    let mut output = json!({"type":resolved["type"]});
    if schema["optional"] == true || resolved["optional"] == true {
        output["optional"] = true.into();
    }
    if let Some(values) = resolved["enum"].as_array() {
        output["enum"] = values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .or_else(|| value["name"].as_str())
                    .map(Value::from)
                    .ok_or("invalid setting enum")
            })
            .collect::<Result<Vec<_>, _>>()?
            .into();
    }
    if let Some(properties) = resolved["properties"].as_object() {
        output["properties"] = properties
            .iter()
            .map(|(name, schema)| {
                Ok((
                    name.clone(),
                    setting_schema(owner, schema, types, depth + 1)?,
                ))
            })
            .collect::<Result<Map<_, _>, String>>()?
            .into();
    }
    if let Some(items) = resolved.get("items") {
        output["items"] = setting_schema(owner, items, types, depth + 1)?;
    }
    Ok(output)
}

fn event(schema: &Value) -> Value {
    let mut output = json!({});
    if let Some(filters) = schema["filters"].as_array().filter(|v| !v.is_empty()) {
        output["filter"] = if filters.iter().any(|filter| filter["name"] == "windowTypes") {
            "window"
        } else {
            "url"
        }
        .into();
    }
    if let Some(extra) = schema["extraParameters"].as_array() {
        if extra.iter().any(|p| p["name"] == "filter") {
            output["filter"] = "request".into();
        }
    }
    if schema.get("returns").is_some()
        || schema["parameters"]
            .as_array()
            .is_some_and(|v| v.iter().any(|p| p["type"] == "function"))
    {
        output["blocking"] = true.into();
    }
    if schema["options"]["supportsRules"] == true {
        output["rules"] = true.into();
    }
    output
}

fn ensure_namespace<'a>(namespaces: &'a mut Map<String, Value>, name: &str) -> &'a mut Value {
    if let Some((parent, _)) = name.rsplit_once('.') {
        ensure_namespace(namespaces, parent);
    }
    namespaces
        .entry(name.to_owned())
        .or_insert_with(|| json!({}))
}

fn constants(
    namespace: &str,
    properties: &Value,
    types: &BTreeMap<String, Value>,
    namespaces: &mut Map<String, Value>,
    depth: usize,
) {
    if depth >= 16 {
        return;
    }
    if let Some(properties) = properties.as_object() {
        for (name, property) in properties {
            if let Some(value) = property.get("value").filter(|_| {
                property.get("$ref").is_none()
                    && property["properties"].as_object().is_none_or(Map::is_empty)
            }) {
                let target = ensure_namespace(namespaces, namespace);
                if !target["properties"].is_object() {
                    target["properties"] = json!({});
                }
                target["properties"][name] = value.clone();
            } else {
                let child = format!("{namespace}.{name}");
                constants(
                    &child,
                    &property["properties"],
                    types,
                    namespaces,
                    depth + 1,
                );
                let referenced = property.get("$ref").and_then(|_| {
                    resolved(
                        namespace.split('.').next().unwrap_or(namespace),
                        property,
                        types,
                        0,
                    )
                });
                if let Some((_, resolved)) = referenced {
                    constants(
                        &child,
                        &resolved["properties"],
                        types,
                        namespaces,
                        depth + 1,
                    );
                }
            }
        }
    }
}

pub(super) fn surface(catalog: &Catalog) -> Result<Value, String> {
    let types = type_index(catalog.schemas.iter())?;
    let rules: BTreeMap<_, _> = catalog
        .schemas
        .iter()
        .map(|schema| {
            (
                schema["namespace"].as_str().unwrap_or("").to_owned(),
                schema["feature_rules"].clone(),
            )
        })
        .collect();
    let mut namespaces = Map::new();
    for schema in &catalog.schemas {
        let namespace = schema["namespace"]
            .as_str()
            .ok_or("namespace missing name")?;
        ensure_namespace(&mut namespaces, namespace);
        for (field, convert) in [
            (
                "functions",
                function as fn(&str, &Value, &BTreeMap<String, Value>) -> Value,
            ),
            ("events", |_, schema, _| event(schema)),
        ] {
            for member in schema[field]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                let name = member["name"].as_str().ok_or("member missing name")?;
                let full = format!("{namespace}.{name}");
                if full == "runtime.markListenerRegistrationComplete" {
                    continue;
                }
                let mut spec = convert(namespace, member, &types);
                if namespace == "privacy" && name.ends_with(".set") {
                    let mut property = schema;
                    for part in name
                        .strip_suffix(".set")
                        .ok_or("invalid setting method")?
                        .split('.')
                    {
                        property = property
                            .get("properties")
                            .and_then(|p| p.get(part))
                            .ok_or("missing setting property")?;
                    }
                    let value = property["value"]
                        .as_array()
                        .and_then(|v| v.get(1))
                        .ok_or("missing setting value schema")?;
                    let details = json!({"type":"object","properties":{"scope":{"$ref":"types.ChromeSettingScope","optional":true},"value":value}});
                    spec["setting"] = setting_schema(namespace, &details, &types, 0)?;
                }
                let mut access = access(
                    member["feature_rules"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    &rules,
                    0,
                )
                .unwrap_or(Access {
                    contexts: BTreeSet::new(),
                    permission: None,
                    manifest: None,
                });
                apply_policy(&full, &mut access);
                add_access(&mut spec, &access);
                let (parent, leaf) = full.rsplit_once('.').ok_or("invalid member name")?;
                let target = ensure_namespace(&mut namespaces, parent);
                if !target[field].is_object() {
                    target[field] = json!({});
                }
                target[field][leaf] = spec;
            }
        }
        let enums = enum_values(schema);
        if !enums.as_object().is_none_or(Map::is_empty) {
            ensure_namespace(&mut namespaces, namespace)["properties"] = enums;
        }
        constants(namespace, &schema["properties"], &types, &mut namespaces, 0);
    }
    // Lift common access metadata to the owning namespace and union child contexts.
    let names: Vec<_> = namespaces.keys().cloned().collect();
    for name in names.iter().rev() {
        let node = namespaces.get(name).ok_or("missing namespace")?;
        let members: Vec<_> = ["functions", "events"]
            .iter()
            .flat_map(|field| {
                node[*field]
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.values())
            })
            .collect();
        let mut allowed = BTreeSet::new();
        for member in &members {
            allowed.extend(
                member["contexts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
        for (child, node) in &namespaces {
            if child
                .rsplit_once('.')
                .is_some_and(|(parent, _)| parent == name)
            {
                allowed.extend(
                    node["contexts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned),
                );
            }
        }
        let children: Vec<_> = namespaces
            .iter()
            .filter(|(child, _)| {
                child
                    .rsplit_once('.')
                    .is_some_and(|(parent, _)| parent == name)
            })
            .map(|(_, node)| node)
            .collect();
        let access_nodes: Vec<_> = members.iter().copied().chain(children).collect();
        let common_access: Vec<_> = ["permissions", "manifest"]
            .into_iter()
            .filter_map(|field| {
                access_nodes
                    .first()
                    .and_then(|member| member.get(field))
                    .filter(|value| {
                        access_nodes
                            .iter()
                            .all(|member| member.get(field) == Some(*value))
                    })
                    .map(|value| (field, value.clone()))
            })
            .collect();
        if allowed.is_empty() && members.is_empty() {
            allowed = access(
                rules
                    .get(name)
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                &rules,
                0,
            )
            .map_or_else(BTreeSet::new, |a| a.contexts);
            allowed.remove("externalPage");
        }
        let all_contexts = context_value(&allowed);
        let node = namespaces.get_mut(name).ok_or("missing namespace")?;
        node["contexts"] = all_contexts.clone();
        for (field, value) in &common_access {
            node[*field] = value.clone();
        }
        for field in ["functions", "events"] {
            if let Some(members) = node.get_mut(field).and_then(Value::as_object_mut) {
                for member in members.values_mut() {
                    let member = member.as_object_mut().ok_or("invalid member")?;
                    if member.get("contexts") == Some(&all_contexts) {
                        member.remove("contexts");
                    }
                    for (field, _) in &common_access {
                        member.remove(*field);
                    }
                }
            }
        }
    }
    namespaces.retain(|name, node| {
        let prefix = format!("{name}.");
        names.iter().any(|child| child.starts_with(&prefix))
            || ["functions", "events", "properties"].iter().any(|field| {
                node[*field]
                    .as_object()
                    .is_some_and(|members| !members.is_empty())
            })
    });
    Ok(json!({"version":1, "namespaces":namespaces}))
}

pub(super) fn fixture(surface: &Value, members: &str) -> Result<String, String> {
    let members: Value = serde_json::from_str(members).map_err(|error| error.to_string())?;
    let mut namespaces = Map::new();
    for (name, selected) in members
        .as_object()
        .ok_or("fixture members must be an object")?
    {
        let mut node = surface["namespaces"][name]
            .as_object()
            .ok_or_else(|| format!("fixture namespace missing: {name}"))?
            .clone();
        for field in ["functions", "events", "properties"] {
            let mut trimmed = Map::new();
            for member in selected[field].as_array().into_iter().flatten() {
                let member = member.as_str().ok_or("fixture member must be a string")?;
                let value = node
                    .get(field)
                    .and_then(|members| members.get(member))
                    .ok_or_else(|| format!("fixture member missing: {name}.{member}"))?;
                trimmed.insert(member.to_owned(), value.clone());
            }
            if trimmed.is_empty() {
                node.remove(field);
            } else {
                node.insert(field.into(), trimmed.into());
            }
        }
        namespaces.insert(name.clone(), node.into());
    }
    serde_json::to_string_pretty(&json!({"version":1,"namespaces":namespaces}))
        .map(|s| s + "\n")
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(permission: &str) -> Value {
        json!([{"name":"sample","rules":{"contexts":["privileged_extension"],"dependencies":[format!("permission:{permission}")]}}])
    }

    #[test]
    fn namespace_permission_lifting_preserves_child_alternatives_and_constants() {
        let catalog = Catalog {
            features: BTreeMap::new(),
            schemas: vec![json!({
                "namespace":"sample", "feature_rules":rules("parent"),
                "functions":[
                    {"name":"top","feature_rules":rules("parent")},
                    {"name":"local.call","feature_rules":rules("child")}
                ], "events":[],
                "types":[{"id":"Area","type":"object","properties":{"TYPE_LIMIT":{"value":3}}}],
                "properties":{"local":{"$ref":"Area","value":["local"],"properties":{"QUOTA_BYTES":{"value":9}}}}
            })],
        };
        let surface = surface(&catalog).unwrap();
        assert!(surface["namespaces"]["sample"].get("permissions").is_none());
        assert_eq!(
            surface["namespaces"]["sample"]["functions"]["top"]["permissions"],
            json!(["parent"])
        );
        assert_eq!(
            surface["namespaces"]["sample.local"]["permissions"],
            json!(["child"])
        );
        assert_eq!(
            surface["namespaces"]["sample.local"]["properties"],
            json!({"QUOTA_BYTES":9,"TYPE_LIMIT":3})
        );
        let selected = fixture(
            &surface,
            r#"{"sample":{},"sample.local":{"functions":["call"],"properties":["QUOTA_BYTES"]}}"#,
        )
        .unwrap();
        let selected: Value = serde_json::from_str(&selected).unwrap();
        assert_eq!(
            selected["namespaces"]["sample.local"]["properties"],
            json!({"QUOTA_BYTES":9})
        );
        assert!(fixture(&surface, r#"{"sample":{"functions":["unknown"]}}"#).is_err());
    }

    #[test]
    fn binding_metadata_preserves_modes_serialization_and_filter_kinds() {
        let types = BTreeMap::from([(
            "sample.Injection".into(),
            json!({"type":"object","properties":{"func":{"type":"function","serializableFunction":true}}}),
        )]);
        let async_method = function(
            "sample",
            &json!({"parameters":[{"name":"injection","$ref":"Injection"}],"returns_async":{"parameters":[{"name":"result","type":"object","nullable":true}]}}),
            &types,
        );
        assert_eq!(
            async_method["signature"],
            "sample.Injection injection, optional function callback"
        );
        assert_eq!(
            async_method["parameters"][0]["serializeFunctions"],
            json!(["func"])
        );
        assert_eq!(async_method["optionalResult"], true);
        let callback_only = function(
            "sample",
            &json!({"returns_async":{"does_not_support_promises":true,"parameters":[]}}),
            &types,
        );
        assert_eq!(callback_only["promise"], false);
        assert_eq!(callback_only["callbackRequired"], true);
        assert_eq!(callback_only["result"], false);
        assert_eq!(
            function("sample", &json!({"returns":{"type":"string"}}), &types)["returns"],
            "sync"
        );
        assert_eq!(function("sample", &json!({}), &types)["callback"], false);
        assert_eq!(
            event(&json!({"filters":[{"name":"windowTypes"}]}))["filter"],
            "window"
        );
        assert_eq!(event(&json!({"filters":[{"name":"url"}]}))["filter"], "url");
        assert_eq!(
            event(&json!({"extraParameters":[{"name":"filter"}],"returns":{"type":"object"}})),
            json!({"filter":"request","blocking":true})
        );
    }

    #[test]
    fn manifest_dependencies_survive_inheritance_and_namespace_lifting() {
        let rule = json!([{"name":"sample","rules":{"contexts":["privileged_extension"],"dependencies":["manifest:action"]}}]);
        let namespaces = BTreeMap::from([("sample".into(), rule.clone())]);
        let inherited = json!([{"name":"child","rules":{"contexts":["privileged_extension"],"dependencies":["api:sample"]}}]);
        assert_eq!(
            access(inherited.as_array().unwrap(), &namespaces, 0)
                .unwrap()
                .manifest
                .as_deref(),
            Some("action")
        );
        let catalog = Catalog {
            features: BTreeMap::new(),
            schemas: vec![json!({
                "namespace":"sample", "feature_rules":rule,
                "functions":[{"name":"call","feature_rules":rule}],
                "events":[{"name":"onEvent","feature_rules":rule}]
            })],
        };
        let surface = surface(&catalog).unwrap();
        assert_eq!(
            surface["namespaces"]["sample"]["manifest"],
            json!(["action"])
        );
        assert!(surface["namespaces"]["sample"]["functions"]["call"]
            .get("manifest")
            .is_none());
        for dependencies in [
            json!(["permission:action", "manifest:action"]),
            json!(["manifest:action", "manifest:commands"]),
        ] {
            let rules = json!([{"name":"sample","rules":{"contexts":["privileged_extension"],"dependencies":dependencies}}]);
            assert!(access(rules.as_array().unwrap(), &namespaces, 0).is_none());
        }
    }

    #[test]
    fn unsupported_authority_expressions_fail_closed() {
        let namespaces = BTreeMap::new();
        assert!(access(json!([{"name":"sample","rules":{"contexts":["privileged_extension"],"dependencies":["unsupported:action"]}}]).as_array().unwrap(), &namespaces, 0).is_none());
        let alternatives = json!([{"name":"sample","rules":[{"contexts":["privileged_extension"],"dependencies":["permission:one"]},{"contexts":["content_script"],"dependencies":["permission:two"]}]}]);
        assert!(access(alternatives.as_array().unwrap(), &namespaces, 0).is_none());
    }
}
