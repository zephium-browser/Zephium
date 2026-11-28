//! Injects the compat layer into a freshly extracted package, in place.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use serde_json::{json, Value};
use thiserror::Error;

use crate::manifest::{self, Background, Manifest, ManifestError};
use crate::store::tag_end;

pub const COMPAT_DIR: &str = "__zephium__";

const NAMESPACES: &[&str] = &[
    "runtime",
    "tabs",
    "windows",
    "storage",
    "alarms",
    "action",
    "browserAction",
    "pageAction",
    "commands",
    "contextMenus",
    "menus",
    "webNavigation",
    "webRequest",
    "cookies",
    "notifications",
    "permissions",
    "idle",
    "management",
    "downloads",
    "bookmarks",
    "history",
    "sessions",
    "omnibox",
    "identity",
    "tabGroups",
    "sidePanel",
    "scripting",
    "i18n",
    "offscreen",
    "declarativeNetRequest",
    "privacy",
    "proxy",
    "fontSettings",
    "tts",
    "extension",
    "gcm",
    "instanceID",
    "topSites",
    "search",
    "readingList",
    "system.display",
    "userScripts",
];

/// `storage.<area>.onChanged` is recorded as `storage.onChanged`.
const STORAGE_AREAS: &[&str] = &["local", "sync", "session", "managed"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatLayer {
    /// Shared JavaScript body, appended after the generated configuration.
    pub script: String,
    pub file_name: String,
    /// API permissions the layer itself needs, added when the manifest lacks
    /// them. They are reported so consent never attributes them to the
    /// extension.
    pub permissions: Vec<String>,
    /// Report extensions' own errors and warnings to the browser.
    pub diagnostics: bool,
}

impl CompatLayer {
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            file_name: "compat.js".to_owned(),
            permissions: Vec::new(),
            diagnostics: false,
        }
    }

    pub fn with_diagnostics(mut self, diagnostics: bool) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    pub fn with_permissions(mut self, permissions: &[&str]) -> Self {
        self.permissions = permissions.iter().map(|p| (*p).to_owned()).collect();
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrepareReport {
    /// Sorted `namespace.onEvent` names the package's scripts reference.
    pub events: Vec<String>,
    pub html_injected: usize,
    /// HTML files left untouched because they are not UTF-8.
    pub html_skipped: Vec<String>,
    /// The service worker that now loads the compat layer.
    pub worker: Option<String>,
    pub manifest_rewritten: bool,
    /// Permissions the compat layer added to the manifest.
    pub added_permissions: Vec<String>,
}

#[derive(Debug, Error)]
pub enum PrepareError {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("package has already been prepared")]
    AlreadyPrepared,
    #[error("invalid compat file name {0:?}")]
    InvalidFileName(String),
    #[error("background service worker {0:?} is missing")]
    MissingWorker(String),
    #[error("manifest could not be edited")]
    ManifestLayout,
}

/// Prepares a pristine extraction at `dir`. Every edit only inserts text, so
/// no declaration the extension made is removed or altered.
pub fn prepare(dir: &Path, compat: &CompatLayer) -> Result<PrepareReport, PrepareError> {
    let name = &compat.file_name;
    if manifest::normalize_resource(name).as_deref() != Some(name.as_str())
        || name.contains(['/', '?', '#', '"', '<', '>'])
    {
        return Err(PrepareError::InvalidFileName(name.clone()));
    }
    let compat_dir = dir.join(COMPAT_DIR);
    if compat_dir.exists() {
        return Err(PrepareError::AlreadyPrepared);
    }
    let manifest = Manifest::load(dir)?;
    let resource = format!("{COMPAT_DIR}/{name}");
    let url = format!("/{resource}");

    let manifest_path = dir.join("manifest.json");
    let original = fs::read_to_string(&manifest_path)?;
    let declared = manifest.permissions();
    let added_permissions: Vec<String> = compat
        .permissions
        .iter()
        .filter(|permission| !declared.contains(permission))
        .cloned()
        .collect();
    let rewritten = edit_manifest(&original, manifest.raw(), &resource, &added_permissions)?;

    let worker = match manifest.background() {
        Some(Background::ServiceWorker { path, module }) => {
            let file = path.split(['?', '#']).next().unwrap_or(&path).to_owned();
            let source = fs::read(dir.join(&file)).map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => PrepareError::MissingWorker(path.clone()),
                _ => error.into(),
            })?;
            Some((file, source, module))
        }
        _ => None,
    };

    let mut files = Vec::new();
    list_files(dir, "", &mut files)?;
    let mut events = BTreeSet::new();
    for file in files.iter().filter(|f| has_extension(f, &["js", "mjs"])) {
        scan_events(&fs::read(dir.join(file))?, &mut events);
    }
    let events: Vec<String> = events.into_iter().collect();

    let mut config = json!({ "events": events, "manifestVersion": manifest.manifest_version() });
    if compat.diagnostics {
        config["diagnostics"] = json!(true);
    }
    fs::create_dir(&compat_dir)?;
    fs::write(
        compat_dir.join(name),
        format!("var __zephiumConfig = {config};\n{}", compat.script),
    )?;

    let mut report = PrepareReport {
        events,
        added_permissions,
        ..PrepareReport::default()
    };

    if let Some((file, source, module)) = worker {
        fs::write(dir.join(&file), inject_worker(&source, module, &url))?;
        report.worker = Some(file);
    }

    inject_extension_pages(dir, &manifest, &files, &url, &mut report)?;

    if rewritten != original {
        fs::write(&manifest_path, rewritten)?;
        report.manifest_rewritten = true;
    }
    Ok(report)
}

pub(crate) fn inject_extension_pages(
    dir: &Path,
    manifest: &Manifest,
    files: &[String],
    url: &str,
    report: &mut PrepareReport,
) -> io::Result<()> {
    let sandboxed: HashSet<String> = manifest
        .sandbox_pages()
        .iter()
        .map(|page| page.split(['?', '#']).next().unwrap_or(page).to_lowercase())
        .collect();
    let script_tag = format!("<script src=\"{url}\"></script>");
    for file in files.iter().filter(|f| has_extension(f, &["html", "htm"])) {
        if sandboxed.contains(&file.to_lowercase()) {
            continue;
        }
        match String::from_utf8(fs::read(dir.join(file))?) {
            Ok(html) => {
                fs::write(dir.join(file), inject_html(&html, &script_tag))?;
                report.html_injected += 1;
            }
            Err(_) => report.html_skipped.push(file.clone()),
        }
    }

    Ok(())
}

pub(crate) fn list_files(root: &Path, prefix: &str, out: &mut Vec<String>) -> io::Result<()> {
    let mut entries = fs::read_dir(root.join(prefix))?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            list_files(root, &path, out)?;
        } else if kind.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn has_extension(path: &str, extensions: &[&str]) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| extensions.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn ident_end(src: &[u8], start: usize) -> usize {
    src[start..]
        .iter()
        .position(|&b| !is_ident(b))
        .map_or(src.len(), |n| start + n)
}

/// Collects `<namespace>.<onEvent>` member accesses. Matches inside strings
/// and comments are included; a spurious event only costs an unused hook.
fn scan_events(src: &[u8], events: &mut BTreeSet<String>) {
    let mut i = 0;
    while i < src.len() {
        if !is_ident(src[i]) || (i > 0 && is_ident(src[i - 1])) {
            i += 1;
            continue;
        }
        let end = ident_end(src, i);
        let word = &src[i..end];
        for namespace in NAMESPACES {
            let (first, rest) = namespace.split_once('.').unwrap_or((namespace, ""));
            if first.as_bytes() != word {
                continue;
            }
            if let Some(event) = match_event(src, end, rest, namespace == &"storage") {
                events.insert(format!("{namespace}.{event}"));
            }
        }
        i = end;
    }
}

/// Matches the remaining namespace `segments` and an `onEvent` member after
/// the namespace's first identifier, which ends at `pos`.
fn match_event<'a>(
    src: &'a [u8],
    mut pos: usize,
    segments: &str,
    storage: bool,
) -> Option<&'a str> {
    for segment in segments.split('.').filter(|s| !s.is_empty()) {
        let (start, end) = next_member(src, pos)?;
        if &src[start..end] != segment.as_bytes() {
            return None;
        }
        pos = end;
    }
    let (mut start, mut end) = next_member(src, pos)?;
    if storage
        && STORAGE_AREAS
            .iter()
            .any(|a| a.as_bytes() == &src[start..end])
    {
        (start, end) = next_member(src, end)?;
    }
    let member = &src[start..end];
    let is_event = member.len() > 2
        && member.starts_with(b"on")
        && member[2].is_ascii_uppercase()
        && member.iter().all(u8::is_ascii_alphabetic);
    is_event.then(|| std::str::from_utf8(member).ok())?
}

fn next_member(src: &[u8], pos: usize) -> Option<(usize, usize)> {
    let start = member_dot(src, pos)?;
    let end = ident_end(src, start);
    (end > start).then_some((start, end))
}

/// Accepts `.` or `?.`, optionally surrounded by whitespace for chained calls.
fn member_dot(src: &[u8], pos: usize) -> Option<usize> {
    let skip_ws = |mut p: usize| {
        while src.get(p).is_some_and(u8::is_ascii_whitespace) {
            p += 1;
        }
        p
    };
    let mut p = skip_ws(pos);
    if src.get(p) == Some(&b'?') {
        p += 1;
    }
    (src.get(p) == Some(&b'.')).then(|| skip_ws(p + 1))
}

/// Loads the compat layer first without shifting line numbers. Classic
/// workers keep their directive prologue so `"use strict"` still applies, and
/// the call is guarded because packages also list the worker file under
/// `background.scripts`, where it may run in a page.
pub(crate) fn inject_worker(src: &[u8], module: bool, url: &str) -> Vec<u8> {
    let mut start = if src.starts_with(b"\xef\xbb\xbf") {
        3
    } else {
        0
    };
    let mut newline = "";
    if src[start..].starts_with(b"#!") {
        match src[start..].iter().position(|&b| b == b'\n') {
            Some(n) => start += n + 1,
            None => {
                start = src.len();
                newline = "\n";
            }
        }
    }
    let (at, statement) = if module {
        (start, format!("{newline}import \"{url}\";"))
    } else {
        let (at, needs_semicolon) = directive_prologue_end(src, start);
        let separator = if needs_semicolon { ";" } else { "" };
        (
            at,
            format!(
                "{newline}{separator}typeof importScripts===\"function\"&&importScripts(\"{url}\");"
            ),
        )
    };
    [&src[..at], statement.as_bytes(), &src[at..]].concat()
}

fn directive_prologue_end(src: &[u8], start: usize) -> (usize, bool) {
    let mut insert = (start, false);
    let mut pos = start;
    loop {
        let (token, _) = skip_trivia(src, pos);
        let Some(quote @ (b'"' | b'\'')) = src.get(token).copied() else {
            return insert;
        };
        let Some(string_end) = js_string_end(src, token, quote) else {
            return insert;
        };
        let (next, newline) = skip_trivia(src, string_end);
        match src.get(next) {
            Some(b';') => {
                insert = (next + 1, false);
                pos = next + 1;
            }
            None | Some(b'}') => return (string_end, true),
            Some(&b) if newline && ends_statement_by_asi(src, next, b) => {
                insert = (string_end, true);
                pos = string_end;
            }
            Some(_) => return insert,
        }
    }
}

fn ends_statement_by_asi(src: &[u8], pos: usize, b: u8) -> bool {
    if matches!(b, b'"' | b'\'' | b'{' | b'!' | b'~') {
        return true;
    }
    let word = &src[pos..ident_end(src, pos)];
    is_ident(b) && word != b"in" && word != b"instanceof"
}

fn js_string_end(src: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut i = start + 1;
    while let Some(&b) = src.get(i) {
        match b {
            b'\\' => i += 2,
            b'\n' | b'\r' => return None,
            _ if b == quote => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Skips whitespace and comments, reporting whether a line break was crossed.
fn skip_trivia(src: &[u8], mut pos: usize) -> (usize, bool) {
    let mut newline = false;
    loop {
        match (src.get(pos), src.get(pos + 1)) {
            (Some(b'\n' | b'\r'), _) => {
                newline = true;
                pos += 1;
            }
            (Some(b), _) if b.is_ascii_whitespace() => pos += 1,
            (Some(b'/'), Some(b'/')) => {
                pos = src[pos..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(src.len(), |n| pos + n);
            }
            (Some(b'/'), Some(b'*')) => {
                let end = src[pos + 2..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .map_or(src.len(), |n| pos + 2 + n + 2);
                newline |= src[pos..end].iter().any(|&b| b == b'\n' || b == b'\r');
                pos = end;
            }
            _ => return (pos, newline),
        }
    }
}

fn inject_html(html: &str, script_tag: &str) -> String {
    let at = html_insertion_point(html);
    [&html[..at], script_tag, &html[at..]].concat()
}

/// Just after `<head>`, else `<html>`, else a leading doctype, else the start.
fn html_insertion_point(html: &str) -> usize {
    let bom = if html.starts_with('\u{feff}') { 3 } else { 0 };
    let lower = html.to_ascii_lowercase();
    let mut html_tag = None;
    let mut i = bom;
    while let Some(offset) = lower[i..].find('<') {
        let start = i + offset;
        let rest = &lower[start..];
        if rest.starts_with("<!--") {
            i = start + rest.find("-->").map_or(rest.len(), |n| n + 3);
            continue;
        }
        let name_len = rest[1..]
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len() - 1);
        let name = &rest[1..1 + name_len];
        if name.is_empty() && !rest[1..].starts_with(['/', '!', '?']) {
            i = start + 1;
            continue;
        }
        let Some(end) = tag_end(rest) else {
            break;
        };
        let after = start + end + 1;
        match name {
            "head" => return after,
            "html" if html_tag.is_none() => html_tag = Some(after),
            "body" => break,
            "script" | "style" | "title" | "textarea" => {
                let close = format!("</{name}");
                i = lower[after..]
                    .find(&close)
                    .map_or(lower.len(), |n| after + n);
                continue;
            }
            _ => {}
        }
        i = after;
    }
    if let Some(after) = html_tag {
        return after;
    }
    let leading = bom + (html.len() - bom - html[bom..].trim_start().len());
    if lower[leading..].starts_with("<!doctype") {
        if let Some(end) = tag_end(&lower[leading..]) {
            return leading + end + 1;
        }
    }
    bom
}

/// Edits the manifest as text: serde_json's `preserve_order` would unify
/// across the workspace, and a textual edit also keeps number formatting and
/// layout. The output is strict JSON since WebKit may not share Chrome's
/// leniency.
fn edit_manifest(
    original: &str,
    raw: &Value,
    resource: &str,
    added_permissions: &[String],
) -> Result<String, PrepareError> {
    let text = manifest::to_strict_json(original);
    let root = SpanParser::parse(&text).ok_or(PrepareError::ManifestLayout)?;
    let entry = format!("\"{resource}\"");
    let mut inserts = Vec::new();

    if !added_permissions.is_empty() {
        let list = added_permissions
            .iter()
            .map(|permission| serde_json::to_string(permission).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(", ");
        match root.get("permissions") {
            Some(
                node @ Node {
                    kind: Kind::Array(items),
                    ..
                },
            ) => match items.first() {
                Some(_) => inserts.extend(prepend(&text, node, &list)),
                None => inserts.push((node.start + 1, list)),
            },
            Some(_) => return Err(PrepareError::ManifestLayout),
            None => inserts.push((root.start + 1, format!("\"permissions\": [{list}], "))),
        }
    }

    if let Some(scripts) = root.get("background").and_then(|b| b.get("scripts")) {
        inserts.extend(prepend(&text, scripts, &entry));
    }
    if let (Some(Kind::Array(nodes)), Some(values)) = (
        root.get("content_scripts").map(|n| &n.kind),
        raw["content_scripts"].as_array(),
    ) {
        for (node, value) in nodes.iter().zip(values) {
            if value["world"].as_str() == Some("MAIN") {
                continue;
            }
            if let Some(js) = node.get("js") {
                inserts.extend(prepend(&text, js, &entry));
            }
        }
    }

    inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let mut out = text;
    for (at, insert) in inserts {
        out.insert_str(at, &insert);
    }
    Ok(out)
}

/// An insertion placing `entry` first in a non-empty array, matching the
/// whitespace that already separates `[` from the first element.
fn prepend(text: &str, array: &Node, entry: &str) -> Option<(usize, String)> {
    let Kind::Array(items) = &array.kind else {
        return None;
    };
    let first = items.first()?.start;
    let gap = &text[array.start + 1..first];
    let separator = if gap.contains('\n') { gap } else { " " };
    Some((first, format!("{entry},{separator}")))
}

struct Node {
    start: usize,
    kind: Kind,
}

enum Kind {
    Object(Vec<(String, Node)>),
    Array(Vec<Node>),
    Scalar,
}

impl Node {
    fn get(&self, key: &str) -> Option<&Node> {
        match &self.kind {
            Kind::Object(fields) => fields.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Records where each value of an already-validated JSON document starts.
struct SpanParser<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> SpanParser<'a> {
    fn parse(text: &'a str) -> Option<Node> {
        let mut parser = Self { text, pos: 0 };
        let node = parser.value()?;
        parser.skip_ws();
        (parser.pos == text.len()).then_some(node)
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Option<Node> {
        self.skip_ws();
        let start = self.pos;
        let kind = match self.peek()? {
            b'{' => Kind::Object(self.members(b'}', |p| {
                let key_start = p.pos;
                p.pos = manifest::string_end(p.text.as_bytes(), key_start);
                let key: String = serde_json::from_str(&p.text[key_start..p.pos]).ok()?;
                p.skip_ws();
                (p.peek()? == b':').then_some(())?;
                p.pos += 1;
                Some((key, p.value()?))
            })?),
            b'[' => Kind::Array(self.members(b']', Self::value)?),
            b'"' => {
                self.pos = manifest::string_end(self.text.as_bytes(), start);
                Kind::Scalar
            }
            _ => {
                while self
                    .peek()
                    .is_some_and(|b| !b.is_ascii_whitespace() && !b",]}".contains(&b))
                {
                    self.pos += 1;
                }
                Kind::Scalar
            }
        };
        Some(Node { start, kind })
    }

    fn members<T>(
        &mut self,
        close: u8,
        mut member: impl FnMut(&mut Self) -> Option<T>,
    ) -> Option<Vec<T>> {
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek()? == close {
            self.pos += 1;
            return Some(items);
        }
        loop {
            self.skip_ws();
            items.push(member(self)?);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b if b == close => {
                    self.pos += 1;
                    return Some(items);
                }
                _ => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "/__zephium__/compat.js";

    fn worker(src: &str, module: bool) -> String {
        String::from_utf8(inject_worker(src.as_bytes(), module, URL)).unwrap()
    }

    fn classic(prefix: &str) -> String {
        format!("{prefix}typeof importScripts===\"function\"&&importScripts(\"{URL}\");")
    }

    #[test]
    fn classic_worker_loads_compat_on_the_first_line() {
        let out = worker("self.x = 1;\nfoo();\n", false);
        assert_eq!(out, format!("{}self.x = 1;\nfoo();\n", classic("")));
    }

    #[test]
    fn classic_worker_keeps_directive_prologue() {
        let src = "/* license */\n'use strict';\n\"use asm\"; foo();";
        let out = worker(src, false);
        assert_eq!(
            out,
            format!(
                "/* license */\n'use strict';\n\"use asm\";{} foo();",
                classic("")
            )
        );
        assert_eq!(out.lines().count(), src.lines().count());

        let asi = worker("\"use strict\"\nfoo()", false);
        assert_eq!(asi, format!("\"use strict\"{}\nfoo()", classic(";")));

        let not_directive = worker("\"use strict\" + x;", false);
        assert!(not_directive.starts_with("typeof importScripts"));
    }

    #[test]
    fn workers_keep_bom_and_hashbang_first() {
        let out = worker("\u{feff}#!/usr/bin/env node\nfoo();", false);
        assert_eq!(
            out,
            format!("\u{feff}#!/usr/bin/env node\n{}foo();", classic(""))
        );
        let module = worker("\u{feff}import a from \"./a.js\";\n", true);
        assert_eq!(
            module,
            format!("\u{feff}import \"{URL}\";import a from \"./a.js\";\n")
        );
    }

    fn html(src: &str) -> String {
        inject_html(src, "<S>")
    }

    #[test]
    fn html_injects_after_head() {
        assert_eq!(
            html("<!DOCTYPE html><HTML><!-- <head> --><Head data-x=\"a>b\"><title><head></title>"),
            "<!DOCTYPE html><HTML><!-- <head> --><Head data-x=\"a>b\"><S><title><head></title>"
        );
        assert_eq!(
            html("<html><header></header><head>"),
            "<html><header></header><head><S>"
        );
    }

    #[test]
    fn html_falls_back_to_html_doctype_or_start() {
        assert_eq!(
            html("<!doctype html><html lang='a>'><body><script>'<head>'</script>"),
            "<!doctype html><html lang='a>'><S><body><script>'<head>'</script>"
        );
        assert_eq!(
            html("\n<!DOCTYPE html>\n<p>x"),
            "\n<!DOCTYPE html><S>\n<p>x"
        );
        assert_eq!(
            html("\u{feff}<div>1 < 2</div>"),
            "\u{feff}<S><div>1 < 2</div>"
        );
    }

    #[test]
    fn scans_event_accesses() {
        let mut events = BTreeSet::new();
        scan_events(
            b"chrome.runtime.onMessage.addListener(f); browser.storage.local.onChanged;\
              chrome.tabs?.onUpdated; chrome.system.display.onDisplayChanged;\
              chrome.runtime\n  .onInstalled; x.runtime.onmessage; myruntime.onFoo;\
              chrome.tabs.query; chrome.alarms.on_Alarm; chrome.menus.onClicked",
            &mut events,
        );
        assert_eq!(
            events.into_iter().collect::<Vec<_>>(),
            [
                "menus.onClicked",
                "runtime.onInstalled",
                "runtime.onMessage",
                "storage.onChanged",
                "system.display.onDisplayChanged",
                "tabs.onUpdated",
            ]
        );
    }

    fn write(dir: &Path, path: &str, contents: &[u8]) {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn read(dir: &Path, path: &str) -> String {
        fs::read_to_string(dir.join(path)).unwrap()
    }

    const MV3: &str = r#"{
  "name": "Test",
  "manifest_version": 3,
  "version": "1.0",
  "background": {
    "service_worker": "sw.js"
  },
  "content_scripts": [
    {
      "matches": ["<all_urls>"],
      "js": [
        "isolated.js"
      ]
    },
    {"world": "MAIN", "matches": ["<all_urls>"], "js": ["main.js"]},
    {"matches": ["<all_urls>"], "css": ["a.css"]}
  ],
  "sandbox": {"pages": ["sandbox.html"]},
  "externally_connectable": {"matches": ["https://a.test/*"]},
  "zeta": 1.50,
  "alpha": true
}"#;

    #[test]
    fn prepares_an_mv3_package() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        write(dir, "manifest.json", MV3.as_bytes());
        write(
            dir,
            "sw.js",
            b"'use strict';\nchrome.runtime.onInstalled.addListener(f);\n",
        );
        write(
            dir,
            "isolated.js",
            b"chrome.storage.sync.onChanged.addListener(f);",
        );
        write(dir, "main.js", b"");
        write(dir, "popup/index.html", b"<html><head></head></html>");
        write(dir, "sandbox.html", b"<head></head>");
        write(dir, "latin1.htm", b"<head>\xe9</head>");

        let report = prepare(dir, &CompatLayer::new("/* body */")).unwrap();
        assert_eq!(report.events, ["runtime.onInstalled", "storage.onChanged"]);
        assert_eq!(report.worker.as_deref(), Some("sw.js"));
        assert_eq!(report.html_injected, 1);
        assert_eq!(report.html_skipped, ["latin1.htm"]);
        assert!(report.manifest_rewritten);

        assert_eq!(
            read(dir, "__zephium__/compat.js"),
            "var __zephiumConfig = {\"events\":[\"runtime.onInstalled\",\"storage.onChanged\"],\
             \"manifestVersion\":3};\n/* body */"
        );
        assert!(read(dir, "sw.js").starts_with("'use strict';typeof importScripts"));
        assert_eq!(
            read(dir, "popup/index.html"),
            format!("<html><head><script src=\"{URL}\"></script></head></html>")
        );
        assert_eq!(read(dir, "sandbox.html"), "<head></head>");

        let expected = MV3.replacen(
            "\"js\": [\n        \"isolated.js\"",
            "\"js\": [\n        \"__zephium__/compat.js\",\n        \"isolated.js\"",
            1,
        );
        assert_eq!(read(dir, "manifest.json"), expected);
        let manifest = Manifest::load(dir).unwrap();
        assert_eq!(manifest.content_scripts()[1].js, ["main.js"]);

        assert!(matches!(
            prepare(dir, &CompatLayer::new("")),
            Err(PrepareError::AlreadyPrepared)
        ));
    }

    #[test]
    fn prepares_background_scripts_and_pages() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let manifest = "\u{feff}{\"manifest_version\": 2, // mv2\n \
            \"background\": {\"scripts\": [\"a.js\", \"b.js\",], \"persistent\": false},\n \
            \"options_page\": \"options.html\"}";
        write(dir, "manifest.json", manifest.as_bytes());
        write(dir, "options.html", b"<!doctype html><p>");

        let report = prepare(dir, &CompatLayer::new("")).unwrap();
        assert_eq!(report.worker, None);
        assert_eq!(report.html_injected, 1);
        assert_eq!(
            read(dir, "manifest.json"),
            "{\"manifest_version\": 2, \n \"background\": {\"scripts\": \
             [\"__zephium__/compat.js\", \"a.js\", \"b.js\"], \"persistent\": false},\n \
             \"options_page\": \"options.html\"}"
        );
        assert_eq!(
            read(dir, "options.html"),
            format!("<!doctype html><script src=\"{URL}\"></script><p>")
        );
    }

    #[test]
    fn prepares_module_workers_and_leaves_unchanged_manifests_alone() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let manifest = r#"{"manifest_version":3,"background":{"service_worker":"/bg/sw.mjs","type":"module"}}"#;
        write(dir, "manifest.json", manifest.as_bytes());
        write(dir, "bg/sw.mjs", b"import './a.js';");

        let report = prepare(dir, &CompatLayer::new("")).unwrap();
        assert!(!report.manifest_rewritten);
        assert_eq!(read(dir, "manifest.json"), manifest);
        assert_eq!(
            read(dir, "bg/sw.mjs"),
            format!("import \"{URL}\";import './a.js';")
        );
    }

    #[test]
    fn rejects_missing_workers_and_bad_file_names() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        write(
            dir,
            "manifest.json",
            br#"{"manifest_version":3,"background":{"service_worker":"sw.js"}}"#,
        );
        let mut compat = CompatLayer::new("");
        compat.file_name = "../x.js".into();
        assert!(matches!(
            prepare(dir, &compat),
            Err(PrepareError::InvalidFileName(_))
        ));
        assert!(matches!(
            prepare(dir, &CompatLayer::new("")),
            Err(PrepareError::MissingWorker(_))
        ));
    }

    #[test]
    fn compat_permissions_are_added_only_when_missing() {
        let edit = |manifest: &str, add: &[&str]| {
            let raw: Value = serde_json::from_str(manifest).unwrap();
            let add: Vec<String> = add.iter().map(|p| (*p).to_owned()).collect();
            let out = edit_manifest(manifest, &raw, "__zephium__/compat.js", &add).unwrap();
            serde_json::from_str::<Value>(&out).unwrap()["permissions"].clone()
        };
        assert_eq!(
            edit(r#"{"name":"a","version":"1"}"#, &["nativeMessaging"]),
            json!(["nativeMessaging"])
        );
        assert_eq!(
            edit(r#"{"permissions":[],"name":"a"}"#, &["nativeMessaging"]),
            json!(["nativeMessaging"])
        );
        assert_eq!(
            edit(
                r#"{"permissions":["tabs", "storage"]}"#,
                &["nativeMessaging"]
            ),
            json!(["nativeMessaging", "tabs", "storage"])
        );
        assert_eq!(edit(r#"{"permissions":["tabs"]}"#, &[]), json!(["tabs"]));
    }
}
