use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::json::strip_comments;

#[derive(Default)]
struct Interface {
    name: String,
    parent: Option<String>,
    attributes: Map<String, Value>,
    methods: Vec<Value>,
    properties: Map<String, Value>,
}

struct Parser {
    tokens: Vec<String>,
    index: usize,
    webidl: bool,
    namespace: Option<String>,
    attributes: Map<String, Value>,
    types: BTreeMap<String, Value>,
    interfaces: Vec<Interface>,
}

pub(super) fn parse(source: &str, webidl: bool) -> Result<Value, String> {
    let mut parser = Parser {
        tokens: tokenize(source)?,
        index: 0,
        webidl,
        namespace: None,
        attributes: Map::new(),
        types: BTreeMap::new(),
        interfaces: Vec::new(),
    };
    parser.declarations(false)?;
    parser.finish()
}

fn tokenize(source: &str) -> Result<Vec<String>, String> {
    let source = strip_comments(source)?;
    let mut chars = source.char_indices().peekable();
    let mut tokens = Vec::new();
    while let Some((start, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '"' {
            let mut escaped = false;
            let mut end = None;
            for (index, ch) in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    end = Some(index + 1);
                    break;
                }
            }
            let end = end.ok_or("unterminated IDL string")?;
            tokens.push(source[start..end].to_owned());
        } else if ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.') {
            let mut end = start + ch.len_utf8();
            while let Some(&(index, ch)) = chars.peek() {
                if ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                    chars.next();
                    end = index + ch.len_utf8();
                } else {
                    break;
                }
            }
            tokens.push(source[start..end].to_owned());
        } else {
            tokens.push(ch.to_string());
        }
    }
    Ok(tokens)
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.index).map(String::as_str)
    }

    fn take(&mut self) -> Result<String, String> {
        let token = self
            .tokens
            .get(self.index)
            .cloned()
            .ok_or_else(|| format!("unexpected end of IDL at token {}", self.index))?;
        self.index += 1;
        Ok(token)
    }

    fn eat(&mut self, token: &str) -> bool {
        if self.peek() == Some(token) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<(), String> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(format!(
                "expected {token:?}, got {:?} at token {}",
                self.peek(),
                self.index
            ))
        }
    }

    fn attributes(&mut self) -> Result<Map<String, Value>, String> {
        let mut attributes = Map::new();
        while self.eat("[") {
            while !self.eat("]") {
                let name = self.take()?;
                let value = if self.eat("=") {
                    if self.eat("(") {
                        let mut values = Vec::new();
                        while !self.eat(")") {
                            values.push(scalar(&self.take()?));
                            if !self.eat(",") {
                                self.expect(")")?;
                                break;
                            }
                        }
                        Value::Array(values)
                    } else {
                        scalar(&self.take()?)
                    }
                } else {
                    Value::Bool(true)
                };
                let (name, value) = match name.as_str() {
                    "deprecated" => ("deprecated".into(), Value::Bool(true)),
                    "doesNotSupportPromises" => {
                        ("does_not_support_promises".into(), Value::Bool(true))
                    }
                    "NullIsSameAs" => ("null_is_same_as".into(), value),
                    _ => (name, value),
                };
                attributes.insert(name, value);
                if !self.eat(",") {
                    self.expect("]")?;
                    break;
                }
            }
        }
        Ok(attributes)
    }

    fn declarations(&mut self, nested: bool) -> Result<(), String> {
        while self.peek().is_some() {
            if nested && self.eat("}") {
                self.expect(";")?;
                return Ok(());
            }
            let attributes = self.attributes()?;
            self.eat("partial");
            match self.take()?.as_str() {
                "namespace" => {
                    self.namespace = Some(self.take()?);
                    self.attributes.extend(attributes);
                    self.expect("{")?;
                    self.declarations(true)?;
                }
                "dictionary" => self.dictionary(attributes)?,
                "enum" => self.enumeration(attributes)?,
                "callback" => self.callback(attributes)?,
                "typedef" => {
                    let mut ty = self.ty()?;
                    let name = self.take()?;
                    self.expect(";")?;
                    if let Some(external) = attributes.get("ExternalExtensionType") {
                        ty = json!({"$ref": external});
                    }
                    ty.as_object_mut()
                        .ok_or("invalid typedef")?
                        .extend(attributes);
                    ty["id"] = name.clone().into();
                    self.types.insert(name, ty);
                }
                "interface" => self.interface(attributes)?,
                token => return Err(format!("unsupported IDL declaration {token:?}")),
            }
        }
        if nested {
            return Err("unterminated IDL namespace".into());
        }
        Ok(())
    }

    fn dictionary(&mut self, attributes: Map<String, Value>) -> Result<(), String> {
        let name = self.take()?;
        let parent = if self.eat(":") {
            Some(self.take()?)
        } else {
            None
        };
        self.expect("{")?;
        let mut properties = Map::new();
        while !self.eat("}") {
            let attributes = self.attributes()?;
            let required = self.eat("required");
            let mut ty = self.ty()?;
            let name = self.take()?;
            if self.eat("=") {
                ty["default"] = self.default_value()?;
            }
            self.expect(";")?;
            if (self.webidl && !required) || (!self.webidl && ty.get("optional").is_some()) {
                ty["optional"] = true.into();
            }
            ty.as_object_mut()
                .ok_or("invalid property")?
                .extend(attributes);
            properties.insert(name, ty);
        }
        self.expect(";")?;
        let mut ty = json!({"id":name, "type":"object", "properties":properties});
        if let Some(parent) = parent {
            ty["extends"] = parent.into();
        }
        ty.as_object_mut()
            .ok_or("invalid dictionary")?
            .extend(attributes);
        if let Some(existing) = self.types.get_mut(&name) {
            let properties = ty["properties"].as_object().ok_or("invalid properties")?;
            existing["properties"]
                .as_object_mut()
                .ok_or("invalid partial dictionary")?
                .extend(properties.clone());
        } else {
            self.types.insert(name, ty);
        }
        Ok(())
    }

    fn enumeration(&mut self, attributes: Map<String, Value>) -> Result<(), String> {
        let name = self.take()?;
        self.expect("{")?;
        let mut values = Vec::new();
        while !self.eat("}") {
            values.push(Value::String(unquote(&self.take()?)));
            if !self.eat(",") {
                self.expect("}")?;
                break;
            }
        }
        self.expect(";")?;
        let mut ty = json!({"id":name, "type":"string", "enum":values});
        ty.as_object_mut().ok_or("invalid enum")?.extend(attributes);
        self.types.insert(name, ty);
        Ok(())
    }

    fn callback(&mut self, attributes: Map<String, Value>) -> Result<(), String> {
        let name = self.take()?;
        self.expect("=")?;
        let returns = self.ty()?;
        let parameters = self.parameters()?;
        self.expect(";")?;
        let mut ty = json!({"id":name,"type":"function","parameters":parameters});
        if returns["type"] != "undefined" {
            ty["returns"] = returns;
        }
        ty.as_object_mut()
            .ok_or("invalid callback")?
            .extend(attributes);
        self.types.insert(name, ty);
        Ok(())
    }

    fn interface(&mut self, attributes: Map<String, Value>) -> Result<(), String> {
        let mut interface = Interface {
            name: self.take()?,
            attributes,
            ..Interface::default()
        };
        if self.eat(":") {
            interface.parent = Some(self.take()?);
        }
        self.expect("{")?;
        while !self.eat("}") {
            let attributes = self.attributes()?;
            self.eat("static");
            self.eat("readonly");
            let constant = self.eat("const");
            let attribute = self.eat("attribute");
            let mut ty = self.ty()?;
            let name = self.take()?;
            if constant {
                self.expect("=")?;
                ty["value"] = self.default_value()?;
                if let Some(value) = attributes.get("StringValue") {
                    ty["type"] = "string".into();
                    ty["value"] = value.clone();
                }
                ty.as_object_mut()
                    .ok_or("invalid constant")?
                    .extend(attributes);
                interface.properties.insert(name, ty);
            } else if attribute {
                ty.as_object_mut()
                    .ok_or("invalid attribute")?
                    .extend(attributes);
                interface.properties.insert(name, ty);
            } else {
                let parameters = self.parameters()?;
                let mut method = json!({"name":name, "type":"function", "parameters":parameters});
                if let Some(promise) = ty.get("promise") {
                    let results = if promise["type"] == "undefined" {
                        Vec::new()
                    } else {
                        let mut result = promise.clone();
                        result["name"] = "result".into();
                        vec![result]
                    };
                    method["returns_async"] = json!({"name":"callback","optional":true,"parameters":results,"supportsPromises":true});
                } else if ty["type"] != "undefined" {
                    method["returns"] = ty;
                }
                method
                    .as_object_mut()
                    .ok_or("invalid method")?
                    .extend(attributes);
                interface.methods.push(method);
            }
            self.expect(";")?;
        }
        self.expect(";")?;
        self.interfaces.push(interface);
        Ok(())
    }

    fn parameters(&mut self) -> Result<Vec<Value>, String> {
        self.expect("(")?;
        let mut parameters = Vec::new();
        while !self.eat(")") {
            let attributes = self.attributes()?;
            let optional = self.eat("optional");
            let mut ty = self.ty()?;
            let variadic = self.eat("...");
            ty["name"] = self.take()?.into();
            if optional {
                ty["optional"] = true.into();
            }
            if variadic {
                ty["variadic"] = true.into();
            }
            if self.eat("=") {
                ty["default"] = self.default_value()?;
            }
            ty.as_object_mut()
                .ok_or("invalid argument")?
                .extend(attributes);
            parameters.push(ty);
            if !self.eat(",") {
                self.expect(")")?;
                break;
            }
        }
        Ok(parameters)
    }

    fn ty(&mut self) -> Result<Value, String> {
        let mut value = if self.eat("(") {
            let mut choices = vec![self.ty()?];
            while self.eat("or") {
                choices.push(self.ty()?);
            }
            self.expect(")")?;
            json!({"choices":choices})
        } else {
            let mut token = self.take()?;
            if matches!(token.as_str(), "unsigned" | "unrestricted") {
                token.push(' ');
                token.push_str(&self.take()?);
            }
            if token == "long" && self.eat("long") {
                token.push_str(" long");
            }
            if token == "unsigned long" && self.eat("long") {
                token.push_str(" long");
            }
            match token.as_str() {
                "Promise" => {
                    self.expect("<")?;
                    let result = self.ty()?;
                    self.expect(">")?;
                    json!({"promise":result})
                }
                "sequence" | "FrozenArray" | "ObservableArray" => {
                    self.expect("<")?;
                    let items = self.ty()?;
                    self.expect(">")?;
                    json!({"type":"array","items":items})
                }
                "record" => {
                    self.expect("<")?;
                    self.ty()?;
                    self.expect(",")?;
                    let item = self.ty()?;
                    self.expect(">")?;
                    json!({"type":"object","additionalProperties":item})
                }
                "DOMString" | "USVString" | "ByteString" => json!({"type":"string"}),
                "boolean" => json!({"type":"boolean"}),
                "byte" | "octet" | "short" | "unsigned short" | "long" | "unsigned long"
                | "long long" | "unsigned long long" => json!({"type":"integer"}),
                "double" | "float" | "unrestricted double" | "unrestricted float" => {
                    json!({"type":"number"})
                }
                "object" => json!({"type":"object","additionalProperties":{"type":"any"}}),
                "any" => json!({"type":"any"}),
                "void" | "undefined" => json!({"type":"undefined"}),
                "ArrayBuffer" => json!({"type":"binary","isInstanceOf":"ArrayBuffer"}),
                "ArrayBufferView" => json!({"type":"binary"}),
                "Date" => json!({"type":"object","instanceOf":"Date"}),
                _ => json!({"$ref":token}),
            }
        };
        if self.eat("?") {
            value[if self.webidl { "nullable" } else { "optional" }] = true.into();
        }
        while self.eat("[") {
            self.expect("]")?;
            value = json!({"type":"array","items":value});
        }
        Ok(value)
    }

    fn default_value(&mut self) -> Result<Value, String> {
        if self.eat("[") {
            self.expect("]")?;
            return Ok(json!([]));
        }
        if self.eat("{") {
            self.expect("}")?;
            return Ok(json!({}));
        }
        Ok(scalar(&self.take()?))
    }

    fn finish(mut self) -> Result<Value, String> {
        let main_name = self
            .interfaces
            .iter()
            .find(|interface| interface.name == "Browser")
            .and_then(|interface| interface.properties.iter().next())
            .and_then(|(name, schema)| {
                schema["$ref"]
                    .as_str()
                    .map(|ty| (name.clone(), ty.to_owned()))
            });
        let namespace = self
            .namespace
            .take()
            .or_else(|| main_name.as_ref().map(|(name, _)| name.clone()))
            .ok_or("IDL has no namespace or Browser namespace attribute")?;
        let mut functions = Vec::new();
        let mut events = Vec::new();
        let mut properties = Map::new();
        let mut event_types = BTreeMap::new();
        for interface in &self.interfaces {
            if interface.parent.as_deref() == Some("ExtensionEvent") {
                let listener = interface
                    .methods
                    .iter()
                    .find(|method| method["name"] == "addListener")
                    .ok_or("ExtensionEvent has no addListener")?;
                let reference = listener["parameters"][0]["$ref"]
                    .as_str()
                    .ok_or("event listener has no callback ref")?;
                let callback = self
                    .types
                    .get(reference)
                    .ok_or_else(|| format!("missing callback {reference}"))?;
                let mut event = json!({"type":"function","parameters":callback["parameters"]});
                event
                    .as_object_mut()
                    .ok_or("invalid event")?
                    .extend(interface.attributes.clone());
                if listener["parameters"]
                    .as_array()
                    .is_some_and(|params| params.len() > 1)
                {
                    event["extraParameters"] = Value::Array(
                        listener["parameters"]
                            .as_array()
                            .ok_or("invalid listener")?[1..]
                            .to_vec(),
                    );
                }
                event_types.insert(interface.name.clone(), event);
            }
        }
        for interface in self.interfaces {
            if interface.name == "Events" {
                events.extend(interface.methods);
            } else if interface.name == "Functions"
                || main_name
                    .as_ref()
                    .is_some_and(|(_, name)| *name == interface.name)
            {
                self.attributes.extend(interface.attributes);
                functions.extend(interface.methods);
                for (name, schema) in interface.properties {
                    if let Some(event) = schema["$ref"]
                        .as_str()
                        .and_then(|name| event_types.get(name))
                    {
                        let mut event = event.clone();
                        let mut attributes = schema
                            .as_object()
                            .ok_or("invalid event attributes")?
                            .clone();
                        attributes.remove("$ref");
                        event
                            .as_object_mut()
                            .ok_or("invalid event")?
                            .extend(attributes);
                        event["name"] = name.into();
                        events.push(event);
                    } else {
                        properties.insert(name, schema);
                    }
                }
            } else if interface.name == "Properties" {
                properties.extend(interface.properties);
            } else if interface.name != "Browser"
                && interface.parent.as_deref() != Some("ExtensionEvent")
            {
                return Err(format!("unclassified IDL interface {}", interface.name));
            }
        }
        for function in &mut functions {
            let parameters = function["parameters"]
                .as_array_mut()
                .ok_or("invalid function parameters")?;
            if let Some(callback) = parameters
                .last()
                .and_then(|last| last["$ref"].as_str())
                .and_then(|name| self.types.get(name))
                .filter(|ty| ty["type"] == "function")
            {
                let argument = parameters.pop().ok_or("missing callback argument")?;
                let mut callback = callback.clone();
                callback
                    .as_object_mut()
                    .ok_or("invalid callback")?
                    .remove("id");
                callback.as_object_mut().ok_or("invalid callback")?.extend(
                    argument
                        .as_object()
                        .ok_or("invalid callback argument")?
                        .clone(),
                );
                callback
                    .as_object_mut()
                    .ok_or("invalid callback")?
                    .remove("$ref");
                callback["supportsPromises"] = function
                    .get("supportsPromises")
                    .cloned()
                    .unwrap_or(Value::Bool(true));
                if function["does_not_support_promises"] == true {
                    callback["does_not_support_promises"] = true.into();
                    callback["supportsPromises"] = false.into();
                }
                function["returns_async"] = callback;
            }
        }
        let mut namespace = json!({"namespace":namespace,"types":self.types.values().cloned().collect::<Vec<_>>(),"functions":functions,"events":events,"properties":properties});
        namespace
            .as_object_mut()
            .ok_or("invalid namespace")?
            .extend(self.attributes);
        inline_callbacks(&mut namespace, &self.types, &mut Vec::new())?;
        Ok(namespace)
    }
}

fn inline_callbacks(
    value: &mut Value,
    types: &BTreeMap<String, Value>,
    active: &mut Vec<String>,
) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(Value::as_str).map(str::to_owned) {
                if let Some(callback) = types.get(&reference).filter(|ty| ty["type"] == "function")
                {
                    if active.contains(&reference) {
                        return Err(format!("recursive IDL callback {reference}"));
                    }
                    let mut callback = callback
                        .as_object()
                        .ok_or("invalid callback schema")?
                        .clone();
                    callback.remove("id");
                    map.remove("$ref");
                    callback.extend(map.clone());
                    *map = callback;
                    active.push(reference);
                    for child in map.values_mut() {
                        inline_callbacks(child, types, active)?;
                    }
                    active.pop();
                    return Ok(());
                }
            }
            for child in map.values_mut() {
                inline_callbacks(child, types, active)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                inline_callbacks(value, types, active)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn scalar(token: &str) -> Value {
    serde_json::from_str(token).unwrap_or_else(|_| Value::String(token.to_owned()))
}

fn unquote(token: &str) -> String {
    scalar(token).as_str().unwrap_or(token).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_idl_retains_optional_union_enums_callback_and_flags() {
        let schema = parse(r#"namespace sample {
            enum Mode { FIRST, SECOND };
            dictionary Info { DOMString name; long? tabId; [nodoc] (DOMString or long) value; };
            callback Done = void(Info result);
            interface Functions { [deprecated="old"] static void get(optional Info info, optional Done callback); };
            interface Events { static void onChanged(Info info); };
            interface Properties { const long LIMIT = 25; };
        };"#, false).unwrap();
        assert_eq!(schema["namespace"], "sample");
        assert_eq!(schema["functions"][0]["returns_async"]["name"], "callback");
        assert_eq!(schema["functions"][0]["deprecated"], true);
        assert_eq!(schema["properties"]["LIMIT"]["value"], 25);
        let info = schema["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|ty| ty["id"] == "Info")
            .unwrap();
        assert_eq!(info["properties"]["tabId"]["optional"], true);
        assert_eq!(
            info["properties"]["value"]["choices"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn webidl_resolves_browser_events_promise_and_external_types() {
        let schema = parse(r#"
            [ExternalExtensionType="extensionTypes.RunAt"] typedef object RunAt;
            dictionary Info { required long tabId; sequence<DOMString> files; RunAt runAt; };
            callback OnDone = undefined(Info info);
            interface DoneEvent : ExtensionEvent { static undefined addListener(OnDone listener); static boolean hasListener(OnDone listener); };
            interface Sample { static Promise<sequence<Info>> get(optional Info info); [deprecated="old", maxListeners=1] static attribute DoneEvent onDone; };
            partial interface Browser { static attribute Sample sample; };
        "#, true).unwrap();
        assert_eq!(schema["namespace"], "sample");
        assert_eq!(
            schema["functions"][0]["returns_async"]["supportsPromises"],
            true
        );
        assert_eq!(schema["events"][0]["parameters"][0]["$ref"], "Info");
        assert_eq!(schema["events"][0]["deprecated"], true);
        assert_eq!(schema["events"][0]["maxListeners"], 1);
        let info = schema["types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|ty| ty["id"] == "Info")
            .unwrap();
        assert!(info["properties"]["tabId"].get("optional").is_none());
        assert_eq!(info["properties"]["files"]["optional"], true);
    }

    #[test]
    fn unknown_declarations_are_rejected() {
        assert!(parse("namespace broken { nonsense Bad; };", false).is_err());
    }
}
