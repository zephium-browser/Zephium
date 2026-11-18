//! Chromium argument matching over the JSON bridge. Callbacks stay in the
//! binding; JSON cannot prove a function, Date or binary object's identity.
//! Only generated `serializableFunction` fields accept bounded source strings;
//! the binding must serialize actual functions before calling the bridge.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::api::generated;
use crate::errors::ApiError;
use crate::limits::MAX_ARGUMENT_DEPTH;

mod access;
pub use access::{access, Access};

const MAX_REFERENCE_DEPTH: usize = 64;
const MAX_PARAMETERS: usize = 64;

struct Method {
    feature_rules: Vec<Value>,
    namespace: String,
    invocation_name: String,
    setting_argument: Option<Value>,
    parameters: Vec<Value>,
    optional_callback: bool,
    signature: String,
}

struct Catalog {
    namespace_rules: HashMap<String, Vec<Value>>,
    methods: HashMap<String, Method>,
    references: HashMap<String, (String, Value)>,
}

static CATALOG: OnceLock<Result<Catalog, ()>> = OnceLock::new();

fn catalog() -> Result<&'static Catalog, ApiError> {
    CATALOG
        .get_or_init(|| Catalog::parse(generated::SCHEMAS_JSON))
        .as_ref()
        .map_err(|()| ApiError::malformed_arguments())
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn qualify(namespace: &str, reference: &str) -> String {
    if reference.contains('.') {
        reference.to_owned()
    } else {
        format!("{namespace}.{reference}")
    }
}

// Callback functions never cross JSON; the binding sends bounded callback IDs.
fn add_listener_methods(namespaces: &mut [Value]) -> Result<(), ()> {
    let event_type = namespaces
        .iter()
        .find(|ns| ns["namespace"] == "events")
        .and_then(|ns| array(ns, "types").iter().find(|kind| kind["id"] == "Event"));
    let operations = event_type
        .map(|kind| array(kind, "functions").to_vec())
        .unwrap_or_default();
    for namespace in namespaces {
        let mut methods = array(namespace, "functions").to_vec();
        for event in array(namespace, "events") {
            for operation in operations.iter().filter(|op| {
                matches!(
                    op["name"].as_str(),
                    Some("addListener" | "removeListener" | "hasListener" | "hasListeners")
                )
            }) {
                let mut method = operation.clone();
                method["name"] = format!(
                    "{}.{}",
                    event["name"].as_str().ok_or(())?,
                    operation["name"].as_str().ok_or(())?
                )
                .into();
                method["feature_rules"] = event["feature_rules"].clone();
                for parameter in method["parameters"].as_array_mut().ok_or(())? {
                    if parameter["type"] == "function" {
                        *parameter = serde_json::json!({"name":"callback", "type":"number", "bridge_callback":true, "minimum":1, "maximum":9_007_199_254_740_991_i64});
                    }
                }
                if operation["name"] == "addListener" {
                    let parameters = method["parameters"].as_array_mut().ok_or(())?;
                    parameters.extend(array(event, "extraParameters").iter().cloned());
                    if let Some(filter) = event_filter(event) {
                        parameters.push(filter);
                    }
                }
                methods.push(method);
            }
        }
        namespace["functions"] = methods.into();
    }
    Ok(())
}

fn event_filter(event: &Value) -> Option<Value> {
    let filters = event.get("filters")?.as_array()?;
    let mut properties = Map::new();
    for filter in filters {
        properties.insert(filter.get("name")?.as_str()?.to_owned(), filter.clone());
    }
    Some(
        serde_json::json!({"name":"filter", "type":"object", "optional":true, "properties":properties}),
    )
}

impl Catalog {
    fn parse(json: &str) -> Result<Self, ()> {
        let mut namespaces: Vec<Value> = serde_json::from_str(json).map_err(|_| ())?;
        add_listener_methods(&mut namespaces)?;
        let mut result = Self {
            namespace_rules: HashMap::new(),
            methods: HashMap::new(),
            references: HashMap::new(),
        };
        for schema in &namespaces {
            let namespace = schema.get("namespace").and_then(Value::as_str).ok_or(())?;
            result.namespace_rules.insert(
                namespace.to_owned(),
                array(schema, "feature_rules").to_vec(),
            );
            for kind in array(schema, "types") {
                let id = kind.get("id").and_then(Value::as_str).ok_or(())?;
                let mut kind = kind.clone();
                if namespace == "scripting" && id == "ScriptInjection" {
                    // Function arguments follow JSON serialization, including object nulls.
                    if let Some(items) = kind
                        .pointer_mut("/properties/args/items")
                        .and_then(Value::as_object_mut)
                    {
                        items.insert("preserveNull".into(), Value::Bool(true));
                    }
                }
                if result
                    .references
                    .insert(qualify(namespace, id), (namespace.to_owned(), kind))
                    .is_some()
                {
                    return Err(());
                }
            }
            for function in array(schema, "functions") {
                let name = function.get("name").and_then(Value::as_str).ok_or(())?;
                if function
                    .get("parameters")
                    .is_some_and(|value| !value.is_array())
                    || function
                        .get("returns_async")
                        .is_some_and(|value| !value.is_object())
                {
                    return Err(());
                }
                let mut parameters = array(function, "parameters").to_vec();
                if parameters.len() > MAX_PARAMETERS {
                    return Err(());
                }
                let optional_callback =
                    function.get("returns_async").is_some_and(callback_optional);
                let signature = signature(namespace, function);
                // Chromium messaging hooks use JSON.stringify for payloads.
                if matches!(
                    (namespace, name),
                    ("runtime", "sendMessage" | "sendNativeMessage") | ("tabs", "sendMessage")
                ) {
                    for parameter in &mut parameters {
                        if parameter.get("name").and_then(Value::as_str) == Some("message") {
                            parameter
                                .as_object_mut()
                                .ok_or(())?
                                .insert("preserveNull".into(), Value::Bool(true));
                            if let Some(additional) = parameter
                                .get_mut("additionalProperties")
                                .and_then(Value::as_object_mut)
                            {
                                additional.insert("preserveNull".into(), Value::Bool(true));
                            }
                        }
                    }
                }
                let method = Method {
                    feature_rules: array(function, "feature_rules").to_vec(),
                    namespace: namespace.to_owned(),
                    invocation_name: if namespace == "storage" && name.contains('.') {
                        format!("storage.{}", name.rsplit('.').next().ok_or(())?)
                    } else if namespace == "privacy" && name.contains('.') {
                        format!("types.ChromeSetting.{}", name.rsplit('.').next().ok_or(())?)
                    } else {
                        format!("{namespace}.{name}")
                    },
                    setting_argument: if namespace == "privacy" && name.ends_with(".set") {
                        Some(setting_argument(schema, name).ok_or(())?)
                    } else {
                        None
                    },
                    parameters,
                    optional_callback,
                    signature,
                };
                if result
                    .methods
                    .insert(format!("{namespace}.{name}"), method)
                    .is_some()
                {
                    return Err(());
                }
            }
        }
        Ok(result)
    }

    fn reference(&self, namespace: &str, name: &str) -> Option<(&str, &Value)> {
        self.references
            .get(&qualify(namespace, name))
            .map(|(namespace, spec)| (namespace.as_str(), spec))
    }

    fn correct_type(&self, namespace: &str, spec: &Value, value: &Value, depth: usize) -> bool {
        if depth >= MAX_REFERENCE_DEPTH {
            return false;
        }
        if let Some(reference) = spec.get("$ref").and_then(Value::as_str) {
            return self
                .reference(namespace, reference)
                .is_some_and(|(namespace, spec)| {
                    self.correct_type(namespace, spec, value, depth + 1)
                });
        }
        if let Some(choices) = spec.get("choices").and_then(Value::as_array) {
            return choices
                .iter()
                .any(|choice| self.correct_type(namespace, choice, value, depth + 1));
        }
        match spec.get("type").and_then(Value::as_str) {
            Some("integer") => is_int32(value),
            Some("number") => value.is_number(),
            Some("boolean") => value.is_boolean(),
            Some("string") => value.is_string(),
            Some("function") => flag(spec, "serializableFunction") && value.is_string(),
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            Some("any") => true,
            _ => false,
        }
    }

    fn resolve<'a>(&self, method: &Method, args: &[&'a Value]) -> Option<Vec<Option<&'a Value>>> {
        let mut slots = vec![None; method.parameters.len()];
        let mut failed = HashSet::new();
        if method.optional_callback
            && args.last().is_some_and(|value| value.is_null())
            && self.resolve_from(
                method,
                &args[..args.len() - 1],
                0,
                0,
                &mut slots,
                &mut failed,
            )
        {
            return Some(slots);
        }
        failed.clear();
        self.resolve_from(method, args, 0, 0, &mut slots, &mut failed)
            .then_some(slots)
    }

    fn resolve_from<'a>(
        &self,
        method: &Method,
        args: &[&'a Value],
        parameter: usize,
        argument: usize,
        slots: &mut [Option<&'a Value>],
        failed: &mut HashSet<(usize, usize)>,
    ) -> bool {
        if parameter == method.parameters.len() {
            return argument == args.len();
        }
        if args.len() - argument > method.parameters.len() - parameter
            || failed.contains(&(parameter, argument))
        {
            return false;
        }
        let spec = &method.parameters[parameter];
        let optional = flag(spec, "optional");
        if let Some(value) = args.get(argument) {
            let omitted = optional && value.is_null();
            if omitted || self.correct_type(&method.namespace, spec, value, 0) {
                slots[parameter] = if omitted { None } else { Some(*value) };
                if self.resolve_from(method, args, parameter + 1, argument + 1, slots, failed) {
                    return true;
                }
            }
        }
        if optional {
            slots[parameter] = None;
            if self.resolve_from(method, args, parameter + 1, argument, slots, failed) {
                return true;
            }
        }
        failed.insert((parameter, argument));
        false
    }

    fn normalize(&self, name: &str, args: &[Value]) -> Result<Vec<Value>, ApiError> {
        let method = self
            .methods
            .get(name)
            .ok_or_else(ApiError::unknown_method)?;
        if let (Some(spec), Some(value)) = (&method.setting_argument, args.first()) {
            if self
                .parse_value(&method.namespace, spec, value, 0, 0)
                .is_err()
            {
                return Err(ApiError::invalid_invocation());
            }
        }
        if args.len() > MAX_PARAMETERS + 1 {
            return Err(ApiError::no_matching_signature(
                &method.invocation_name,
                &method.signature,
            ));
        }
        let provided: Vec<&Value> = if name == "runtime.sendMessage" {
            massage_message(args).unwrap_or_else(|| args.iter().collect())
        } else {
            args.iter().collect()
        };
        let slots = self.resolve(method, &provided).ok_or_else(|| {
            ApiError::no_matching_signature(&method.invocation_name, &method.signature)
        })?;
        slots
            .into_iter()
            .zip(&method.parameters)
            .map(|(value, spec)| {
                let Some(value) = value else {
                    return Ok(Value::Null);
                };
                let normalized = self
                    .parse_value(&method.namespace, spec, value, 0, 0)
                    .map_err(|detail| {
                        ApiError::invalid_arguments(
                            &method.invocation_name,
                            &method.signature,
                            &format!(
                                "Error at parameter '{}': {detail}",
                                spec.get("name").and_then(Value::as_str).unwrap_or("")
                            ),
                        )
                    })?;
                Ok(normalized)
            })
            .collect()
    }

    fn parse_value(
        &self,
        namespace: &str,
        spec: &Value,
        value: &Value,
        depth: usize,
        references: usize,
    ) -> Result<Value, String> {
        if depth > MAX_ARGUMENT_DEPTH || references >= MAX_REFERENCE_DEPTH {
            return Err("Value is unserializable.".into());
        }
        if !self.correct_type(namespace, spec, value, references) {
            return Err(format!(
                "Invalid type: expected {}, found {}.",
                type_name(namespace, spec),
                value_type(value)
            ));
        }
        if let Some(reference) = spec.get("$ref").and_then(Value::as_str) {
            let (namespace, spec) = self
                .reference(namespace, reference)
                .ok_or_else(|| "Value is unserializable.".to_owned())?;
            return self.parse_value(namespace, spec, value, depth, references + 1);
        }
        if let Some(choices) = spec.get("choices").and_then(Value::as_array) {
            for choice in choices {
                if let Ok(parsed) =
                    self.parse_value(namespace, choice, value, depth, references + 1)
                {
                    return Ok(parsed);
                }
            }
            return Err("Value did not match any choice.".into());
        }
        match spec.get("type").and_then(Value::as_str) {
            Some("function") if flag(spec, "serializableFunction") => {
                let source = value.as_str().ok_or("Value is unserializable.")?;
                if source.is_empty() || source.len() > 1024 * 1024 || source.contains('\0') {
                    return Err("Value is unserializable.".into());
                }
                Ok(value.clone())
            }
            Some("integer" | "number") => {
                let number = value
                    .as_f64()
                    .ok_or_else(|| "Value must not be NaN or Infinity.".to_owned())?;
                if !number.is_finite() || (flag(spec, "bridge_callback") && number.fract() != 0.0) {
                    return Err("Value must not be NaN or Infinity.".into());
                }
                if let Some(minimum) = spec.get("minimum").and_then(Value::as_i64) {
                    if number < minimum as f64 {
                        return Err(format!("Value must be at least {minimum}."));
                    }
                }
                if let Some(maximum) = spec.get("maximum").and_then(Value::as_i64) {
                    if number > maximum as f64 {
                        return Err(format!("Value must be at most {maximum}."));
                    }
                }
                if spec.get("type").and_then(Value::as_str) == Some("integer") {
                    Ok(Value::from(number as i32))
                } else {
                    Ok(value.clone())
                }
            }
            Some("string") => {
                let string = value.as_str().unwrap_or_default();
                check_length(spec, string.encode_utf16().count(), false)?;
                let options: std::collections::BTreeSet<&str> = array(spec, "enum")
                    .iter()
                    .filter_map(|item| {
                        item.as_str()
                            .or_else(|| item.get("name").and_then(Value::as_str))
                    })
                    .collect();
                if !options.is_empty() && !options.contains(string) {
                    return Err(format!(
                        "Value must be one of {}.",
                        options.into_iter().collect::<Vec<_>>().join(", ")
                    ));
                }
                Ok(value.clone())
            }
            Some("object") => self.parse_object(namespace, spec, value, depth),
            Some("array") => {
                let values = value
                    .as_array()
                    .ok_or_else(|| "Value is unserializable.".to_owned())?;
                check_length(spec, values.len(), true)?;
                let items = spec
                    .get("items")
                    .ok_or_else(|| "Value is unserializable.".to_owned())?;
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        self.parse_value(namespace, items, value, depth + 1, 0)
                            .map_err(|error| format!("Error at index {index}: {error}"))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
            Some("any") => json_value(value, flag(spec, "preserveNull"), depth),
            Some("boolean") => Ok(value.clone()),
            _ => Err("Value is unserializable.".into()),
        }
    }

    fn parse_object(
        &self,
        namespace: &str,
        spec: &Value,
        value: &Value,
        depth: usize,
    ) -> Result<Value, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "Value is unserializable.".to_owned())?;
        let properties = spec.get("properties").and_then(Value::as_object);
        let additional = spec
            .get("additionalProperties")
            .filter(|value| value.is_object());
        let mut result = Map::new();
        for (key, value) in object {
            let declared = properties.and_then(|properties| properties.get(key));
            let property = if let Some(property) = declared.or(additional) {
                property
            } else if flag(spec, "ignoreAdditionalProperties") {
                continue;
            } else {
                return Err(format!("Unexpected property: '{key}'."));
            };
            if value.is_null() {
                if declared.is_some() && !flag(property, "optional") {
                    return Err(format!("Missing required property '{key}'."));
                }
                if flag(spec, "preserveNull") {
                    result.insert(key.clone(), Value::Null);
                }
                continue;
            }
            let parsed = self
                .parse_value(namespace, property, value, depth + 1, 0)
                .map_err(|error| format!("Error at property '{key}': {error}"))?;
            result.insert(key.clone(), parsed);
        }
        if let Some(properties) = properties {
            for (key, property) in properties.iter().collect::<BTreeMap<_, _>>() {
                if !flag(property, "optional") && !object.contains_key(key) {
                    return Err(format!("Missing required property '{key}'."));
                }
            }
        }
        if let Some(instance) = spec
            .get("isInstanceOf")
            .or_else(|| spec.get("instanceOf"))
            .and_then(Value::as_str)
        {
            return Err(format!("Value must be an instance of {instance}."));
        }
        Ok(Value::Object(result))
    }
}

fn setting_argument(schema: &Value, name: &str) -> Option<Value> {
    let mut property = schema;
    for part in name.strip_suffix(".set")?.split('.') {
        property = property.get("properties")?.get(part)?;
    }
    let value = property.get("value")?.as_array()?.get(1)?;
    Some(serde_json::json!({"type":"object","properties":{
        "scope":{"$ref":"types.ChromeSettingScope","optional":true},
        "value":value
    }}))
}

fn massage_message(args: &[Value]) -> Option<Vec<&Value>> {
    static NULL: Value = Value::Null;
    match args {
        [message] => Some(vec![&NULL, message, &NULL, &NULL]),
        [first, second] if first.is_string() || first.is_null() => {
            Some(vec![first, second, &NULL, &NULL])
        }
        [message, options] => Some(vec![&NULL, message, options, &NULL]),
        [extension, message, options] => Some(vec![extension, message, options, &NULL]),
        _ => None,
    }
}

fn is_int32(value: &Value) -> bool {
    value.as_f64().is_some_and(|number| {
        number.fract() == 0.0 && number >= f64::from(i32::MIN) && number <= f64::from(i32::MAX)
    })
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) if is_int32(value) => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_name(namespace: &str, spec: &Value) -> String {
    if let Some(reference) = spec.get("$ref").and_then(Value::as_str) {
        return qualify(namespace, reference);
    }
    if let Some(choices) = spec.get("choices").and_then(Value::as_array) {
        return format!(
            "[{}]",
            choices
                .iter()
                .map(|choice| type_name(namespace, choice))
                .collect::<Vec<_>>()
                .join("|")
        );
    }
    if spec.get("type").and_then(Value::as_str) == Some("object") {
        if let Some(instance) = spec
            .get("isInstanceOf")
            .or_else(|| spec.get("instanceOf"))
            .and_then(Value::as_str)
        {
            return instance.to_owned();
        }
    }
    spec.get("type")
        .and_then(Value::as_str)
        .unwrap_or("any")
        .to_owned()
}

fn signature(namespace: &str, method: &Value) -> String {
    let mut parts: Vec<String> = array(method, "parameters")
        .iter()
        .map(|parameter| {
            format!(
                "{}{} {}",
                if flag(parameter, "optional") {
                    "optional "
                } else {
                    ""
                },
                type_name(namespace, parameter),
                parameter.get("name").and_then(Value::as_str).unwrap_or("")
            )
        })
        .collect();
    if let Some(asynchronous) = method
        .get("returns_async")
        .filter(|value| value.is_object())
    {
        parts.push(format!(
            "{}function callback",
            if callback_optional(asynchronous) {
                "optional "
            } else {
                ""
            }
        ));
    }
    parts.join(", ")
}

fn callback_optional(asynchronous: &Value) -> bool {
    let promises = asynchronous.get("does_not_support_promises").is_none()
        && asynchronous
            .get("supportsPromises")
            .and_then(Value::as_bool)
            != Some(false);
    promises || flag(asynchronous, "optional")
}

fn check_length(spec: &Value, length: usize, is_array: bool) -> Result<(), String> {
    let minimum = spec
        .get("minLength")
        .or_else(|| spec.get("minItems"))
        .and_then(Value::as_u64);
    let maximum = spec
        .get("maxLength")
        .or_else(|| spec.get("maxItems"))
        .and_then(Value::as_u64);
    let (kind, unit) = if is_array {
        ("Array", "items")
    } else {
        ("String", "characters")
    };
    if let Some(minimum) = minimum {
        if (length as u64) < minimum {
            return Err(format!(
                "{kind} must have at least {minimum} {unit}; found {length}."
            ));
        }
    }
    if let Some(maximum) = maximum {
        if length as u64 > maximum {
            return Err(format!(
                "{kind} must have at most {maximum} {unit}; found {length}."
            ));
        }
    }
    Ok(())
}

fn json_value(value: &Value, preserve_null: bool, depth: usize) -> Result<Value, String> {
    if depth > MAX_ARGUMENT_DEPTH {
        return Err("Value is unserializable.".into());
    }
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| json_value(value, preserve_null, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(object) => object
            .iter()
            .filter(|(_, value)| preserve_null || !value.is_null())
            .map(|(key, value)| {
                json_value(value, preserve_null, depth + 1).map(|value| (key.clone(), value))
            })
            .collect::<Result<Map<_, _>, _>>()
            .map(Value::Object),
        _ => Ok(value.clone()),
    }
}

/// Whether Chromium's generated permission metadata recognizes this API name.
pub fn known_permission(name: &str) -> bool {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    NAMES
        .get_or_init(|| {
            let Ok(Value::Object(files)) = serde_json::from_str::<Value>(generated::FEATURES_JSON)
            else {
                return HashSet::new();
            };
            files
                .into_iter()
                .filter(|(path, _)| {
                    path.ends_with("/_permission_features.json")
                        || path.ends_with("/_permission_features.override.json")
                })
                .flat_map(|(_, features)| {
                    features
                        .as_object()
                        .map(|object| object.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .collect()
        })
        .contains(name)
}

/// Checks a generated method's arguments using Chromium's type-first optional
/// argument shifting, then full schema validation. The binding must check
/// callback presence and remove the callback before JSON serialization.
pub fn validate(name: &str, args: &[Value]) -> Result<(), ApiError> {
    normalize(name, args).map(|_| ())
}

/// Validates and converts arguments into positional slots for the native handler.
/// Omitted optionals become null; object nulls follow `preserveNull`. Functions,
/// Dates and binary identities require binding-side handling and cannot be
/// represented by caller-supplied JSON marker objects.
pub fn normalize(name: &str, args: &[Value]) -> Result<Vec<Value>, ApiError> {
    catalog()?.normalize(source_method(name), args)
}

/// Validates the binding's wire arguments. Native message bodies are opaque
/// JSON text on the bridge; their public object type is checked by the binding
/// before serialization. All target and optional slots retain generated checks.
pub fn normalize_wire(name: &str, args: &[Value]) -> Result<Vec<Value>, ApiError> {
    if name != "runtime.sendNativeMessage" {
        return normalize(name, args);
    }
    let payload = args
        .get(1)
        .and_then(Value::as_str)
        .ok_or_else(ApiError::malformed_arguments)?;
    let mut projected = args.to_vec();
    projected[1] = Value::Object(Map::new());
    let mut normalized = normalize(name, &projected)?;
    normalized[1] = Value::String(payload.to_owned());
    Ok(normalized)
}

pub(crate) fn source_method(name: &str) -> &str {
    // The binding uses a background-only internal name for this notification.
    if name == "events.completeRegistration" {
        "runtime.markListenerRegistrationComplete"
    } else {
        name
    }
}

/// Whether this method (including a generated event's binding operation) exists.
pub fn contains(name: &str) -> bool {
    catalog().is_ok_and(|catalog| catalog.methods.contains_key(source_method(name)))
}

/// The exact Chromium invocation signature, including its callback parameter.
pub fn invocation_signature(name: &str) -> Option<&'static str> {
    catalog()
        .ok()?
        .methods
        .get(name)
        .map(|method| method.signature.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn arguments(value: &Value) -> &[Value] {
        value.as_array().expect("arguments")
    }

    fn converted(name: &str, value: Value) -> Vec<Value> {
        normalize(name, arguments(&value)).expect("valid arguments")
    }

    fn error(name: &str, value: Value) -> String {
        validate(name, arguments(&value))
            .expect_err("invalid arguments")
            .message()
            .to_owned()
    }

    fn fixture(parameters: Value, types: Value) -> Catalog {
        Catalog::parse(&json!([{"namespace":"test", "types":types, "functions":[{"name":"call", "parameters":parameters}]}]).to_string()).expect("catalog")
    }

    fn fixture_error(parameters: Value, value: Value) -> String {
        fixture(parameters, json!([]))
            .normalize("test.call", arguments(&value))
            .expect_err("invalid")
            .message()
            .to_owned()
    }

    #[test]
    fn tabs_query_checks_choices_refs_properties_and_exact_signature() {
        assert_eq!(
            invocation_signature("tabs.query"),
            Some("object queryInfo, optional function callback")
        );
        assert_eq!(
            converted(
                "tabs.query",
                json!([{"url":["https://example.com/*"],"status":"complete"}])
            ),
            vec![json!({"url":["https://example.com/*"],"status":"complete"})]
        );
        assert_eq!(error("tabs.query", json!([])), "Error in invocation of tabs.query(object queryInfo, optional function callback): No matching signature.");
        assert_eq!(error("tabs.query", json!([{"active":"yes"}])), "Error in invocation of tabs.query(object queryInfo, optional function callback): Error at parameter 'queryInfo': Error at property 'active': Invalid type: expected boolean, found string.");
        assert!(error("tabs.query", json!([{"status":"invalid"}]))
            .ends_with("Value must be one of complete, loading, unloaded."));
        assert!(error("tabs.query", json!([{"url":[3]}]))
            .ends_with("Error at property 'url': Value did not match any choice."));
        assert!(error("tabs.query", json!([{"notAFilter":true}]))
            .ends_with("Unexpected property: 'notAFilter'."));
        assert_eq!(
            converted("tabs.query", json!([{"active":null},null])),
            vec![json!({})]
        );
    }

    #[test]
    fn storage_get_preserves_defaults_and_accepts_omitted_or_null_keys() {
        assert_eq!(
            invocation_signature("storage.local.get"),
            Some("optional [string|array|object] keys, optional function callback")
        );
        for input in [json!([]), json!([null]), json!([null, null])] {
            assert_eq!(converted("storage.local.get", input), vec![Value::Null]);
        }
        let defaults = json!({"key":null,"nested":{"null":null,"array":[null,{"key":null}]}});
        assert_eq!(
            converted("storage.local.get", json!([defaults])),
            vec![defaults]
        );
        assert_eq!(
            converted("storage.local.get", json!([["first", "second"]])),
            vec![json!(["first", "second"])]
        );
        assert!(error("storage.local.get", json!([["first", 3]]))
            .ends_with("Error at parameter 'keys': Value did not match any choice."));
        assert!(error("storage.local.get", json!([7])).ends_with("No matching signature."));
        assert_eq!(error("storage.local.get",json!([7])),"Error in invocation of storage.get(optional [string|array|object] keys, optional function callback): No matching signature.");
        assert!(error("storage.local.get", json!([{}, {}, {}])).ends_with("No matching signature."));
    }

    #[test]
    fn tabs_message_checks_int32_ranges_and_optional_options() {
        assert_eq!(
            invocation_signature("tabs.sendMessage"),
            Some("integer tabId, any message, optional object options, optional function callback")
        );
        assert_eq!(
            converted("tabs.sendMessage", json!([1,{"key":null}])),
            vec![json!(1), json!({"key":null}), Value::Null]
        );
        assert_eq!(
            converted("tabs.sendMessage", json!([1,null,{"frameId":0},null])),
            vec![json!(1), Value::Null, json!({"frameId":0})]
        );
        assert!(error("tabs.sendMessage", json!([-1, "message"]))
            .ends_with("Error at parameter 'tabId': Value must be at least 0."));
        for input in [
            json!([2147483648_i64, "message"]),
            json!([1.5, "message"]),
            json!([1, "message", "options"]),
        ] {
            assert!(error("tabs.sendMessage", input).ends_with("No matching signature."));
        }
        assert!(
            error("tabs.sendMessage", json!([1,"message",{"frameId":-1}]))
                .ends_with("Error at property 'frameId': Value must be at least 0.")
        );
        assert_eq!(
            converted("tabs.sendMessage", json!([-0.0, "message"])),
            vec![json!(0), json!("message"), Value::Null]
        );
    }

    #[test]
    fn runtime_message_applies_chromes_custom_overload_selection() {
        assert_eq!(invocation_signature("runtime.sendMessage"), Some("optional string extensionId, any message, optional object options, optional function callback"));
        let message = json!({"nested":{"key":null}});
        assert_eq!(
            converted("runtime.sendMessage", json!([message])),
            vec![Value::Null, message.clone(), Value::Null]
        );
        assert_eq!(
            converted("runtime.sendMessage", json!([message, {}])),
            vec![Value::Null, message.clone(), json!({})]
        );
        assert_eq!(
            converted("runtime.sendMessage", json!(["extension", message])),
            vec![json!("extension"), message, Value::Null]
        );
        assert_eq!(
            converted("runtime.sendMessage", json!(["string-message"])),
            vec![Value::Null, json!("string-message"), Value::Null]
        );
        assert_eq!(
            converted("runtime.sendMessage", json!(["string", {}])),
            vec![json!("string"), json!({}), Value::Null]
        );
        assert_eq!(
            converted("runtime.sendMessage", json!([null])),
            vec![Value::Null, Value::Null, Value::Null]
        );
        assert!(error("runtime.sendMessage", json!([])).ends_with("No matching signature."));
        assert!(error("runtime.sendMessage", json!([{}, "bad-options"]))
            .ends_with("No matching signature."));
        assert!(
            error("runtime.sendMessage", json!([{}, {}, null])).ends_with("No matching signature.")
        );
        assert!(error("runtime.sendMessage", json!([{}, {}, null, null]))
            .ends_with("No matching signature."));
        assert_eq!(
            converted(
                "runtime.sendMessage",
                json!(["extension",{"message":true},null])
            ),
            vec![json!("extension"), json!({"message":true}), Value::Null]
        );
        assert_eq!(
            converted(
                "runtime.sendMessage",
                json!(["extension",{"message":true},null,null])
            ),
            vec![json!("extension"), json!({"message":true}), Value::Null]
        );
        assert!(
            error("runtime.sendMessage", json!([{}, {"unexpected":true}]))
                .ends_with("Unexpected property: 'unexpected'.")
        );
        let payload = json!({"key":null,"nested":{"key":null},"array":[{"key":null}]});
        assert_eq!(
            converted("runtime.sendNativeMessage", json!(["host", payload])),
            vec![json!("host"), payload]
        );
    }

    #[test]
    fn scripting_validates_injection_dictionary_and_ref_signature() {
        assert_eq!(
            invocation_signature("scripting.executeScript"),
            Some("scripting.ScriptInjection injection, optional function callback")
        );
        assert!(validate(
            "scripting.executeScript",
            arguments(&json!([{"target":{"tabId":1},"files":["script.js"]}]))
        )
        .is_ok());
        assert!(
            error("scripting.executeScript", json!([{"files":["script.js"]}]))
                .contains("Missing required property 'target'.")
        );
        assert!(error(
            "scripting.executeScript",
            json!([{"target":{"tabId":"1"},"files":["script.js"]}])
        )
        .contains("Invalid type: expected integer, found string."));
        assert!(error(
            "scripting.executeScript",
            json!([{"target":{"tabId":1},"func":{"__function":"spoof"}}])
        )
        .contains("Invalid type: expected function, found object."));
    }

    #[test]
    fn only_serializable_function_fields_accept_bounded_source() {
        let injection = json!([{"target":{"tabId":1},"func":"(value) => value","args":[42,{"nested":{"key":null}}]}]);
        assert_eq!(
            converted("scripting.executeScript", injection.clone()),
            injection.as_array().unwrap().clone()
        );
        for source in [
            String::new(),
            "x".repeat(1024 * 1024 + 1),
            "x\0y".to_owned(),
        ] {
            assert!(validate(
                "scripting.executeScript",
                arguments(&json!([{"target":{"tabId":1},"func":source}]))
            )
            .is_err());
        }
        let catalog = fixture(
            json!([{"name":"value","type":"object","properties":{"callback":{"type":"function"}}}]),
            json!([]),
        );
        assert!(catalog
            .normalize("test.call", &[json!({"callback":"() => 1"})])
            .is_err());
    }

    #[test]
    fn chrome_settings_apply_typed_value_hooks_and_original_invocation_names() {
        let boolean = "privacy.services.passwordSavingEnabled.set";
        assert_eq!(
            converted(boolean, json!([{"value":true}])),
            vec![json!({"value":true})]
        );
        for value in [
            json!([{"value":"yes"}]),
            json!([{"value":null}]),
            json!([{}]),
            json!([null]),
            json!([{"value":true,"unknown":true}]),
        ] {
            assert_eq!(error(boolean, value), "Invalid invocation");
        }
        assert!(error(boolean, json!([]))
            .starts_with("Error in invocation of types.ChromeSetting.set("));
        assert!(
            error("privacy.services.passwordSavingEnabled.get", json!([false]))
                .starts_with("Error in invocation of types.ChromeSetting.get(")
        );
        let policy = "privacy.network.webRTCIPHandlingPolicy.set";
        assert!(validate(policy, arguments(&json!([{"value":"default"}]))).is_ok());
        assert_eq!(
            error(policy, json!([{"value":"invalid"}])),
            "Invalid invocation"
        );
        assert_eq!(
            error(policy, json!([{"value":"default","scope":"invalid"}])),
            "Invalid invocation"
        );
    }

    #[test]
    fn shifting_uses_basic_types_then_validates_without_backtracking() {
        let schema = fixture(
            json!([
                {"name":"first","type":"object","optional":true,"properties":{"ok":{"type":"boolean"}}},
                {"name":"second","type":"object","optional":true,"additionalProperties":{"type":"any"}},
                {"name":"last","type":"string"}
            ]),
            json!([]),
        );
        assert_eq!(
            schema
                .normalize("test.call", arguments(&json!(["last"])))
                .expect("shift"),
            vec![Value::Null, Value::Null, json!("last")]
        );
        assert!(schema
            .normalize("test.call", arguments(&json!([{"different":true},"last"])))
            .expect_err("first must validate")
            .message()
            .contains("Error at parameter 'first': Unexpected property: 'different'."));
        let input = vec![Value::Null; MAX_PARAMETERS + 2];
        assert!(schema.normalize("test.call", &input).is_err());
    }

    #[test]
    fn ambiguous_optional_parameters_have_a_bounded_failed_search() {
        let mut parameters =
            vec![json!({"name":"optional","type":"boolean","optional":true}); MAX_PARAMETERS - 1];
        parameters.push(json!({"name":"required","type":"string"}));
        let schema = fixture(Value::Array(parameters), json!([]));
        let method = &schema.methods["test.call"];
        let values = vec![Value::Bool(true); MAX_PARAMETERS / 2];
        let provided: Vec<_> = values.iter().collect();
        let mut slots = vec![None; MAX_PARAMETERS];
        let mut failed = HashSet::new();
        assert!(!schema.resolve_from(method, &provided, 0, 0, &mut slots, &mut failed));
        assert!(failed.len() <= (MAX_PARAMETERS + 1) * (provided.len() + 1));
    }

    #[test]
    fn string_lengths_use_utf16_and_enums_are_sorted() {
        let parameter = json!([{"name":"text","type":"string","minLength":2,"maxLength":2}]);
        assert!(fixture(parameter.clone(), json!([]))
            .normalize("test.call", arguments(&json!(["🦀"])))
            .is_ok());
        assert!(fixture_error(parameter.clone(), json!(["x"]))
            .ends_with("String must have at least 2 characters; found 1."));
        assert!(fixture_error(parameter, json!(["🦀x"]))
            .ends_with("String must have at most 2 characters; found 3."));
        assert!(fixture_error(
            json!([{"name":"text","type":"string","enum":["z",{"name":"a"}]}]),
            json!(["bad"])
        )
        .ends_with("Value must be one of a, z."));
    }

    #[test]
    fn arrays_ranges_and_nested_errors_have_chromes_details() {
        let parameter = json!([{"name":"items","type":"array","minItems":1,"maxItems":2,"items":{"type":"integer","minimum":0,"maximum":3}}]);
        assert!(fixture_error(parameter.clone(), json!([[]]))
            .ends_with("Array must have at least 1 items; found 0."));
        assert!(fixture_error(parameter.clone(), json!([[1, 2, 3]]))
            .ends_with("Array must have at most 2 items; found 3."));
        assert!(fixture_error(parameter, json!([[1, 4]]))
            .ends_with("Error at index 1: Value must be at most 3."));
        assert!(fixture_error(
            json!([{"name":"x","type":"object","properties":{"required":{"type":"any"}}}]),
            json!([{"required":null}])
        )
        .ends_with("Missing required property 'required'."));
    }

    #[test]
    fn null_conversion_additional_properties_and_ignore_match_chrome() {
        let schema = fixture(
            json!([
                {"name":"strip","type":"any"},
                {"name":"preserve","type":"any","preserveNull":true},
                {"name":"typed","type":"object","additionalProperties":{"type":"integer"}},
                {"name":"ignore","type":"object","ignoreAdditionalProperties":true}
            ]),
            json!([]),
        );
        let value = json!({"null":null,"nested":{"null":null},"array":[null,{"null":null}]});
        assert_eq!(
            schema
                .normalize(
                    "test.call",
                    arguments(&json!([value,value,{"null":null,"ok":1},{"ignored":3}]))
                )
                .expect("convert"),
            vec![
                json!({"nested":{},"array":[null,{}]}),
                value,
                json!({"ok":1}),
                json!({})
            ]
        );
        assert!(schema
            .normalize("test.call", arguments(&json!([0,0,{"bad":"string"},{}])))
            .expect_err("typed additional")
            .message()
            .contains("Error at property 'bad': Invalid type: expected integer, found string."));
    }

    #[test]
    fn json_cannot_impersonate_binary_dates_or_functions() {
        assert!(fixture_error(
            json!([{"name":"buffer","type":"binary"}]),
            json!([[1, 2, 3]])
        )
        .ends_with("No matching signature."));
        assert!(fixture_error(
            json!([{"name":"function","type":"function"}]),
            json!([{"__function":"spoof"}])
        )
        .ends_with("No matching signature."));
        assert!(fixture_error(json!([{"name":"date","type":"object","isInstanceOf":"Date","additionalProperties":{"type":"any"}}]),json!([{"constructor":"Date"}])).ends_with("Value must be an instance of Date."));
        assert!(fixture_error(
            json!([{"name":"date","type":"object","isInstanceOf":"Date"}]),
            json!(["2026-11-14T00:00:00Z"])
        )
        .ends_with("No matching signature."));
    }

    #[test]
    fn broken_refs_cycles_and_deep_values_fail_closed() {
        for reference in ["Missing", "Loop"] {
            let schema = fixture(
                json!([{"name":"input","$ref":reference}]),
                json!([{"id":"Loop","$ref":"Loop"}]),
            );
            assert!(schema
                .normalize("test.call", arguments(&json!([{}])))
                .is_err());
        }
        let schema = fixture(json!([{"name":"value","type":"any"}]), json!([]));
        let mut nested = Value::Null;
        for _ in 0..MAX_ARGUMENT_DEPTH + 1 {
            nested = json!([nested]);
        }
        assert!(schema.normalize("test.call", &[nested]).is_err());
        assert!(Catalog::parse("invalid JSON").is_err());
        assert_eq!(
            validate("missing.method", &[]),
            Err(ApiError::unknown_method())
        );
    }

    #[test]
    fn cross_namespace_refs_and_callback_modes_keep_their_signatures() {
        let schema = Catalog::parse(&json!([
            {"namespace":"shared","types":[{"id":"Count","type":"integer","minimum":0}]},
            {"namespace":"test","functions":[{"name":"call","parameters":[{"name":"count","$ref":"shared.Count"}],"returns_async":{"optional":false,"does_not_support_promises":true}}]}
        ]).to_string()).expect("catalog");
        assert_eq!(
            schema.methods["test.call"].signature,
            "shared.Count count, function callback"
        );
        assert_eq!(
            schema
                .normalize("test.call", &[json!(1)])
                .expect("binding removed callback"),
            vec![json!(1)]
        );
        assert!(schema
            .normalize("test.call", &[json!(-1)])
            .expect_err("range")
            .message()
            .ends_with("Error at parameter 'count': Value must be at least 0."));
        assert!(schema
            .normalize("test.call", &[json!(1), Value::Null])
            .is_err());
        assert_eq!(
            signature(
                "test",
                &json!({"parameters":[],"returns_async":{"supportsPromises":false}})
            ),
            "function callback"
        );
    }
}
