//! Reading `manifest.json` and its localized strings.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;
use thiserror::Error;

const PREFERRED_LOCALES: [&str; 3] = ["en", "en_US", "en_GB"];

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("cannot read manifest: {0}")]
    Io(#[from] io::Error),
    #[error("invalid manifest JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("manifest is not a JSON object")]
    NotObject,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum World {
    #[default]
    Isolated,
    Main,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunAt {
    DocumentStart,
    DocumentEnd,
    #[default]
    DocumentIdle,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentScript {
    pub matches: Vec<String>,
    pub exclude_matches: Vec<String>,
    pub js: Vec<String>,
    pub css: Vec<String>,
    pub world: World,
    pub all_frames: bool,
    pub run_at: RunAt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Background {
    ServiceWorker { path: String, module: bool },
    Scripts(Vec<String>),
    Page(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionInfo {
    pub default_popup: Option<String>,
    pub default_title: Option<String>,
    pub default_icon: IconSet,
}

/// Icons keyed by pixel size, ascending. A bare string icon has size 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IconSet(Vec<(u32, String)>);

impl IconSet {
    fn from_value(value: Option<&Value>) -> Self {
        let mut icons: Vec<_> = match value {
            Some(Value::String(path)) => normalize_resource(path)
                .map(|path| (0, path))
                .into_iter()
                .collect(),
            Some(Value::Object(map)) => map
                .iter()
                .filter_map(|(size, path)| {
                    Some((size.parse().ok()?, normalize_resource(path.as_str()?)?))
                })
                .collect(),
            _ => Vec::new(),
        };
        icons.sort_by_key(|(size, _)| *size);
        Self(icons)
    }

    /// The smallest icon at least `size` pixels, else the largest available.
    pub fn best(&self, size: u32) -> Option<&str> {
        self.0
            .iter()
            .find(|(icon_size, _)| *icon_size >= size)
            .or(self.0.last())
            .map(|(_, path)| path.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.0.iter().map(|(size, path)| (*size, path.as_str()))
    }
}

#[derive(Debug, Clone, Default)]
struct Message {
    text: String,
    placeholders: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    raw: Value,
    messages: HashMap<String, Message>,
}

impl Manifest {
    pub fn load(dir: &Path) -> Result<Self, ManifestError> {
        let raw = parse_lenient(&fs::read_to_string(dir.join("manifest.json"))?)?;
        if !raw.is_object() {
            return Err(ManifestError::NotObject);
        }
        let mut manifest = Self::from_value(raw);
        manifest.messages = load_messages(dir, manifest.default_locale());
        Ok(manifest)
    }

    /// A manifest without localized messages, for manifests held in memory.
    pub fn from_value(raw: Value) -> Self {
        Self {
            raw,
            messages: HashMap::new(),
        }
    }

    pub fn raw(&self) -> &Value {
        &self.raw
    }

    pub fn manifest_version(&self) -> u8 {
        self.raw["manifest_version"]
            .as_u64()
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(0)
    }

    /// Chrome's version grammar: one to four dot-separated integers up to
    /// 65535. Anything else is refused, since the version also names the
    /// package's folder on disk.
    pub fn version(&self) -> Option<&str> {
        let version = self.raw["version"].as_str()?;
        let parts: Vec<&str> = version.split('.').collect();
        (!parts.is_empty()
            && parts.len() <= 4
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part.len() <= 5
                    && part.bytes().all(|byte| byte.is_ascii_digit())
                    && (part.len() == 1 || !part.starts_with('0'))
                    && part.parse::<u32>().is_ok_and(|value| value <= 65_535)
            }))
        .then_some(version)
    }

    pub fn name(&self) -> Option<String> {
        self.localized("name")
    }

    pub fn short_name(&self) -> Option<String> {
        self.localized("short_name")
    }

    pub fn description(&self) -> Option<String> {
        self.localized("description")
    }

    pub fn default_locale(&self) -> Option<&str> {
        self.raw["default_locale"].as_str()
    }

    pub fn minimum_chrome_version(&self) -> Option<&str> {
        self.raw["minimum_chrome_version"].as_str()
    }

    pub fn web_accessible_resources(&self) -> Option<&Value> {
        self.raw.get("web_accessible_resources")
    }

    pub fn externally_connectable(&self) -> Option<&Value> {
        self.raw.get("externally_connectable")
    }

    pub fn permissions(&self) -> Vec<String> {
        api_permissions(&self.raw["permissions"])
    }

    pub fn optional_permissions(&self) -> Vec<String> {
        api_permissions(&self.raw["optional_permissions"])
    }

    pub fn host_permissions(&self) -> Vec<String> {
        self.hosts("permissions", "host_permissions")
    }

    pub fn optional_host_permissions(&self) -> Vec<String> {
        self.hosts("optional_permissions", "optional_host_permissions")
    }

    pub fn content_scripts(&self) -> Vec<ContentScript> {
        let Some(scripts) = self.raw["content_scripts"].as_array() else {
            return Vec::new();
        };
        scripts
            .iter()
            .filter(|script| script.is_object())
            .map(|script| ContentScript {
                matches: strings(&script["matches"]).map(str::to_owned).collect(),
                exclude_matches: strings(&script["exclude_matches"])
                    .map(str::to_owned)
                    .collect(),
                js: resources(&script["js"]),
                css: resources(&script["css"]),
                world: match script["world"].as_str() {
                    Some("MAIN") => World::Main,
                    _ => World::Isolated,
                },
                all_frames: script["all_frames"].as_bool().unwrap_or(false),
                run_at: match script["run_at"].as_str() {
                    Some("document_start") => RunAt::DocumentStart,
                    Some("document_end") => RunAt::DocumentEnd,
                    _ => RunAt::DocumentIdle,
                },
            })
            .collect()
    }

    /// The declared background context. A service worker wins over
    /// `scripts`, which packages may also declare for Firefox.
    pub fn background(&self) -> Option<Background> {
        let background = &self.raw["background"];
        if let Some(path) = background["service_worker"].as_str() {
            return Some(Background::ServiceWorker {
                path: normalize_resource(path)?,
                module: background["type"].as_str() == Some("module"),
            });
        }
        let scripts = resources(&background["scripts"]);
        if !scripts.is_empty() {
            return Some(Background::Scripts(scripts));
        }
        let page = background["page"].as_str()?;
        normalize_resource(page).map(Background::Page)
    }

    pub fn action(&self) -> Option<ActionInfo> {
        let action = ["action", "browser_action", "page_action"]
            .iter()
            .find_map(|key| self.raw.get(key).filter(|value| value.is_object()))?;
        Some(ActionInfo {
            default_popup: action["default_popup"]
                .as_str()
                .and_then(normalize_resource),
            default_title: action["default_title"]
                .as_str()
                .map(|title| self.localize(title)),
            default_icon: IconSet::from_value(action.get("default_icon")),
        })
    }

    pub fn options_page(&self) -> Option<String> {
        self.raw["options_ui"]["page"]
            .as_str()
            .or_else(|| self.raw["options_page"].as_str())
            .and_then(normalize_resource)
    }

    pub fn icons(&self) -> IconSet {
        IconSet::from_value(self.raw.get("icons"))
    }

    pub fn sandbox_pages(&self) -> Vec<String> {
        resources(&self.raw["sandbox"]["pages"])
    }

    /// Replaces every `__MSG_key__` token with its localized message. Unknown
    /// keys are left verbatim so a broken package stays recognizable.
    pub fn localize(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("__MSG_") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 6..];
            let token = after.find("__").map(|end| &after[..end]).filter(|key| {
                !key.is_empty()
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'@')
            });
            match token.and_then(|key| self.messages.get(&key.to_ascii_lowercase())) {
                Some(message) => {
                    out.push_str(&expand(&message.text, &message.placeholders));
                    rest = &after[token.map_or(0, str::len) + 2..];
                }
                None => {
                    out.push_str("__MSG_");
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn localized(&self, key: &str) -> Option<String> {
        self.raw[key].as_str().map(|text| self.localize(text))
    }

    fn hosts(&self, mixed_key: &str, hosts_key: &str) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        let legacy = (self.manifest_version() < 3)
            .then(|| strings(&self.raw[mixed_key]).filter(|p| is_match_pattern(p)));
        for pattern in strings(&self.raw[hosts_key]).chain(legacy.into_iter().flatten()) {
            if !hosts.iter().any(|host| host == pattern) {
                hosts.push(pattern.to_owned());
            }
        }
        hosts
    }
}

pub(crate) fn is_match_pattern(permission: &str) -> bool {
    permission == "<all_urls>" || permission.contains("://")
}

fn api_permissions(value: &Value) -> Vec<String> {
    strings(value)
        .filter(|p| !is_match_pattern(p))
        .map(str::to_owned)
        .collect()
}

fn strings(value: &Value) -> impl Iterator<Item = &str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

fn resources(value: &Value) -> Vec<String> {
    strings(value).filter_map(normalize_resource).collect()
}

/// Normalizes a package-relative resource path, rejecting anything that
/// could resolve outside the package. A query or fragment is kept as is.
pub fn normalize_resource(path: &str) -> Option<String> {
    let mut rest = path;
    while let Some(stripped) = rest.strip_prefix('/').or_else(|| rest.strip_prefix("./")) {
        rest = stripped;
    }
    let split = rest.find(['?', '#']).unwrap_or(rest.len());
    let (file, suffix) = rest.split_at(split);
    if file.contains('\\') || file.chars().any(char::is_control) {
        return None;
    }
    let mut parts = Vec::new();
    for part in file.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part if !portable_name(part) => return None,
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/") + suffix)
}

/// A path part that names one file inside the package on every platform: no
/// drive or stream separator, no Windows device name, and no trailing dot or
/// space that Windows would strip into another name.
pub(crate) fn portable_name(part: &str) -> bool {
    if part.contains(':') || part.ends_with('.') || part.ends_with(' ') {
        return false;
    }
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    let device = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit());
    !device
}

fn load_messages(dir: &Path, default_locale: Option<&str>) -> HashMap<String, Message> {
    let mut messages = HashMap::new();
    let mut tried = Vec::new();
    for locale in PREFERRED_LOCALES.into_iter().chain(default_locale) {
        if tried.contains(&locale) || normalize_resource(locale).as_deref() != Some(locale) {
            continue;
        }
        tried.push(locale);
        let path = dir.join("_locales").join(locale).join("messages.json");
        let Some(Value::Object(entries)) = fs::read_to_string(path)
            .ok()
            .and_then(|text| parse_lenient(&text).ok())
        else {
            continue;
        };
        for (key, entry) in entries {
            let Some(text) = entry["message"].as_str() else {
                continue;
            };
            let placeholders = entry["placeholders"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(name, placeholder)| {
                    Some((
                        name.to_ascii_lowercase(),
                        placeholder["content"].as_str()?.to_owned(),
                    ))
                })
                .collect();
            messages
                .entry(key.to_ascii_lowercase())
                .or_insert_with(|| Message {
                    text: text.to_owned(),
                    placeholders,
                });
        }
    }
    messages
}

/// Expands `$name$` placeholders and `$$`. Positional `$1`…`$9` arguments are
/// runtime values, which a static manifest string never has.
fn expand(text: &str, placeholders: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(dollar) = rest.find('$') {
        out.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            out.push('$');
            rest = tail;
            continue;
        }
        let len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..len];
        if !name.is_empty() && after[len..].starts_with('$') {
            if let Some(content) = placeholders.get(&name.to_ascii_lowercase()) {
                out.push_str(&expand(content, &HashMap::new()));
                rest = &after[len + 1..];
                continue;
            }
        }
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
            rest = &after[len..];
            continue;
        }
        out.push('$');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Parses JSON as Chrome reads manifests: a leading BOM, comments, and
/// trailing commas are tolerated.
pub fn parse_lenient(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(&to_strict_json(text))
}

/// Rewrites Chrome's lenient JSON dialect as strict JSON, preserving
/// everything else byte for byte and keeping line breaks inside comments.
pub fn to_strict_json(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    remove_trailing_commas(&strip_comments(text))
}

fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut copied = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => i = string_end(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                out.push_str(&text[copied..i]);
                i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
                copied = i;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.push_str(&text[copied..i]);
                let end = text[i + 2..].find("*/").map_or(bytes.len(), |n| i + n + 4);
                out.extend(text[i..end].chars().filter(|&c| c == '\n'));
                i = end;
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn remove_trailing_commas(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut copied = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => i = string_end(bytes, i),
            b',' => {
                let next = bytes[i + 1..].iter().find(|b| !b.is_ascii_whitespace());
                if matches!(next, Some(b'}' | b']')) {
                    out.push_str(&text[copied..i]);
                    copied = i + 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// The index just past the string literal opening at `start`.
pub(crate) fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_chrome_versions_name_a_package() {
        let version = |value: &str| {
            super::Manifest::from_value(serde_json::json!({ "version": value }))
                .version()
                .map(str::to_owned)
        };
        for good in ["1", "1.0", "1.2.3.4", "65535.0.0.1", "0.1"] {
            assert_eq!(version(good).as_deref(), Some(good), "{good}");
        }
        for bad in [
            "",
            "../../..",
            "1.2.3.4.5",
            "01.0",
            "1..2",
            "65536",
            "1.a",
            "1 ",
        ] {
            assert_eq!(version(bad), None, "{bad}");
        }
    }

    #[test]
    fn resources_cannot_name_drives_or_devices() {
        assert_eq!(
            super::normalize_resource("/js/app.js").as_deref(),
            Some("js/app.js")
        );
        for path in ["C:/x.js", "js/CON.js", "js/a.js:zone", "js/a."] {
            assert_eq!(super::normalize_resource(path), None, "{path}");
        }
    }

    use serde_json::json;

    use super::*;

    fn write(dir: &Path, path: &str, contents: &str) {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn parses_lenient_json() {
        let text = "\u{feff}{\n  // comment\n  \"a\": \"http://x/*\", /* block\n */ \"b\": [1, 2,],\n  \"c\": \"a,]\",\n}";
        let value = parse_lenient(text).unwrap();
        assert_eq!(value, json!({"a": "http://x/*", "b": [1, 2], "c": "a,]"}));
        assert_eq!(to_strict_json(text).lines().count(), text.lines().count());
        assert_eq!(parse_lenient(r#"{"a": "\"//"}"#).unwrap()["a"], "\"//");
    }

    #[test]
    fn localizes_from_preferred_locales_with_fallback() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "manifest.json",
            r#"{"manifest_version": 3, "name": "__MSG_extName__", "default_locale": "de",
                "description": "__MSG_desc__ by __MSG_Author__", "short_name": "__MSG_missing__"}"#,
        );
        write(
            temp.path(),
            "_locales/en/messages.json",
            r#"{"extname": {"message": "Vault $WHO$ $$5 $1"},
                "EXTNAME_UNUSED": {"message": "x"}}"#,
        );
        write(
            temp.path(),
            "_locales/de/messages.json",
            r#"{"extName": {"message": "Tresor"}, "desc": {"message": "Passwörter"},
                "author": {"message": "$org$", "placeholders": {"ORG": {"content": "ACME"}}}}"#,
        );
        let manifest = Manifest::load(temp.path()).unwrap();
        assert_eq!(manifest.name().as_deref(), Some("Vault $WHO$ $5 "));
        assert_eq!(
            manifest.description().as_deref(),
            Some("Passwörter by ACME")
        );
        assert_eq!(manifest.short_name().as_deref(), Some("__MSG_missing__"));
    }

    #[test]
    fn splits_permissions_by_manifest_version() {
        let mv2 = Manifest::from_value(json!({
            "manifest_version": 2,
            "permissions": ["tabs", "<all_urls>", "https://*.example.com/*", {"x": 1}],
            "optional_permissions": ["bookmarks", "*://a.test/*"]
        }));
        assert_eq!(mv2.permissions(), ["tabs"]);
        assert_eq!(
            mv2.host_permissions(),
            ["<all_urls>", "https://*.example.com/*"]
        );
        assert_eq!(mv2.optional_permissions(), ["bookmarks"]);
        assert_eq!(mv2.optional_host_permissions(), ["*://a.test/*"]);

        let mv3 = Manifest::from_value(json!({
            "manifest_version": 3,
            "permissions": ["storage", "https://ignored.test/*"],
            "host_permissions": ["https://a.test/*", "https://a.test/*"]
        }));
        assert_eq!(mv3.permissions(), ["storage"]);
        assert_eq!(mv3.host_permissions(), ["https://a.test/*"]);
    }

    #[test]
    fn reads_background_variants() {
        let both = Manifest::from_value(json!({"background": {
            "service_worker": "/bg.js", "type": "module", "scripts": ["bg.js"]}}));
        assert_eq!(
            both.background(),
            Some(Background::ServiceWorker {
                path: "bg.js".into(),
                module: true
            })
        );
        let scripts =
            Manifest::from_value(json!({"background": {"scripts": ["./a.js", "../x.js"]}}));
        assert_eq!(
            scripts.background(),
            Some(Background::Scripts(vec!["a.js".into()]))
        );
        let page = Manifest::from_value(json!({"background": {"page": "bg.html"}}));
        assert_eq!(page.background(), Some(Background::Page("bg.html".into())));
        assert_eq!(Manifest::from_value(json!({})).background(), None);
    }

    #[test]
    fn reads_content_scripts_actions_and_icons() {
        let manifest = Manifest::from_value(json!({
            "content_scripts": [
                {"matches": ["<all_urls>"], "js": ["/cs.js"], "world": "MAIN",
                 "run_at": "document_start", "all_frames": true},
                {"matches": ["https://a.test/*"], "css": ["a.css"]}
            ],
            "browser_action": {"default_popup": "popup/index.html?mode=popup",
                               "default_icon": "icon.png"},
            "icons": {"16": "i16.png", "128": "i128.png", "48": "./i48.png"},
            "options_ui": {"page": "options.html"},
            "options_page": "legacy.html",
            "sandbox": {"pages": ["sandbox.html"]}
        }));
        let scripts = manifest.content_scripts();
        assert_eq!(scripts[0].js, ["cs.js"]);
        assert_eq!(scripts[0].world, World::Main);
        assert_eq!(scripts[0].run_at, RunAt::DocumentStart);
        assert!(scripts[0].all_frames);
        assert_eq!(scripts[1].world, World::Isolated);
        assert_eq!(scripts[1].run_at, RunAt::DocumentIdle);

        let action = manifest.action().unwrap();
        assert_eq!(
            action.default_popup.as_deref(),
            Some("popup/index.html?mode=popup")
        );
        assert_eq!(action.default_icon.best(32), Some("icon.png"));

        let icons = manifest.icons();
        assert_eq!(icons.best(16), Some("i16.png"));
        assert_eq!(icons.best(32), Some("i48.png"));
        assert_eq!(icons.best(256), Some("i128.png"));
        assert_eq!(manifest.options_page().as_deref(), Some("options.html"));
        assert_eq!(manifest.sandbox_pages(), ["sandbox.html"]);
    }

    #[test]
    fn normalizes_resource_paths() {
        assert_eq!(normalize_resource("/./a/./b.js").as_deref(), Some("a/b.js"));
        assert_eq!(
            normalize_resource("a.html#x/../y").as_deref(),
            Some("a.html#x/../y")
        );
        assert_eq!(normalize_resource("a/../../b"), None);
        assert_eq!(normalize_resource("a\\b"), None);
        assert_eq!(normalize_resource("/"), None);
    }
}
