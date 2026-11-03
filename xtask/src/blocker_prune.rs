//! Release tooling for `crates/zephium-blocker/src/webkit_pruned.txt`: the
//! wildcard block rules a recorded page corpus never matched. The corpus is
//! recorded and replayed through WebKit with `scripts/blocker-corpus`; this
//! turns WebKit's own match output into the shipped digest list. The steps are
//! in docs/adblock.md.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::value::RawValue;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// One notify rule per resource type, so a replayed load that WebKit never
/// checked can be told apart from one that matched nothing.
const RESOURCE_TYPES: [&str; 12] = [
    "top-document",
    "child-document",
    "image",
    "style-sheet",
    "script",
    "font",
    "fetch",
    "websocket",
    "svg-document",
    "media",
    "ping",
    "other",
];

pub(crate) fn run(arguments: &[String]) -> Result<(), String> {
    let (command, rest) = arguments.split_first().ok_or_else(usage)?;
    let options = Options::parse(rest)?;
    match command.as_str() {
        "materialize" => crate::blocker_seed::materialize_webkit_unpruned(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."),
            &options.path("output")?,
        ),
        "notify" => notify(&options.path("rules")?, &options.path("output")?),
        "hosts" => hosts(&options.path("rules")?, &options.path("output")?),
        "build" => build(
            &options.path("rules")?,
            &options.paths("matches")?,
            &options.text("corpus")?,
            &options.path("output")?,
        ),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: cargo xtask blocker-prune <materialize --output PATH \
     | notify --rules ARTIFACT --output PATH \
     | hosts --rules ARTIFACT --output PATH \
     | build --rules ARTIFACT --matches PATH... --corpus TEXT --output PATH>"
        .into()
}

struct Options(BTreeMap<String, Vec<String>>);

impl Options {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut options: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut current = None;
        for argument in arguments {
            if let Some(name) = argument.strip_prefix("--") {
                current = Some(name.to_owned());
                options.entry(name.to_owned()).or_default();
            } else if let Some(name) = &current {
                options
                    .entry(name.clone())
                    .or_default()
                    .push(argument.clone());
            } else {
                return Err(usage());
            }
        }
        Ok(Self(options))
    }

    fn paths(&self, name: &str) -> Result<Vec<PathBuf>, String> {
        match self.0.get(name) {
            Some(values) if !values.is_empty() => Ok(values.iter().map(PathBuf::from).collect()),
            _ => Err(format!("--{name} is required\n{}", usage())),
        }
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        let mut paths = self.paths(name)?;
        if paths.len() != 1 {
            return Err(format!("--{name} takes one value"));
        }
        Ok(paths.remove(0))
    }

    fn text(&self, name: &str) -> Result<String, String> {
        Ok(self
            .paths(name)?
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" "))
    }
}

/// The artifact's rules as emitted, each with its parsed form.
fn rules(path: &Path) -> Result<Vec<(Box<RawValue>, Value)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let raw: Vec<Box<RawValue>> =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    raw.into_iter()
        .map(|rule| {
            let value = serde_json::from_str(rule.get()).map_err(|e| e.to_string())?;
            Ok((rule, value))
        })
        .collect()
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

fn is_wildcard_block(rule: &Value) -> bool {
    rule["action"]["type"] == "block"
        && rule["trigger"]["url-filter"]
            .as_str()
            .is_some_and(|filter| filter.contains(".*"))
}

/// A notify list for the replay: one rule per resource type, then every
/// artifact rule in order, notifying its index.
fn notify(rules_path: &Path, output: &Path) -> Result<(), String> {
    let mut list: Vec<Value> = RESOURCE_TYPES
        .iter()
        .map(|kind| {
            json!({ "trigger": { "url-filter": ".*", "resource-type": [kind] },
                    "action": { "type": "notify", "notification": kind } })
        })
        .collect();
    for (index, (_, rule)) in rules(rules_path)?.into_iter().enumerate() {
        list.push(json!({ "trigger": rule["trigger"], "action": { "type": "notify", "notification": index.to_string() } }));
    }
    write(
        output,
        &serde_json::to_string(&list).map_err(|e| e.to_string())?,
    )
}

/// Pages for the hosts the wildcard rules name, so the corpus covers the
/// sites those rules exist for. Wildcard hosts and adult sites are left out.
fn hosts(rules_path: &Path, output: &Path) -> Result<(), String> {
    const PREFIX: &str = "^[^:]+:(//)?([^/]+\\.)?";
    const ADULT: [&str; 12] = [
        "porn", "xxx", "sex", "adult", "nude", "escort", "hentai", "xvideo", "xhamster", "nsfw",
        "milf", "fetish",
    ];
    let mut pages = BTreeSet::new();
    for (_, rule) in rules(rules_path)? {
        let Some(filter) = rule["trigger"]["url-filter"].as_str() else {
            continue;
        };
        let Some(rest) = filter
            .strip_prefix(PREFIX)
            .filter(|_| filter.contains(".*"))
        else {
            continue;
        };
        let mut host = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                'a'..='z' | '0'..='9' | '-' => host.push(c),
                '\\' if chars.peek() == Some(&'.') => {
                    chars.next();
                    host.push('.');
                }
                _ => break,
            }
        }
        let terminated =
            rest[host.len() + host.matches('.').count()..].starts_with(['/', '[', '$']);
        if !terminated || !host.contains('.') || host.ends_with('.') {
            continue;
        }
        if ADULT.iter().any(|word| host.contains(word)) {
            continue;
        }
        pages.insert(format!("https://{host}/"));
    }
    let mut text = pages.into_iter().collect::<Vec<_>>().join("\n");
    text.push('\n');
    write(output, &text)
}

/// The digest list: every wildcard block rule no exercised replayed load
/// matched. A load counts as exercised when WebKit reported its resource type.
fn build(
    rules_path: &Path,
    matches: &[PathBuf],
    corpus: &str,
    output: &Path,
) -> Result<(), String> {
    let rules = rules(rules_path)?;
    let mut hits = vec![0usize; rules.len()];
    let (mut rows, mut exercised, mut pages) = (0usize, 0usize, BTreeSet::new());
    // Replays may overlap, and a load counts once.
    let mut loads = BTreeSet::new();
    for path in matches {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let row: Value =
                serde_json::from_str(line).map_err(|e| format!("{}: {e}", path.display()))?;
            if !loads.insert((
                row["top"].to_string(),
                row["url"].to_string(),
                row["type"].to_string(),
            )) {
                continue;
            }
            rows += 1;
            let seen: Vec<&str> = row["seen"]
                .as_str()
                .unwrap_or_default()
                .split(',')
                .collect();
            if !seen.contains(&row["type"].as_str().unwrap_or_default()) {
                continue;
            }
            exercised += 1;
            if let Some(page) = row["page"].as_str() {
                pages.insert(page.to_owned());
            }
            for index in seen.iter().filter_map(|note| note.parse::<usize>().ok()) {
                if let Some(count) = hits.get_mut(index) {
                    *count += 1;
                }
            }
        }
    }
    if exercised == 0 {
        return Err("no replayed load was exercised; check the replay".into());
    }
    let wildcard_blocks = rules
        .iter()
        .filter(|(_, rule)| is_wildcard_block(rule))
        .count();
    let digests: BTreeSet<String> = rules
        .iter()
        .zip(&hits)
        .filter(|((_, rule), hits)| **hits == 0 && is_wildcard_block(rule))
        .map(|((raw, _), _)| hex(&Sha256::digest(raw.get().as_bytes())))
        .collect();
    let artifact = std::fs::read(rules_path).map_err(|e| e.to_string())?;
    let mut text = String::new();
    let _ = writeln!(
        text,
        "# Wildcard block rules the recorded page corpus never matched: SHA-256 of each"
    );
    let _ = writeln!(
        text,
        "# rule as emitted for WebKit. Written by `cargo xtask blocker-prune build`; see"
    );
    let _ = writeln!(text, "# docs/adblock.md. Corpus: {corpus}.");
    let _ = writeln!(
        text,
        "# Replay: {rows} loads, {exercised} exercised, on {} pages.",
        pages.len()
    );
    let _ = writeln!(
        text,
        "# Artifact: {} rules, sha256 {}.",
        rules.len(),
        hex(&Sha256::digest(&artifact))
    );
    let _ = writeln!(
        text,
        "# Pruned: {} of {wildcard_blocks} wildcard block rules.",
        digests.len()
    );
    for digest in &digests {
        let _ = writeln!(text, "{digest}");
    }
    write(output, &text)?;
    println!(
        "{} of {wildcard_blocks} wildcard block rules matched nothing in {exercised} exercised loads ({rows} replayed)",
        digests.len()
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTIFACT: &str = concat!(
        r#"[{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?a\\.example/x.*y","resource-type":["image"]}},"#,
        r#"{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?b\\.example/x.*y","resource-type":["image"]}},"#,
        r#"{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?c\\.example[^A-Za-z0-9_.%-]","resource-type":["image"]}},"#,
        r#"{"action":{"type":"ignore-previous-rules"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?d\\.example/x.*y","resource-type":["image"]}}]"#
    );

    #[test]
    fn only_wildcard_blocks_no_exercised_load_matched_are_listed() {
        let directory = tempfile::tempdir().unwrap();
        let (rules, matches, output) = (
            directory.path().join("rules.json"),
            directory.path().join("matches.jsonl"),
            directory.path().join("pruned.txt"),
        );
        std::fs::write(&rules, ARTIFACT).unwrap();
        // Rule 1 matched an exercised load; rule 0 only one WebKit never checked.
        std::fs::write(
            &matches,
            concat!(
                r#"{"page":"p","type":"image","seen":"image,1,2"}"#,
                "\n",
                r#"{"page":"p","type":"script","seen":"0"}"#,
                "\n"
            ),
        )
        .unwrap();
        build(&rules, &[matches], "test", &output).unwrap();
        let written = std::fs::read_to_string(&output).unwrap();
        let entries: Vec<&str> = written
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect();
        let raw: Vec<Box<RawValue>> = serde_json::from_str(ARTIFACT).unwrap();
        assert_eq!(entries, [hex(&Sha256::digest(raw[0].get().as_bytes()))]);
        assert!(written.contains("Pruned: 1 of 2 wildcard block rules."));
    }

    #[test]
    fn host_pages_come_from_terminated_wildcard_hosts() {
        let directory = tempfile::tempdir().unwrap();
        let (rules, output) = (
            directory.path().join("rules.json"),
            directory.path().join("hosts.txt"),
        );
        std::fs::write(&rules, ARTIFACT).unwrap();
        hosts(&rules, &output).unwrap();
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            "https://a.example/\nhttps://b.example/\nhttps://d.example/\n"
        );
    }
}
