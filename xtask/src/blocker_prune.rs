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
            &options.paths("loads")?,
            &options.paths("matches")?,
            &options.paths("visits")?,
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
     | build --rules ARTIFACT --loads PATH... --matches PATH... --visits PATH... --corpus TEXT \
       --output PATH>"
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
    const ADULT: [&str; 12] = [
        "porn", "xxx", "sex", "adult", "nude", "escort", "hentai", "xvideo", "xhamster", "nsfw",
        "milf", "fetish",
    ];
    let mut pages = BTreeSet::new();
    for (_, rule) in rules(rules_path)? {
        let Some(host) = rule_host(&rule) else {
            continue;
        };
        if ADULT.iter().any(|word| host.contains(word)) {
            continue;
        }
        pages.insert(format!("https://{host}/"));
    }
    let mut text = pages.into_iter().collect::<Vec<_>>().join("\n");
    text.push('\n');
    write(output, &text)
}

/// A replayed load: its top page, URL and resource type.
type LoadKey = (String, String, String);

fn load_key(row: &Value) -> LoadKey {
    (
        row["top"].to_string(),
        row["url"].as_str().unwrap_or_default().to_owned(),
        row["type"].as_str().unwrap_or_default().to_owned(),
    )
}

/// The host a wildcard rule names, when its url-filter starts with a whole
/// host.
fn rule_host(rule: &Value) -> Option<String> {
    const PREFIX: &str = "^[^:]+:(//)?([^/]+\\.)?";
    let filter = rule["trigger"]["url-filter"].as_str()?;
    let rest = filter
        .strip_prefix(PREFIX)
        .filter(|_| filter.contains(".*"))?;
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
    let terminated = rest[host.len() + host.matches('.').count()..].starts_with(['/', '[', '$']);
    (terminated && host.contains('.') && !host.ends_with('.')).then_some(host)
}

/// The digest list: every wildcard block rule no replayed load matched. A load
/// counts as exercised when WebKit reported its resource type. Where the replay
/// could not test a rule, it is kept: a load WebKit did not check as its
/// recorded type counts for every rule of that type whose url-filter matches
/// it, and a rule that applies only on sites whose pages did not load is kept.
/// Every recorded load in `loads` must have a replay row.
fn build(
    rules_path: &Path,
    recorded: &[PathBuf],
    matches: &[PathBuf],
    visits: &[PathBuf],
    corpus: &str,
    output: &Path,
) -> Result<(), String> {
    let rules = rules(rules_path)?;
    let (visited, visits_loaded, loaded) = loaded_hosts(visits)?;
    let mut hits = vec![0usize; rules.len()];
    // Replays may overlap: one load's reports are merged across them.
    let mut loads: BTreeMap<LoadKey, (Option<String>, BTreeSet<String>)> = BTreeMap::new();
    for path in matches {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let row: Value =
                serde_json::from_str(line).map_err(|e| format!("{}: {e}", path.display()))?;
            let load = loads.entry(load_key(&row)).or_default();
            if let Some(page) = row["page"].as_str() {
                load.0 = Some(page.to_owned());
            }
            load.1.extend(
                row["seen"]
                    .as_str()
                    .unwrap_or_default()
                    .split(',')
                    .filter(|note| !note.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    let mut unreplayed = 0usize;
    for path in recorded {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let row: Value =
                serde_json::from_str(line).map_err(|e| format!("{}: {e}", path.display()))?;
            if !loads.contains_key(&load_key(&row)) {
                unreplayed += 1;
            }
        }
    }
    if unreplayed > 0 {
        return Err(format!(
            "{unreplayed} recorded loads have no replay row; rerun the replay"
        ));
    }
    let (mut exercised, mut untested, mut pages) = (0usize, Vec::new(), BTreeSet::new());
    for ((_, url, kind), (page, notes)) in &loads {
        for index in notes.iter().filter_map(|note| note.parse::<usize>().ok()) {
            if let Some(count) = hits.get_mut(index) {
                *count += 1;
            }
        }
        if notes.contains(kind) {
            exercised += 1;
            pages.extend(page.clone());
        } else {
            untested.push((url.as_str(), kind.as_str()));
        }
    }
    if exercised == 0 {
        return Err("no replayed load was exercised; check the replay".into());
    }
    let rows = loads.len();
    let (mut kept_untested, mut kept_unloaded) = (0usize, 0usize);
    for ((_, rule), count) in rules.iter().zip(&mut hits) {
        if *count > 0 || !is_wildcard_block(rule) {
            continue;
        }
        if untested_match(rule, &untested) {
            *count += 1;
            kept_untested += 1;
        } else if !applies_on_a_loaded_site(rule, &loaded) {
            *count += 1;
            kept_unloaded += 1;
        }
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
        "# Replay: {rows} loads, {exercised} exercised, on {} pages; {kept_untested} rules kept for {} untested loads.",
        pages.len(),
        untested.len()
    );
    let _ = writeln!(
        text,
        "# Visits: {visits_loaded} of {visited} loaded; {kept_unloaded} rules kept that apply only on sites that did not."
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

/// How many visits there were and how many loaded, and the hosts of those
/// that finished in time and loaded more than the page's own document.
fn loaded_hosts(visits: &[PathBuf]) -> Result<(usize, usize, BTreeSet<String>), String> {
    let mut loaded = BTreeSet::new();
    let (mut total, mut ok) = (0usize, 0usize);
    for path in visits {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let visit: Value =
                serde_json::from_str(line).map_err(|e| format!("{}: {e}", path.display()))?;
            total += 1;
            let loads: usize = visit["loads"]
                .as_str()
                .and_then(|loads| loads.parse().ok())
                .ok_or_else(|| format!("{}: a visit without a load count", path.display()))?;
            if visit["status"] == "ok" && loads > 1 {
                ok += 1;
                loaded.extend(visit["page"].as_str().and_then(url_host).map(str::to_owned));
            }
        }
    }
    if total == 0 {
        return Err("no recorded visits; record writes them next to the corpus".into());
    }
    Ok((total, ok, loaded))
}

fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

/// Whether an untested load of a type the rule applies to matches its
/// url-filter. Domain and load-type conditions are left out, so this can only
/// keep more rules; a url-filter the regex engine refuses keeps its rule.
fn untested_match(rule: &Value, untested: &[(&str, &str)]) -> bool {
    let trigger = &rule["trigger"];
    let types = trigger["resource-type"].as_array();
    let Ok(filter) = regex::RegexBuilder::new(trigger["url-filter"].as_str().unwrap_or_default())
        .case_insensitive(trigger["url-filter-is-case-sensitive"] != true)
        .build()
    else {
        return true;
    };
    untested.iter().any(|(url, kind)| {
        types.is_none_or(|types| types.iter().any(|t| t == kind)) && filter.is_match(url)
    })
}

/// Whether a rule can match on any site whose page loaded: a rule limited to
/// listed sites (`if-domain`), or to first-party loads of the host it names,
/// can match nowhere else.
fn applies_on_a_loaded_site(rule: &Value, loaded: &BTreeSet<String>) -> bool {
    let trigger = &rule["trigger"];
    let on = |domain: &str| {
        let (name, subdomains) = match domain.strip_prefix('*') {
            Some(name) => (name, true),
            None => (domain, false),
        };
        loaded.iter().any(|host| {
            host == name
                || subdomains
                    && host
                        .strip_suffix(name)
                        .is_some_and(|rest| rest.ends_with('.'))
        })
    };
    if let Some(domains) = trigger["if-domain"].as_array() {
        return domains.iter().filter_map(Value::as_str).any(on);
    }
    if trigger["load-type"] == json!(["first-party"]) {
        if let Some(host) = rule_host(rule) {
            return on(&format!("*{host}"));
        }
    }
    true
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

    const LOADED: &str = concat!(
        r#"{"page":"https://a.example/","status":"ok","loads":"9"}"#,
        "\n",
        r#"{"page":"https://b.example/","status":"ok","loads":"9"}"#,
        "\n"
    );

    /// The listed entries, as indices into `artifact`, and the header.
    fn listed(artifact: &str, matches: &[&str], visits: &str) -> (Vec<usize>, String) {
        let directory = tempfile::tempdir().unwrap();
        let (rules, visits_path, output) = (
            directory.path().join("rules.json"),
            directory.path().join("visits.jsonl"),
            directory.path().join("pruned.txt"),
        );
        std::fs::write(&rules, artifact).unwrap();
        std::fs::write(&visits_path, visits).unwrap();
        let matches: Vec<PathBuf> = matches
            .iter()
            .enumerate()
            .map(|(n, text)| {
                let path = directory.path().join(format!("matches-{n}.jsonl"));
                std::fs::write(&path, text).unwrap();
                path
            })
            .collect();
        build(&rules, &matches, &matches, &[visits_path], "test", &output).unwrap();
        let written = std::fs::read_to_string(&output).unwrap();
        let raw: Vec<Box<RawValue>> = serde_json::from_str(artifact).unwrap();
        let digests: Vec<String> = raw
            .iter()
            .map(|rule| hex(&Sha256::digest(rule.get().as_bytes())))
            .collect();
        let indices = written
            .lines()
            .filter(|line| !line.starts_with('#'))
            .map(|line| digests.iter().position(|digest| digest == line).unwrap())
            .collect();
        (indices, written)
    }

    #[test]
    fn only_wildcard_blocks_no_replayed_load_matched_are_listed() {
        // Rule 1 matched a load WebKit did not check as its recorded type;
        // that still counts.
        let (listed, written) = listed(
            ARTIFACT,
            &[concat!(
                r#"{"page":"p","top":"t","url":"u1","type":"image","seen":"image,2"}"#,
                "\n",
                r#"{"page":"p","top":"t","url":"u2","type":"script","seen":"1"}"#,
                "\n"
            )],
            LOADED,
        );
        assert_eq!(listed, [0]);
        assert!(written.contains("Pruned: 1 of 2 wildcard block rules."));
    }

    #[test]
    fn a_match_in_any_replay_of_a_load_counts() {
        let load = r#""page":"p","top":"t","url":"u1","type":"image""#;
        let (listed, _) = listed(
            ARTIFACT,
            &[
                &format!("{{{load},\"seen\":\"image\"}}\n"),
                &format!("{{{load},\"seen\":\"image,0\"}}\n"),
            ],
            LOADED,
        );
        assert_eq!(listed, [1]);
    }

    #[test]
    fn an_untested_load_keeps_the_rules_of_its_type_that_match_it() {
        let artifact = concat!(
            r#"[{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?a\\.example/x.*y","resource-type":["image","other"]}},"#,
            r#"{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?b\\.example/x.*y","resource-type":["image"]}}]"#
        );
        // Root `other` loads replay as preconnects, which WebKit checks as `ping`.
        let (listed, written) = listed(
            artifact,
            &[concat!(
                r#"{"page":"p","top":"t","url":"https://A.example/x1y","type":"other","seen":"ping"}"#,
                "\n",
                r#"{"page":"p","top":"t","url":"https://b.example/x1y","type":"other","seen":"ping"}"#,
                "\n",
                r#"{"page":"p","top":"t","url":"https://c.example/","type":"image","seen":"image"}"#,
                "\n"
            )],
            LOADED,
        );
        assert_eq!(listed, [1]);
        assert!(written.contains("1 rules kept for 2 untested loads."));
    }

    #[test]
    fn a_rule_that_applies_only_on_sites_that_did_not_load_is_kept() {
        let artifact = concat!(
            r#"[{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?x\\.example/x.*y","if-domain":["*a.example"]}},"#,
            r#"{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?a\\.example/x.*y","load-type":["first-party"]}},"#,
            r#"{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?x\\.example/x.*y","if-domain":["*b.example"]}},"#,
            r#"{"action":{"type":"block"},"trigger":{"url-filter":"^[^:]+:(//)?([^/]+\\.)?a\\.example/z.*y"}}]"#
        );
        let visits = concat!(
            r#"{"page":"https://a.example/","status":"failed","loads":"1"}"#,
            "\n",
            r#"{"page":"https://www.b.example/","status":"ok","loads":"9"}"#,
            "\n"
        );
        let (listed, written) = listed(
            artifact,
            &[r#"{"page":"p","top":"t","url":"https://c.example/","type":"image","seen":"image"}"#],
            visits,
        );
        assert_eq!(listed, [2, 3]);
        assert!(written.contains("# Visits: 1 of 2 loaded; 2 rules kept"));
    }

    #[test]
    fn a_recorded_load_without_a_replay_row_fails_the_build() {
        let directory = tempfile::tempdir().unwrap();
        let path = |name: &str| directory.path().join(name);
        std::fs::write(path("rules.json"), ARTIFACT).unwrap();
        std::fs::write(path("visits.jsonl"), LOADED).unwrap();
        let replayed = r#"{"page":"p","top":"t","url":"u1","type":"image","seen":"image"}"#;
        std::fs::write(path("matches.jsonl"), replayed).unwrap();
        std::fs::write(
            path("corpus.jsonl"),
            format!(
                "{replayed}\n{}\n",
                r#"{"page":"p","top":"t","url":"u2","type":"image"}"#
            ),
        )
        .unwrap();
        let error = build(
            &path("rules.json"),
            &[path("corpus.jsonl")],
            &[path("matches.jsonl")],
            &[path("visits.jsonl")],
            "test",
            &path("pruned.txt"),
        )
        .unwrap_err();
        assert!(
            error.starts_with("1 recorded loads have no replay row"),
            "{error}"
        );
        assert!(!path("pruned.txt").exists());
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
