//! Locale selection and bounded Chrome message substitution, without I/O.

use crate::api::ArgumentValidator;
use crate::dispatch::{ApiTable, RegistrationError};
use crate::{ApiError, ContextKind, ContextMask, ExtensionId, Gate, MethodSpec, Sender};
use serde_json::Value;
use std::collections::BTreeMap;

/// Maximum aggregate catalog input and expanded message size.
pub const MAX_LOCALE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum number of catalog entries loaded for one install.
pub const MAX_LOCALE_MESSAGES: usize = 16_384;

#[derive(Clone, Debug)]
struct Message {
    text: String,
    placeholders: BTreeMap<String, String>,
}

/// Browser locale facts, supplied by the adapter rather than process globals.
#[derive(Clone, Debug)]
pub struct LocaleEnvironment {
    /// BCP 47 UI language, for example `en-GB`.
    pub ui_language: String,
    /// Ordered browser accept languages; independent of the UI language.
    pub accept_languages: Vec<String>,
    /// Text direction from the browser's locale system.
    pub right_to_left: bool,
}

/// Resolved message table for one extension and browser locale.
#[derive(Clone, Debug)]
pub struct Messages {
    extension_id: ExtensionId,
    environment: LocaleEnvironment,
    messages: BTreeMap<String, Message>,
}

/// Locale search order: exact locale, base language, manifest default.
/// Values are normalized for `_locales` directory names; no paths are accepted.
pub fn fallback_chain(
    ui_language: &str,
    default_locale: Option<&str>,
) -> Result<Vec<String>, ApiError> {
    let exact = normalize_locale(ui_language)?;
    let mut result = vec![exact.clone()];
    if let Some((base, _)) = exact.split_once('_') {
        result.push(base.to_owned());
    }
    if let Some(default) = default_locale {
        let default = normalize_locale(default)?;
        if !result.contains(&default) {
            result.push(default);
        }
    }
    Ok(result)
}

fn normalize_locale(locale: &str) -> Result<String, ApiError> {
    if locale.is_empty()
        || locale.len() > 64
        || !locale
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApiError::malformed_arguments());
    }
    let mut parts = locale.split(['-', '_']);
    let first = parts.next().ok_or_else(ApiError::malformed_arguments)?;
    if first.is_empty() {
        return Err(ApiError::malformed_arguments());
    }
    let mut result = first.to_ascii_lowercase();
    for part in parts {
        if part.is_empty() {
            return Err(ApiError::malformed_arguments());
        }
        result.push('_');
        result.push_str(&part.to_ascii_uppercase());
    }
    Ok(result)
}

impl Messages {
    /// Parses adapter-supplied `messages.json` objects in fallback order. Catalog
    /// keys are locale names; unused catalogs need not be read from the package.
    pub fn new(
        extension_id: ExtensionId,
        environment: LocaleEnvironment,
        default_locale: Option<&str>,
        catalogs: &BTreeMap<String, Value>,
    ) -> Result<Self, ApiError> {
        let chain = fallback_chain(&environment.ui_language, default_locale)?;
        if environment.accept_languages.len() > 128 {
            return Err(ApiError::too_many_pending());
        }
        for language in &environment.accept_languages {
            normalize_locale(language)?;
        }
        let mut messages = BTreeMap::new();
        let mut bytes = 0_usize;
        let mut count = 0_usize;
        for locale in chain {
            let Some(catalog) = catalogs.get(&locale) else {
                continue;
            };
            bytes = bytes.saturating_add(
                serde_json::to_vec(catalog)
                    .map_err(|_| ApiError::malformed_arguments())?
                    .len(),
            );
            if bytes > MAX_LOCALE_BYTES {
                return Err(ApiError::message_too_large());
            }
            let object = catalog
                .as_object()
                .ok_or_else(ApiError::malformed_arguments)?;
            count = count.saturating_add(object.len());
            if count > MAX_LOCALE_MESSAGES {
                return Err(ApiError::too_many_pending());
            }
            let mut names = std::collections::BTreeSet::new();
            for (key, value) in object {
                if !valid_name(key)
                    || key.starts_with("@@")
                    || !names.insert(key.to_ascii_lowercase())
                {
                    return Err(ApiError::other("Invalid or duplicate locale message name."));
                }
                let message = parse_message(value)?;
                messages.entry(key.to_ascii_lowercase()).or_insert(message);
            }
        }
        Ok(Self {
            extension_id,
            environment,
            messages,
        })
    }

    /// Browser UI language, without `_locales` directory normalization.
    pub fn ui_language(&self) -> &str {
        &self.environment.ui_language
    }
    /// Ordered browser language preferences.
    pub fn accept_languages(&self) -> &[String] {
        &self.environment.accept_languages
    }
    /// Extension whose catalogs this table contains.
    pub fn extension_id(&self) -> &ExtensionId {
        &self.extension_id
    }

    /// Resolved catalog for a binding's synchronous lookup cache. Named
    /// placeholders remain in Chrome `messages.json` shape; locale fallbacks
    /// have already been selected. Include only in this install's bootstrap.
    pub fn catalog(&self) -> Value {
        Value::Object(
            self.messages
                .iter()
                .map(|(name, message)| {
                    let placeholders: serde_json::Map<String, Value> = message
                        .placeholders
                        .iter()
                        .map(|(name, content)| {
                            (name.clone(), serde_json::json!({"content": content}))
                        })
                        .collect();
                    (
                        name.clone(),
                        serde_json::json!({"message": message.text, "placeholders": placeholders}),
                    )
                })
                .collect(),
        )
    }

    /// Resolves a message; unknown names return the empty string. Substitution
    /// values are never recursively interpreted. `escape_lt` affects literals
    /// in the translation and named placeholder content, not caller values.
    pub fn get_message(
        &self,
        name: &str,
        substitutions: &[String],
        escape_lt: bool,
    ) -> Result<String, ApiError> {
        if substitutions.len() > 9 {
            return Err(ApiError::malformed_arguments());
        }
        if substitutions.iter().map(String::len).sum::<usize>() > MAX_LOCALE_BYTES {
            return Err(ApiError::message_too_large());
        }
        let name = name.to_ascii_lowercase();
        let rtl = self.environment.right_to_left;
        let predefined = match name.as_str() {
            "@@extension_id" => Some(self.extension_id.as_str().to_owned()),
            "@@ui_locale" => Some(normalize_locale(self.ui_language())?),
            "@@bidi_dir" => Some(if rtl { "rtl" } else { "ltr" }.into()),
            "@@bidi_reversed_dir" => Some(if rtl { "ltr" } else { "rtl" }.into()),
            "@@bidi_start_edge" => Some(if rtl { "right" } else { "left" }.into()),
            "@@bidi_end_edge" => Some(if rtl { "left" } else { "right" }.into()),
            _ => None,
        };
        if let Some(predefined) = predefined {
            return Ok(predefined);
        }
        match self.messages.get(&name) {
            Some(message) => expand(
                &message.text,
                Some(&message.placeholders),
                substitutions,
                escape_lt,
            ),
            None => Ok(String::new()),
        }
    }

    /// Localizes manifest/CSS tokens with this table. Unknown tokens remain
    /// verbatim; `@@extension_id` is intentionally excluded from manifests.
    pub fn localize(&self, text: &str, manifest: bool) -> Result<String, ApiError> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("__MSG_") {
            append(&mut out, &rest[..start])?;
            let after = &rest[start + 6..];
            let Some(end) = after.find("__") else {
                append(&mut out, &rest[start..])?;
                return Ok(out);
            };
            let name = &after[..end];
            let replacement = self.get_message(name, &[], false)?;
            if (manifest && name.eq_ignore_ascii_case("@@extension_id"))
                || (!self.messages.contains_key(&name.to_ascii_lowercase())
                    && replacement.is_empty())
            {
                append(&mut out, &rest[start..start + 6 + end + 2])?;
            } else {
                append(&mut out, &replacement)?;
            }
            rest = &after[end + 2..];
        }
        append(&mut out, rest)?;
        Ok(out)
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'@'))
}

fn parse_message(value: &Value) -> Result<Message, ApiError> {
    let text = value
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(ApiError::malformed_arguments)?
        .to_owned();
    let mut placeholders = BTreeMap::new();
    if let Some(values) = value.get("placeholders") {
        for (name, value) in values
            .as_object()
            .ok_or_else(ApiError::malformed_arguments)?
        {
            if !valid_name(name) {
                return Err(ApiError::malformed_arguments());
            }
            let content = value
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(ApiError::malformed_arguments)?;
            if placeholders
                .insert(name.to_ascii_lowercase(), content.to_owned())
                .is_some()
            {
                return Err(ApiError::malformed_arguments());
            }
        }
    }
    // Resolve once to catch missing named placeholders before installation.
    expand(&text, Some(&placeholders), &[], false)?;
    Ok(Message { text, placeholders })
}

fn append(out: &mut String, text: &str) -> Result<(), ApiError> {
    if out.len().saturating_add(text.len()) > MAX_LOCALE_BYTES {
        return Err(ApiError::message_too_large());
    }
    out.push_str(text);
    Ok(())
}

fn literal(out: &mut String, text: &str, escape_lt: bool) -> Result<(), ApiError> {
    if !escape_lt {
        return append(out, text);
    }
    for part in text.split_inclusive('<') {
        if let Some(prefix) = part.strip_suffix('<') {
            append(out, prefix)?;
            append(out, "&lt;")?;
        } else {
            append(out, part)?;
        }
    }
    Ok(())
}

fn expand(
    text: &str,
    placeholders: Option<&BTreeMap<String, String>>,
    substitutions: &[String],
    escape_lt: bool,
) -> Result<String, ApiError> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('$') {
        literal(&mut out, &rest[..start], escape_lt)?;
        let after = &rest[start + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            append(&mut out, "$")?;
            rest = tail;
        } else if after.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            let position = after[..digits]
                .parse::<usize>()
                .ok()
                .and_then(|position| position.checked_sub(1));
            if let Some(value) = position.and_then(|position| substitutions.get(position)) {
                append(&mut out, value)?;
            }
            rest = &after[digits..];
        } else if let Some((end, map)) = after
            .find('$')
            .zip(placeholders)
            .filter(|(end, _)| valid_name(&after[..*end]))
        {
            let content = map
                .get(&after[..end].to_ascii_lowercase())
                .ok_or_else(|| ApiError::other("Undefined locale placeholder."))?;
            append(&mut out, &expand(content, None, substitutions, escape_lt)?)?;
            rest = &after[end + 1..];
        } else {
            append(&mut out, "$")?;
            rest = after;
        }
    }
    literal(&mut out, rest, escape_lt)?;
    Ok(out)
}

/// Adapter access to the caller's already resolved catalog.
pub trait I18nHost {
    /// Returns the message table for the authenticated profile and install.
    fn messages(&self, sender: &Sender) -> Result<&Messages, ApiError>;
}

/// Handwritten validation behind the shared schema boundary.
pub struct I18nValidator;
impl ArgumentValidator for I18nValidator {
    fn validate(&self, method: &str, args: &[Value]) -> Result<(), ApiError> {
        if method != "i18n.getMessage" {
            return if args.is_empty() {
                Ok(())
            } else {
                Err(ApiError::malformed_arguments())
            };
        }
        if args.is_empty() || args.len() > 3 || !args[0].is_string() {
            return Err(ApiError::malformed_arguments());
        }
        substitutions(args.get(1))?;
        if let Some(options) = args.get(2) {
            let options = options
                .as_object()
                .ok_or_else(ApiError::malformed_arguments)?;
            if options
                .iter()
                .any(|(key, value)| key != "escapeLt" || !value.is_boolean())
            {
                return Err(ApiError::malformed_arguments());
            }
        }
        Ok(())
    }
}

fn substitutions(value: Option<&Value>) -> Result<Vec<String>, ApiError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(value)) => Ok(vec![value.clone()]),
        Some(Value::Array(values)) if values.len() <= 9 => {
            values.iter().map(|value| json_string(value, 0)).collect()
        }
        Some(Value::Array(_)) => Err(ApiError::malformed_arguments()),
        _ => Ok(Vec::new()),
    }
}

fn json_string(value: &Value, depth: usize) -> Result<String, ApiError> {
    if depth > crate::limits::MAX_ARGUMENT_DEPTH {
        return Err(ApiError::malformed_arguments());
    }
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Object(_) => Ok("[object Object]".into()),
        Value::Array(values) => {
            let mut out = String::new();
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    append(&mut out, ",")?;
                }
                if !value.is_null() {
                    append(&mut out, &json_string(value, depth + 1)?)?;
                }
            }
            Ok(out)
        }
        _ => Ok(value.to_string()),
    }
}

/// Registers locale lookup and browser language methods. Synchronous JS methods
/// should use this same resolved table in the binding's bootstrap cache.
pub fn register<S: I18nHost + 'static>(table: &mut ApiTable<S>) -> Result<(), RegistrationError> {
    for name in [
        "i18n.getMessage",
        "i18n.getUILanguage",
        "i18n.getAcceptLanguages",
    ] {
        table.register(
            name,
            MethodSpec::new(
                Gate::Open,
                ContextMask::ALL.without(ContextKind::ExternalPage),
                |host: &mut S, invocation| {
                    let result = (|| {
                        I18nValidator.validate(invocation.name(), invocation.args())?;
                        let messages = host.messages(invocation.sender())?;
                        if messages.extension_id() != invocation.sender().extension_id() {
                            return Err(ApiError::context_invalidated());
                        }
                        match invocation.name() {
                            "i18n.getMessage" => {
                                let args = invocation.args();
                                let name =
                                    args[0].as_str().ok_or_else(ApiError::malformed_arguments)?;
                                let escape = args
                                    .get(2)
                                    .and_then(|options| options.get("escapeLt"))
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false);
                                messages
                                    .get_message(name, &substitutions(args.get(1))?, escape)
                                    .map(Value::String)
                            }
                            "i18n.getUILanguage" => {
                                Ok(Value::String(messages.ui_language().to_owned()))
                            }
                            _ => Ok(Value::Array(
                                messages
                                    .accept_languages()
                                    .iter()
                                    .cloned()
                                    .map(Value::String)
                                    .collect(),
                            )),
                        }
                    })();
                    invocation.reply(result)
                },
            ),
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "i18n/tests.rs"]
mod dispatcher_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn messages() -> Messages {
        Messages::new(ExtensionId::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(), LocaleEnvironment {
            ui_language:"en-GB".into(), accept_languages:vec!["pl".into(),"en".into()], right_to_left:false,
        }, Some("fr"), &BTreeMap::from([
            ("en_GB".into(),json!({"color":{"message":"colour"},"hello":{"message":"<b>$WHO$</b> $$ $2", "placeholders":{"who":{"content":"$1"}}}})),
            ("en".into(),json!({"base":{"message":"base"},"color":{"message":"color"}})),
            ("fr".into(),json!({"last":{"message":"fin"}})),
        ])).unwrap()
    }
    #[test]
    fn fallback_predefined_and_preferences_are_distinct() {
        let messages = messages();
        for (key, expected) in [
            ("COLOR", "colour"),
            ("base", "base"),
            ("last", "fin"),
            ("missing", ""),
            ("@@ui_locale", "en_GB"),
            ("@@bidi_start_edge", "left"),
        ] {
            assert_eq!(messages.get_message(key, &[], false).unwrap(), expected);
        }
        assert_eq!(messages.accept_languages(), ["pl", "en"]);
        assert_eq!(messages.ui_language(), "en-GB");
        assert_eq!(
            fallback_chain("en-GB", Some("en")).unwrap(),
            ["en_GB", "en"]
        );
        assert!(fallback_chain("../en", None).is_err());
    }
    #[test]
    fn substitutions_are_not_recursive_and_escape_only_literals() {
        let messages = messages();
        assert_eq!(
            messages
                .get_message("hello", &["<i>$2</i>".into(), "ok".into()], true)
                .unwrap(),
            "&lt;b><i>$2</i>&lt;/b> $ ok"
        );
        assert_eq!(
            messages
                .localize("__MSG_color__ __MSG_missing__", true)
                .unwrap(),
            "colour __MSG_missing__"
        );
        assert_eq!(
            messages.localize("__MSG_@@extension_id__", true).unwrap(),
            "__MSG_@@extension_id__"
        );
        assert_eq!(
            messages.localize("__MSG_@@extension_id__", false).unwrap(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert!(messages
            .get_message("hello", &vec![String::new(); 10], false)
            .is_err());
        assert_eq!(
            expand(
                "$x$",
                Some(&BTreeMap::from([("x".into(), "<b>$1</b>".into())])),
                &["<i>".into()],
                true
            )
            .unwrap(),
            "&lt;b><i>&lt;/b>"
        );
    }
    #[test]
    fn corrupt_catalogs_and_expansion_bombs_are_refused() {
        assert!(parse_message(&json!({"message":"$missing$"})).is_err());
        assert!(parse_message(&json!({"message":3})).is_err());
        let huge = "a".repeat(MAX_LOCALE_BYTES);
        assert!(expand("$1$1", None, &[huge], false).is_err());
        assert_eq!(
            substitutions(Some(&json!([3, null, true, {}, [1, null, 2]]))).unwrap(),
            ["3", "null", "true", "[object Object]", "1,,2"]
        );
        assert!(substitutions(Some(&json!(42))).unwrap().is_empty());
    }
}
