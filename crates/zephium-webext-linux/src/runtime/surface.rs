//! The binding's API surface (`{version: 1, namespaces}`, see the header of
//! the runtime crate's `js/api.js`), projected from the generated Chromium
//! metadata with the same context and permission rules the host registers
//! methods under.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde_json::{json, Map, Value};
use zephium_webext_runtime::api::generated::SCHEMAS_JSON;
use zephium_webext_runtime::context::{ContextKind, ContextMask};
use zephium_webext_runtime::dispatch::permitted_contexts;
use zephium_webext_runtime::permissions::Gate;
use zephium_webext_runtime::schema;

const MAX_DEPTH: usize = 16;
const TYPES: [&str; 9] = [
    "string", "integer", "number", "boolean", "object", "array", "function", "binary", "any",
];

/// The surface as JSON text, built once.
pub(crate) fn surface() -> &'static str {
    static SURFACE: OnceLock<String> = OnceLock::new();
    SURFACE.get_or_init(|| build().to_string())
}

pub(crate) fn kind_name(kind: ContextKind) -> &'static str {
    match kind {
        ContextKind::Background => "background",
        ContextKind::Popup => "popup",
        ContextKind::Options => "options",
        ContextKind::ExtensionTab => "extensionTab",
        ContextKind::Offscreen => "offscreen",
        ContextKind::ExtensionFrame => "extensionFrame",
        ContextKind::ContentScript => "contentScript",
        ContextKind::UserScript => "userScript",
        ContextKind::ExternalPage => "externalPage",
    }
}

fn kinds(mask: ContextMask) -> Value {
    Value::Array(
        ContextKind::ALL
            .into_iter()
            .filter(|kind| mask.contains(*kind))
            .map(|kind| Value::from(kind_name(kind)))
            .collect(),
    )
}

#[derive(Default)]
struct Node {
    functions: Map<String, Value>,
    events: Map<String, Value>,
    properties: Map<String, Value>,
    contexts: Option<ContextMask>,
    permissions: BTreeSet<&'static str>,
    open: bool,
}

impl Node {
    fn admit(&mut self, contexts: ContextMask, gate: Gate) {
        self.contexts = Some(self.contexts.unwrap_or(ContextMask::NONE).union(contexts));
        match gate {
            Gate::Requires(permission) => {
                self.permissions.insert(permission);
            }
            _ => self.open = true,
        }
    }
}

struct Types<'a> {
    by_name: BTreeMap<String, &'a Value>,
}

impl<'a> Types<'a> {
    fn resolve(&self, namespace: &str, reference: &str) -> Option<&'a Value> {
        let name = if reference.contains('.') {
            reference.to_owned()
        } else {
            format!("{namespace}.{reference}")
        };
        self.by_name.get(&name).copied()
    }

    fn type_of(&self, namespace: &str, spec: &Value, depth: usize) -> Value {
        if depth > MAX_DEPTH {
            return "any".into();
        }
        if let Some(reference) = spec.get("$ref").and_then(Value::as_str) {
            let target_namespace = reference.rsplit_once('.').map_or(namespace, |(ns, _)| ns);
            return self.resolve(namespace, reference).map_or_else(
                || "any".into(),
                |target| self.type_of(target_namespace, target, depth + 1),
            );
        }
        if spec.get("enum").is_some() {
            return "string".into();
        }
        if let Some(choices) = spec.get("choices").and_then(Value::as_array) {
            let mut seen = Vec::new();
            for choice in choices {
                match self.type_of(namespace, choice, depth + 1) {
                    Value::String(kind) if kind == "any" => return "any".into(),
                    Value::String(kind) => {
                        if !seen.contains(&kind) {
                            seen.push(kind);
                        }
                    }
                    Value::Array(list) => {
                        for kind in list
                            .into_iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                        {
                            if !seen.contains(&kind) {
                                seen.push(kind);
                            }
                        }
                    }
                    _ => return "any".into(),
                }
            }
            return match seen.len() {
                0 => "any".into(),
                1 => seen.remove(0).into(),
                _ => seen.into_iter().map(Value::from).collect(),
            };
        }
        match spec.get("type").and_then(Value::as_str) {
            Some(kind) if TYPES.contains(&kind) => kind.into(),
            _ => "any".into(),
        }
    }

    fn serialized_functions(&self, namespace: &str, spec: &Value) -> Vec<String> {
        let target = match spec.get("$ref").and_then(Value::as_str) {
            Some(reference) => self.resolve(namespace, reference),
            None => Some(spec),
        };
        let Some(properties) = target
            .and_then(|target| target.get("properties"))
            .and_then(Value::as_object)
        else {
            return Vec::new();
        };
        properties
            .iter()
            .filter(|(_, property)| {
                property
                    .get("serializableFunction")
                    .and_then(Value::as_bool)
                    == Some(true)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }
}

fn access(name: &str) -> Option<(ContextMask, Gate)> {
    let access = schema::access(name)?;
    let contexts = access.contexts.intersection(permitted_contexts(name));
    (!contexts.is_empty()).then_some((contexts, access.gate))
}

fn member_rules(spec: &mut Map<String, Value>, contexts: ContextMask, gate: Gate) {
    spec.insert("contexts".into(), kinds(contexts));
    if let Gate::Requires(permission) = gate {
        spec.insert("permissions".into(), json!([permission]));
    }
}

fn parameter(types: &Types<'_>, namespace: &str, index: usize, raw: &Value) -> Value {
    let mut out = Map::new();
    out.insert(
        "name".into(),
        raw.get("name")
            .and_then(Value::as_str)
            .map_or_else(|| Value::from(format!("arg{index}")), Value::from),
    );
    out.insert("type".into(), types.type_of(namespace, raw, 0));
    if raw.get("optional").and_then(Value::as_bool) == Some(true) {
        out.insert("optional".into(), Value::Bool(true));
    }
    let serialized = types.serialized_functions(namespace, raw);
    if !serialized.is_empty() {
        out.insert("serializeFunctions".into(), json!(serialized));
    }
    Value::Object(out)
}

fn function(types: &Types<'_>, namespace: &str, full: &str, raw: &Value) -> Option<Value> {
    let (contexts, gate) = access(full)?;
    let mut parameters: Vec<&Value> = raw
        .get("parameters")
        .and_then(Value::as_array)
        .map(|list| list.iter().collect())
        .unwrap_or_default();
    let trailing_callback = parameters
        .last()
        .is_some_and(|last| last.get("type").and_then(Value::as_str) == Some("function"));
    if trailing_callback {
        parameters.pop();
    }
    let mut spec = Map::new();
    spec.insert(
        "parameters".into(),
        parameters
            .iter()
            .enumerate()
            .map(|(index, raw)| parameter(types, namespace, index, raw))
            .collect(),
    );
    if let Some(callback) = raw.get("returns_async") {
        if callback
            .get("parameters")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            spec.insert("result".into(), Value::Bool(false));
        }
        if raw.get("does_not_support_promises").is_some() {
            spec.insert("promise".into(), Value::Bool(false));
        }
    } else if raw.get("returns").is_some() {
        spec.insert("returns".into(), "sync".into());
    } else if !trailing_callback {
        spec.insert("callback".into(), Value::Bool(false));
    }
    if let Some(signature) = schema::invocation_signature(full) {
        spec.insert("signature".into(), signature.into());
    }
    member_rules(&mut spec, contexts, gate);
    Some(Value::Object(spec))
}

fn event(types: &Types<'_>, namespace: &str, full: &str, raw: &Value) -> Option<Value> {
    let (contexts, gate) = access(&format!("{full}.addListener"))?;
    let mut spec = Map::new();
    let filters = raw.get("filters").and_then(Value::as_array);
    if filters.is_some_and(|filters| {
        filters
            .iter()
            .any(|filter| filter.get("name").and_then(Value::as_str) == Some("url"))
    }) {
        spec.insert("filter".into(), "url".into());
    }
    let extra = raw.get("extraParameters").and_then(Value::as_array);
    if extra.is_some_and(|extra| {
        extra
            .first()
            .and_then(|first| first.get("$ref"))
            .and_then(Value::as_str)
            == Some("RequestFilter")
    }) {
        spec.insert("filter".into(), "request".into());
        let blocking = extra
            .into_iter()
            .flatten()
            .filter_map(|parameter| parameter.get("items"))
            .filter_map(|items| items.get("$ref").and_then(Value::as_str))
            .filter_map(|reference| types.resolve(namespace, reference))
            .filter_map(|options| options.get("enum").and_then(Value::as_array))
            .flatten()
            .any(|value| enum_value(value) == Some("blocking"));
        if blocking {
            spec.insert("blocking".into(), Value::Bool(true));
        }
    }
    if raw
        .get("options")
        .and_then(|options| options.get("supportsRules"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        spec.insert("rules".into(), Value::Bool(true));
    }
    member_rules(&mut spec, contexts, gate);
    Some(Value::Object(spec))
}

fn enum_value(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("name").and_then(Value::as_str))
}

// Chromium's extensions::binding::GetEnumName.
fn enum_key(value: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase() && previous_lower {
                out.push('_');
            }
            previous_lower = character.is_ascii_lowercase() || character.is_ascii_digit();
            out.push(character.to_ascii_uppercase());
        } else {
            previous_lower = false;
            out.push('_');
        }
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn split(name: &str) -> (&str, &str) {
    name.rsplit_once('.').unwrap_or(("", name))
}

fn build() -> Value {
    let Ok(schemas) = serde_json::from_str::<Vec<Value>>(SCHEMAS_JSON) else {
        return json!({"version": 1, "namespaces": {}});
    };
    let mut by_name = BTreeMap::new();
    for schema in &schemas {
        let Some(namespace) = schema.get("namespace").and_then(Value::as_str) else {
            continue;
        };
        for kind in schema
            .get("types")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = kind.get("id").and_then(Value::as_str) {
                by_name.insert(format!("{namespace}.{id}"), kind);
            }
        }
    }
    let types = Types { by_name };
    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    for schema in &schemas {
        let Some(namespace) = schema.get("namespace").and_then(Value::as_str) else {
            continue;
        };
        for raw in schema
            .get("functions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = raw.get("name").and_then(Value::as_str) else {
                continue;
            };
            let full = format!("{namespace}.{name}");
            let (prefix, member) = split(name);
            let key = if prefix.is_empty() {
                namespace.to_owned()
            } else {
                format!("{namespace}.{prefix}")
            };
            if let Some(spec) = function(&types, namespace, &full, raw) {
                let (contexts, gate) = access(&full).unwrap_or((ContextMask::NONE, Gate::Open));
                let node = nodes.entry(key).or_default();
                node.admit(contexts, gate);
                node.functions.insert(member.to_owned(), spec);
            }
        }
        for raw in schema
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = raw.get("name").and_then(Value::as_str) else {
                continue;
            };
            let full = format!("{namespace}.{name}");
            let (prefix, member) = split(name);
            let key = if prefix.is_empty() {
                namespace.to_owned()
            } else {
                format!("{namespace}.{prefix}")
            };
            if let Some(spec) = event(&types, namespace, &full, raw) {
                let (contexts, gate) = access(&format!("{full}.addListener"))
                    .unwrap_or((ContextMask::NONE, Gate::Open));
                let node = nodes.entry(key).or_default();
                node.admit(contexts, gate);
                node.events.insert(member.to_owned(), spec);
            }
        }
        if let Some(node) = nodes.get_mut(namespace) {
            for kind in schema
                .get("types")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (Some(id), Some(values)) = (
                    kind.get("id").and_then(Value::as_str),
                    kind.get("enum").and_then(Value::as_array),
                ) else {
                    continue;
                };
                let entries: Map<String, Value> = values
                    .iter()
                    .filter_map(enum_value)
                    .map(|value| (enum_key(value), Value::from(value)))
                    .collect();
                node.properties
                    .insert(id.to_owned(), Value::Object(entries));
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (name, property) in properties.into_iter().flatten() {
            constants(&mut nodes, namespace, name, property, 0);
        }
    }
    let namespaces: Map<String, Value> = nodes
        .into_iter()
        .filter(|(_, node)| node.contexts.is_some())
        .map(|(key, node)| {
            let mut spec = Map::new();
            if let Some(contexts) = node.contexts {
                spec.insert("contexts".into(), kinds(contexts));
            }
            if !node.open && !node.permissions.is_empty() {
                spec.insert("permissions".into(), json!(node.permissions));
            }
            spec.insert("functions".into(), Value::Object(node.functions));
            spec.insert("events".into(), Value::Object(node.events));
            if !node.properties.is_empty() {
                spec.insert("properties".into(), Value::Object(node.properties));
            }
            (key, Value::Object(spec))
        })
        .collect();
    json!({"version": 1, "namespaces": namespaces})
}

fn constants(
    nodes: &mut BTreeMap<String, Node>,
    namespace: &str,
    name: &str,
    property: &Value,
    depth: usize,
) {
    if depth > 4 {
        return;
    }
    let value = property.get("value");
    let scalar =
        value.is_some_and(|value| value.is_number() || value.is_string() || value.is_boolean());
    if scalar && property.get("$ref").is_none() {
        if let (Some(node), Some(value)) = (nodes.get_mut(namespace), value) {
            node.properties.insert(name.to_owned(), value.clone());
        }
        return;
    }
    let nested = format!("{namespace}.{name}");
    for (child, spec) in property
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        constants(nodes, &nested, child, spec, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespaces() -> Value {
        serde_json::from_str::<Value>(surface()).unwrap()["namespaces"].clone()
    }

    #[test]
    fn projects_functions_events_and_sub_namespaces() {
        let namespaces = namespaces();
        let local = &namespaces["storage.local"];
        assert_eq!(local["permissions"], json!(["storage"]));
        assert_eq!(local["functions"]["get"]["parameters"][0]["name"], "keys");
        assert!(local["functions"]["get"].get("returns").is_none());
        assert_eq!(local["functions"]["clear"]["result"], false);
        assert_eq!(local["properties"]["QUOTA_BYTES"], 10_485_760);
        assert!(local["events"]["onChanged"].is_object());
        let runtime = &namespaces["runtime"];
        assert_eq!(runtime["functions"]["getURL"]["returns"], "sync");
        assert_eq!(runtime["functions"]["connect"]["returns"], "sync");
        assert_eq!(
            runtime["properties"]["OnInstalledReason"]["CHROME_UPDATE"],
            "chrome_update"
        );
        assert!(runtime.get("permissions").is_none());
        assert!(runtime["contexts"]
            .as_array()
            .unwrap()
            .contains(&json!("externalPage")));
        assert_eq!(
            namespaces["webNavigation"]["events"]["onCommitted"]["filter"],
            "url"
        );
        let before = &namespaces["webRequest"]["events"]["onBeforeRequest"];
        assert_eq!(before["filter"], "request");
        assert_eq!(before["blocking"], true);
        assert_eq!(namespaces["offscreen"]["permissions"], json!(["offscreen"]));
        assert!(
            namespaces["privacy.services.passwordSavingEnabled"]["functions"]["get"].is_object()
        );
        assert!(serde_json::from_str::<Value>(surface()).unwrap()["version"] == 1);
    }

    #[test]
    fn enum_keys_follow_chromium() {
        assert_eq!(enum_key("install"), "INSTALL");
        assert_eq!(enum_key("chrome_update"), "CHROME_UPDATE");
        assert_eq!(enum_key("wasmUnsafe-eval"), "WASM_UNSAFE_EVAL");
        assert_eq!(enum_key("CLIPBOARD"), "CLIPBOARD");
        assert_eq!(enum_key("2d"), "_2D");
    }

    #[test]
    fn stays_well_under_the_runtime_resource_limit() {
        assert!(surface().len() < 512 * 1024, "{}", surface().len());
    }
}
