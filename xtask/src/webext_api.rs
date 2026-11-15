mod binding;
mod idl;
mod json;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const SNAPSHOT: &str = "vendor/chromium-extension-api";
const RUST_OUTPUT: &str = "crates/zephium-webext-runtime/src/api/generated.rs";
const JS_OUTPUT: &str = "crates/zephium-webext-runtime/js/surface.generated.js";
const JS_FIXTURE: &str = "crates/zephium-webext-runtime/js/test/surface.fixture.json";
const JS_MEMBERS: &str = "crates/zephium-webext-runtime/js/test/surface.members.json";

pub fn run(arguments: &[String]) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut check = false;
    let mut coverage = None;
    let mut inventory = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--check" if !check => check = true,
            "--coverage" if coverage.is_none() => {
                coverage = Some(PathBuf::from(
                    arguments.next().ok_or("--coverage needs a path")?,
                ));
            }
            "--inventory" if inventory.is_none() => {
                inventory = Some(PathBuf::from(
                    arguments.next().ok_or("--inventory needs a path")?,
                ));
            }
            _ => {
                return Err(format!(
                    "unknown or repeated webext-api argument {argument:?}"
                ))
            }
        }
    }
    if inventory.is_some() && coverage.is_none() {
        return Err("--inventory requires --coverage".into());
    }
    if coverage.is_some() && inventory.is_none() {
        return Err("--coverage requires --inventory with an audited JSON member inventory".into());
    }
    let catalog = load(&root.join(SNAPSHOT))?;
    let (rust, javascript) = generate(&catalog)?;
    output(&root.join(RUST_OUTPUT), &rust, check)?;
    output(&root.join(JS_OUTPUT), &javascript, check)?;
    let members = fs::read_to_string(root.join(JS_MEMBERS)).map_err(|error| error.to_string())?;
    let fixture = binding::fixture(&binding::surface(&catalog)?, &members)?;
    output(&root.join(JS_FIXTURE), &fixture, check)?;
    if let Some(path) = coverage {
        let absolute = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|error| error.to_string())?
                .join(path)
        };
        let parent = absolute
            .parent()
            .ok_or("coverage path needs a parent directory")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if parent.starts_with(root.canonicalize().map_err(|error| error.to_string())?) {
            return Err("coverage reports belong outside the repository".into());
        }
        let inventory = inventory.ok_or("missing coverage inventory")?;
        let inventory = fs::read_to_string(&inventory)
            .map_err(|error| format!("{}: {error}", inventory.display()))?;
        fs::write(&absolute, coverage_table(&catalog, &inventory)?)
            .map_err(|error| error.to_string())?;
    }
    println!(
        "extension API {}: {} namespaces",
        if check { "verified" } else { "generated" },
        catalog.schemas.len()
    );
    Ok(())
}

struct Catalog {
    schemas: Vec<Value>,
    features: BTreeMap<String, Value>,
}

fn load(snapshot: &Path) -> Result<Catalog, String> {
    let mut paths = Vec::new();
    discover(snapshot, &mut paths)?;
    paths.sort();
    verify_snapshot(snapshot, &paths)?;
    let mut schemas = BTreeMap::new();
    let mut features = BTreeMap::new();
    for path in paths {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid snapshot path")?;
        let extension = path.extension().and_then(|name| name.to_str());
        if !matches!(extension, Some("json" | "idl" | "webidl")) {
            continue;
        }
        let source =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let source_path = path
            .strip_prefix(snapshot)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if filename.starts_with('_') {
            if extension != Some("json") {
                return Err(format!("unexpected metadata format: {}", path.display()));
            }
            let mut feature =
                json::parse(&source).map_err(|error| format!("{}: {error}", path.display()))?;
            if !feature.is_object() {
                return Err(format!("metadata is not an object: {}", path.display()));
            }
            strip_prose(&mut feature);
            features.insert(source_path, feature);
            continue;
        }
        let parsed = match extension {
            Some("json") => json::parse(&source),
            Some("idl") => idl::parse(&source, false).map(|schema| Value::Array(vec![schema])),
            Some("webidl") => idl::parse(&source, true).map(|schema| Value::Array(vec![schema])),
            _ => continue,
        }
        .map_err(|error| format!("{}: {error}", path.display()))?;
        let parsed = parsed
            .as_array()
            .ok_or_else(|| format!("schema root is not an array: {}", path.display()))?;
        for mut schema in parsed.clone() {
            let namespace = schema["namespace"]
                .as_str()
                .ok_or_else(|| format!("missing namespace in {}", path.display()))?
                .to_owned();
            strip_prose(&mut schema);
            schema["source"] = source_path.clone().into();
            if schemas.insert(namespace.clone(), schema).is_some() {
                return Err(format!("duplicate namespace {namespace}"));
            }
        }
    }
    if schemas.is_empty() {
        return Err("snapshot has no API schemas".into());
    }
    let types = type_index(schemas.values())?;
    for (namespace, schema) in &mut schemas {
        let mut functions = schema["functions"].as_array().cloned().unwrap_or_default();
        let mut events = schema["events"].as_array().cloned().unwrap_or_default();
        if let Some(properties) = schema["properties"].as_object() {
            for (name, property) in properties {
                expand_properties(
                    namespace,
                    name,
                    property,
                    &types,
                    &mut functions,
                    &mut events,
                    &mut BTreeSet::new(),
                )?;
            }
        }
        sort_members(&mut functions)?;
        sort_members(&mut events)?;
        for member in functions.iter_mut().chain(events.iter_mut()) {
            member["feature_rules"] = feature_rules(
                &format!(
                    "{namespace}.{}",
                    member["name"].as_str().ok_or("member missing name")?
                ),
                &features,
            );
        }
        schema["functions"] = functions.into();
        schema["events"] = events.into();
        schema["feature_rules"] = feature_rules(namespace, &features);
    }
    for (namespace, schema) in &schemas {
        check_references(namespace, schema, &types)?;
    }
    Ok(Catalog {
        schemas: schemas.into_values().collect(),
        features,
    })
}

fn discover(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "snapshot contains a symlink: {}",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            discover(&entry.path(), paths)?;
        } else if kind.is_file() {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn verify_snapshot(snapshot: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let manifest = snapshot.join("UPSTREAM_FILES.toml");
    let source = fs::read_to_string(&manifest)
        .map_err(|error| format!("{}: {error}", manifest.display()))?;
    let manifest: toml::Value = toml::from_str(&source).map_err(|error| error.to_string())?;
    let entries = manifest
        .get("files")
        .and_then(toml::Value::as_array)
        .ok_or("snapshot inventory has no files")?;
    let mut expected = BTreeSet::new();
    for entry in entries {
        let relative = entry
            .get("path")
            .and_then(toml::Value::as_str)
            .ok_or("snapshot entry missing path")?;
        if Path::new(relative)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!("invalid snapshot inventory path {relative:?}"));
        }
        if !expected.insert(relative.to_owned()) {
            return Err(format!("duplicate snapshot inventory path {relative}"));
        }
        let bytes =
            fs::read(snapshot.join(relative)).map_err(|error| format!("{relative}: {error}"))?;
        let expected_bytes = entry
            .get("bytes")
            .and_then(toml::Value::as_integer)
            .ok_or("snapshot entry missing bytes")?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if expected_bytes < 0
            || bytes.len() as u64 != expected_bytes as u64
            || entry.get("sha256").and_then(toml::Value::as_str) != Some(digest.as_str())
        {
            return Err(format!("snapshot integrity mismatch: {relative}"));
        }
    }
    for path in paths {
        let relative = path
            .strip_prefix(snapshot)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if !expected.contains(&relative)
            && !["UPSTREAM.md", "UPSTREAM_FILES.toml", "REBASE.md"].contains(&relative.as_str())
        {
            return Err(format!("unlisted snapshot file {relative}"));
        }
    }
    Ok(())
}

fn strip_prose(value: &mut Value) {
    strip_node_prose(value, false);
}

fn strip_node_prose(value: &mut Value, named_map: bool) {
    match value {
        Value::Object(map) => {
            if !named_map {
                for key in [
                    "description",
                    "documentation",
                    "description_nodoc",
                    "jsexterns",
                ] {
                    map.remove(key);
                }
                for key in ["deprecated", "does_not_support_promises"] {
                    if map.get(key).is_some_and(Value::is_string) {
                        map.insert(key.into(), Value::Bool(true));
                    }
                }
            }
            for (key, value) in map {
                strip_node_prose(
                    value,
                    !named_map
                        && matches!(
                            key.as_str(),
                            "properties" | "patternProperties" | "definitions"
                        ),
                );
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| strip_node_prose(value, false)),
        _ => {}
    }
}

fn type_index<'a>(
    schemas: impl Iterator<Item = &'a Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut types = BTreeMap::new();
    for schema in schemas {
        let namespace = schema["namespace"].as_str().ok_or("missing namespace")?;
        if let Some(entries) = schema["types"].as_array() {
            for entry in entries {
                let id = entry["id"].as_str().ok_or("type missing id")?;
                let id = if id.contains('.') {
                    id.to_owned()
                } else {
                    format!("{namespace}.{id}")
                };
                if types.insert(id.clone(), entry.clone()).is_some() {
                    return Err(format!("duplicate type {id}"));
                }
            }
        }
    }
    Ok(types)
}

fn resolve<'a>(
    namespace: &str,
    reference: &str,
    types: &'a BTreeMap<String, Value>,
) -> Option<(&'a str, &'a Value)> {
    let qualified = if reference.contains('.') {
        reference.to_owned()
    } else {
        format!("{namespace}.{reference}")
    };
    types
        .get_key_value(&qualified)
        .map(|(key, value)| (key.as_str(), value))
}

fn expand_properties(
    namespace: &str,
    path: &str,
    property: &Value,
    types: &BTreeMap<String, Value>,
    functions: &mut Vec<Value>,
    events: &mut Vec<Value>,
    active: &mut BTreeSet<String>,
) -> Result<(), String> {
    if let Some(reference) = property["$ref"].as_str() {
        let (qualified, ty) = resolve(namespace, reference, types)
            .ok_or_else(|| format!("unknown property type {namespace}.{reference}"))?;
        if !active.insert(qualified.to_owned()) {
            return Ok(());
        }
        let owner = qualified
            .rsplit_once('.')
            .ok_or("invalid qualified type")?
            .0;
        let mut ty = ty.clone();
        qualify_references(owner, &mut ty);
        expand_properties(namespace, path, &ty, types, functions, events, active)?;
        active.remove(qualified);
    }
    for (field, members) in [("functions", &mut *functions), ("events", &mut *events)] {
        if let Some(entries) = property[field].as_array() {
            for entry in entries {
                let mut entry = entry.clone();
                let name = entry["name"].as_str().ok_or("nested member missing name")?;
                entry["name"] = format!("{path}.{name}").into();
                members.push(entry);
            }
        }
    }
    if let Some(properties) = property["properties"].as_object() {
        for (name, child) in properties {
            expand_properties(
                namespace,
                &format!("{path}.{name}"),
                child,
                types,
                functions,
                events,
                active,
            )?;
        }
    }
    Ok(())
}

fn qualify_references(namespace: &str, value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get_mut("$ref") {
                if !reference.contains('.') {
                    *reference = format!("{namespace}.{reference}");
                }
            }
            for value in map.values_mut() {
                qualify_references(namespace, value);
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| qualify_references(namespace, value)),
        _ => {}
    }
}

fn check_references(
    namespace: &str,
    value: &Value,
    types: &BTreeMap<String, Value>,
) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
                if resolve(namespace, reference, types).is_none() {
                    return Err(format!(
                        "unresolved schema reference {namespace}: {reference}"
                    ));
                }
            }
            for value in map.values() {
                check_references(namespace, value, types)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                check_references(namespace, value, types)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn sort_members(members: &mut [Value]) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for member in members.iter() {
        let name = member["name"].as_str().ok_or("member missing name")?;
        if !names.insert(name) {
            return Err(format!("duplicate member {name}"));
        }
    }
    members.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    Ok(())
}

fn feature_rules(name: &str, features: &BTreeMap<String, Value>) -> Value {
    let mut rules = Vec::new();
    for (source, features) in features.iter().filter(|(path, _)| {
        path.ends_with("/_api_features.json") || path.ends_with("/_api_features.override.json")
    }) {
        let mut ancestor = name;
        loop {
            if let Some(rule) = features.get(ancestor) {
                rules.push(json!({"name":ancestor,"source":source,"rules":rule}));
            }
            let Some((parent, _)) = ancestor.rsplit_once('.') else {
                break;
            };
            ancestor = parent;
        }
    }
    rules.into()
}

fn generate(catalog: &Catalog) -> Result<(String, String), String> {
    let schemas = serde_json::to_string(&catalog.schemas).map_err(|error| error.to_string())?;
    let features = serde_json::to_string(&catalog.features).map_err(|error| error.to_string())?;
    let mut rust = String::from("//! Chromium extension API metadata; see vendor/chromium-extension-api/UPSTREAM.md.\n//! Regenerate with `cargo xtask webext-api`.\n\n/// Parameter schemas and namespace members, without documentation prose.\npub const SCHEMAS_JSON: &str = ");
    rust.push_str(&rust_literal(&schemas));
    rust.push_str(";\n\n/// Original feature rule alternatives, dependencies and platform restrictions.\npub const FEATURES_JSON: &str = ");
    rust.push_str(&rust_literal(&features));
    rust.push_str(";\n\n/// Schema validators for each callable dotted member name.\npub const METHOD_VALIDATORS: &[(&str, crate::dispatch::Validator)] = &[\n");
    for schema in &catalog.schemas {
        let namespace = schema["namespace"]
            .as_str()
            .ok_or("namespace missing name")?;
        let functions = schema["functions"]
            .as_array()
            .ok_or("namespace missing functions")?;
        for function in functions {
            let name = function["name"].as_str().ok_or("function missing name")?;
            let full_name = format!("{namespace}.{name}");
            let quoted = serde_json::to_string(&full_name).map_err(|error| error.to_string())?;
            rust.push_str(&format!(
                "    ({quoted}, |args| crate::schema::validate({quoted}, args)),\n"
            ));
        }
    }
    rust.push_str("];\n\n/// Finds the validator for a dotted method name.\npub fn validator(name: &str) -> Option<crate::dispatch::Validator> {\n    METHOD_VALIDATORS\n        .iter()\n        .find_map(|(member, validate)| (*member == name).then_some(*validate))\n}\n");
    let javascript = format!("// Chromium BSD-3-Clause metadata; see vendor/chromium-extension-api/UPSTREAM.md.\n// Regenerate with cargo xtask webext-api.\nglobalThis.__zephiumWebExtensionSurface = Object.freeze({});\n", serde_json::to_string(&binding::surface(catalog)?).map_err(|error| error.to_string())?);
    Ok((format_rust(&rust)?, javascript))
}

fn format_rust(source: &str) -> Result<String, String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2021", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("rustfmt: {error}"))?;
    child
        .stdin
        .take()
        .ok_or("rustfmt stdin unavailable")?
        .write_all(source.as_bytes())
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "rustfmt: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}

fn enum_values(schema: &Value) -> Value {
    let mut values = Map::new();
    if let Some(types) = schema["types"].as_array() {
        for ty in types {
            if let (Some(name), Some(items)) = (ty["id"].as_str(), ty["enum"].as_array()) {
                let items: Map<String, Value> = items
                    .iter()
                    .filter_map(|item| {
                        item.as_str()
                            .or_else(|| item["name"].as_str())
                            .map(|name| (enum_key(name), Value::String(name.to_owned())))
                    })
                    .collect();
                values.insert(name.into(), items.into());
            }
        }
    }
    values.into()
}

fn enum_key(value: &str) -> String {
    let mut key = String::new();
    let mut previous_lower = false;
    for ch in value.chars() {
        if ch.is_ascii_uppercase() && previous_lower {
            key.push('_');
        }
        if ch.is_ascii_alphanumeric() {
            key.push(ch.to_ascii_uppercase());
        } else {
            key.push('_');
        }
        previous_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    key
}

fn rust_literal(value: &str) -> String {
    let mut hashes = String::from("#");
    while value.contains(&format!("\"{hashes}")) {
        hashes.push('#');
    }
    format!("r{hashes}\"{value}\"{hashes}")
}

fn output(path: &Path, expected: &str, check: bool) -> Result<(), String> {
    if check {
        let actual = fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}; run cargo xtask webext-api", path.display()))?;
        if actual != expected {
            return Err(format!(
                "generated file drift: {}; run cargo xtask webext-api",
                path.display()
            ));
        }
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, expected).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn coverage_table(catalog: &Catalog, inventory: &str) -> Result<String, String> {
    let inventory: Value = serde_json::from_str(inventory)
        .map_err(|error| format!("coverage inventory must be audited JSON: {error}"))?;
    let inventory = inventory["namespaces"].as_object().ok_or(
        "coverage inventory needs a namespaces object mapping names to explicit member arrays",
    )?;
    for members in inventory.values() {
        if !members
            .as_array()
            .is_some_and(|members| members.iter().all(Value::is_string))
        {
            return Err("coverage inventory namespace members must be string arrays".into());
        }
    }
    let mut report = String::from("# Chromium extension API snapshot coverage\n\nNeeded means explicitly named in the audited API-MATRIX inventory, including guarded and MV2 paths. Namespace demand does not imply demand for every member. This describes corpus demand, not implementation status.\n\n| Kind | Name | Needed by inventory |\n| --- | --- | --- |\n");
    for schema in &catalog.schemas {
        let namespace = schema["namespace"]
            .as_str()
            .ok_or("namespace missing name")?;
        let needed = inventory.contains_key(namespace);
        report.push_str(&format!(
            "| Namespace | `{namespace}` | {} |\n",
            if needed { "yes" } else { "no" }
        ));
        for (kind, field) in [("Method", "functions"), ("Event", "events")] {
            for member in schema[field].as_array().ok_or("missing members")? {
                let member = member["name"].as_str().ok_or("member missing name")?;
                let needed = inventory_contains(inventory, &format!("{namespace}.{member}"));
                report.push_str(&format!(
                    "| {kind} | `{namespace}.{member}` | {} |\n",
                    if needed { "yes" } else { "no" }
                ));
            }
        }
        if let Some(types) = schema["types"].as_array() {
            for ty in types {
                let id = ty["id"].as_str().ok_or("coverage type missing id")?;
                let path = if id.contains('.') {
                    id.to_owned()
                } else {
                    format!("{namespace}.{id}")
                };
                type_coverage(ty, &path, inventory, &mut report)?;
            }
        }
    }
    Ok(report)
}

fn inventory_contains(inventory: &Map<String, Value>, path: &str) -> bool {
    let Some((namespace, member)) = path.split_once('.') else {
        return false;
    };
    inventory
        .get(namespace)
        .and_then(Value::as_array)
        .is_some_and(|members| members.iter().any(|name| name.as_str() == Some(member)))
}

fn type_coverage(
    schema: &Value,
    path: &str,
    inventory: &Map<String, Value>,
    report: &mut String,
) -> Result<(), String> {
    for (kind, field) in [("Type method", "functions"), ("Type event", "events")] {
        if let Some(members) = schema[field].as_array() {
            for member in members {
                let name = member["name"]
                    .as_str()
                    .ok_or("coverage type member missing name")?;
                let path = format!("{path}.{name}");
                let needed = inventory_contains(inventory, &path);
                report.push_str(&format!(
                    "| {kind} | `{path}` | {} |\n",
                    if needed { "yes" } else { "no" }
                ));
            }
        }
    }
    if let Some(properties) = schema["properties"].as_object() {
        for (name, property) in properties {
            let path = format!("{path}.{name}");
            if property["type"] == "function" {
                let needed = inventory_contains(inventory, &path);
                report.push_str(&format!(
                    "| Type method | `{path}` | {} |\n",
                    if needed { "yes" } else { "no" }
                ));
            }
            type_coverage(property, &path, inventory, report)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_and_prose_free() {
        let mut schema = json!({"namespace":"sample","description":"private prose","functions":[{"name":"get","parameters":[{"name":"keys","type":"string","optional":true}],"returns_async":{"name":"callback","parameters":[]}}],"types":[{"id":"Mode","type":"string","enum":[{"name":"A","description":"more prose"}]}],"events":[],"properties":{"LIMIT":{"type":"integer","value":4}},"feature_rules":[]});
        strip_prose(&mut schema);
        let catalog = Catalog {
            schemas: vec![schema],
            features: BTreeMap::new(),
        };
        let first = generate(&catalog).unwrap();
        assert_eq!(first, generate(&catalog).unwrap());
        assert!(!first.0.contains("private prose"));
        assert!(!first.1.contains("more prose"));
        assert!(first.1.contains("\"optional\":true"));
        assert!(first
            .0
            .contains("crate::schema::validate(\"sample.get\", args)"));
    }

    #[test]
    fn prose_removal_preserves_properties_named_after_metadata() {
        let mut schema = json!({"description":"prose","properties":{"description":{"type":"string","description":"field prose"},"deprecated":{"type":"boolean"}}});
        strip_prose(&mut schema);
        assert!(schema.get("description").is_none());
        assert_eq!(schema["properties"]["description"]["type"], "string");
        assert!(schema["properties"]["description"]
            .get("description")
            .is_none());
        assert_eq!(schema["properties"]["deprecated"]["type"], "boolean");
    }

    #[test]
    fn nested_property_methods_and_cross_namespace_refs_expand() {
        let schemas = [
            json!({"namespace":"storage","types":[{"id":"Area","type":"object","functions":[{"name":"get","parameters":[{"name":"mode","$ref":"Mode"}]}]},{"id":"Mode","type":"string"}]}),
        ];
        let types = type_index(schemas.iter()).unwrap();
        let mut functions = Vec::new();
        expand_properties(
            "storage",
            "local",
            &json!({"$ref":"Area"}),
            &types,
            &mut functions,
            &mut Vec::new(),
            &mut BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(functions[0]["name"], "local.get");
        assert_eq!(functions[0]["parameters"][0]["$ref"], "storage.Mode");
        assert!(check_references("storage", &json!({"$ref":"Missing"}), &types).is_err());
    }

    #[test]
    fn feature_alternatives_remain_alternatives_with_parent_inheritance() {
        let features = BTreeMap::from([(
            "extensions/common/api/_api_features.json".into(),
            json!({"sample":[{"dependencies":["permission:first"],"contexts":["blessed_extension"]},{"dependencies":["permission:second"],"platforms":["chromeos"]}]}),
        )]);
        let rules = feature_rules("sample.get", &features);
        assert_eq!(rules[0]["name"], "sample");
        assert_eq!(rules[0]["rules"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn check_mode_detects_drift_without_rewriting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("generated");
        output(&path, "before", false).unwrap();
        assert!(output(&path, "after", true).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "before");
    }

    #[test]
    fn snapshot_inventory_rejects_modified_and_unlisted_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let source = b"[{}]";
        fs::write(root.join("sample.json"), source).unwrap();
        fs::write(
            root.join("UPSTREAM_FILES.toml"),
            format!(
                "[[files]]\npath = \"sample.json\"\nsha256 = \"{:x}\"\nbytes = {}\n",
                Sha256::digest(source),
                source.len()
            ),
        )
        .unwrap();
        let paths = vec![root.join("sample.json"), root.join("UPSTREAM_FILES.toml")];
        verify_snapshot(root, &paths).unwrap();
        fs::write(root.join("sample.json"), b"[[]]").unwrap();
        assert!(verify_snapshot(root, &paths)
            .unwrap_err()
            .contains("integrity mismatch"));
        fs::write(root.join("sample.json"), source).unwrap();
        fs::write(root.join("unexpected.json"), b"[]").unwrap();
        let mut paths = Vec::new();
        discover(root, &mut paths).unwrap();
        assert!(verify_snapshot(root, &paths)
            .unwrap_err()
            .contains("unlisted snapshot file"));
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_discovery_rejects_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("sample.json"), b"[]").unwrap();
        std::os::unix::fs::symlink("sample.json", directory.path().join("alias.json")).unwrap();
        assert!(discover(directory.path(), &mut Vec::new())
            .unwrap_err()
            .contains("symlink"));
    }

    #[test]
    fn coverage_uses_exact_inventory_members_and_includes_type_members() {
        let catalog = Catalog {
            features: BTreeMap::new(),
            schemas: vec![
                json!({"namespace":"runtime","functions":[{"name":"sendMessage"},{"name":"reload"}],"events":[],"types":[{"id":"Port","properties":{"postMessage":{"type":"function"}},"events":[{"name":"onDisconnect"}]}]}),
            ],
        };
        let table = coverage_table(
            &catalog,
            r#"{"namespaces":{"runtime":["sendMessage","Port.postMessage"]}}"#,
        )
        .unwrap();
        assert!(table.contains("`runtime.sendMessage` | yes"));
        assert!(table.contains("`runtime.reload` | no"));
        assert!(table.contains("Type method | `runtime.Port.postMessage` | yes"));
        assert!(table.contains("Type event | `runtime.Port.onDisconnect` | no"));
    }
}
