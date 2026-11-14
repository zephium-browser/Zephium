//! Content scripts for ordinary tabs: the sealed script table the module
//! evaluates, the world grants gating it, and the style sheets WebKit applies.

use std::sync::Arc;

use zephium_core::ids::ExtensionInstallId;
use zephium_webext_runtime::context::InjectedWorld;
use zephium_webext_runtime::pattern::{ContentScriptMatcher, MatchOptions};
use zephium_webext_runtime::permissions::SiteAccess;
use zephium_webext_runtime::scripting_registry::{ExecutionWorld, InjectionTime};
use zephium_webkit_web_process_protocol::protocol::{
    MatchRule, RunAt, ScriptEntry, ScriptFile, ScriptTable, MAX_BOOTSTRAP_BYTES,
    MAX_SCRIPT_SOURCE_BYTES,
};

use super::host::glue::ContentInstall;
use crate::bridge::{world_name, WorldGrant};

/// A content-script style sheet, applied by the tab's content manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabStyle {
    pub install: ExtensionInstallId,
    pub source: String,
    pub allow: Vec<String>,
    pub block: Vec<String>,
    pub all_frames: bool,
}

pub(crate) struct Content {
    pub table: ScriptTable,
    pub grants: Vec<WorldGrant>,
    pub styles: Vec<TabStyle>,
    pub problems: Vec<(ExtensionInstallId, String)>,
}

fn hex(install: ExtensionInstallId) -> String {
    install
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read(root: &std::path::Path, extension: &str, path: &str) -> Result<ScriptFile, String> {
    let normalized = zephium_webext::manifest::normalize_resource(path)
        .filter(|path| !path.starts_with('_'))
        .ok_or_else(|| format!("invalid content script path {path}"))?;
    let full = root.join(&normalized);
    let metadata = std::fs::symlink_metadata(&full).map_err(|_| format!("missing {path}"))?;
    if !metadata.is_file() || metadata.len() > MAX_SCRIPT_SOURCE_BYTES as u64 {
        return Err(format!("{path} is not a file of at most 2 MiB"));
    }
    let bytes = std::fs::read(&full).map_err(|_| format!("unreadable {path}"))?;
    let source = String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8"))?;
    let source = source.trim_start_matches('\u{feff}').to_owned();
    Ok(ScriptFile {
        source,
        url: format!("chrome-extension://{extension}/{normalized}"),
    })
}

fn bootstrap(install: &ContentInstall) -> Result<String, String> {
    let source = format!(
        "(function(){{'use strict';({})('native-content',{},{});}})();",
        zephium_webext_runtime::js::BUNDLE,
        zephium_webext_runtime::js::surface(),
        install.bootstrap
    );
    if source.len() > MAX_BOOTSTRAP_BYTES || source.contains('\0') {
        return Err("its content-script bootstrap exceeds the module limit".into());
    }
    Ok(source)
}

fn rule(matcher: ContentScriptMatcher, all_frames: bool, file_access: bool) -> MatchRule {
    MatchRule {
        matcher,
        all_frames,
        file_access,
    }
}

fn origins_rule(patterns: &[String], file_access: bool) -> Option<MatchRule> {
    if patterns.is_empty() {
        return None;
    }
    ContentScriptMatcher::new(
        patterns,
        &[],
        &[],
        &[],
        MatchOptions {
            match_about_blank: true,
            match_origin_as_fallback: true,
        },
    )
    .ok()
    .map(|matcher| rule(matcher, true, file_access))
}

/// WebKit user-content patterns for Chrome match patterns. `<all_urls>`
/// means no allow list.
fn webkit_patterns(patterns: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for pattern in patterns {
        if pattern == "<all_urls>" {
            return Some(Vec::new());
        }
        out.push(pattern.clone());
    }
    Some(out)
}

pub(crate) fn build(installs: &[ContentInstall], generation: u64) -> Content {
    let mut entries = Vec::new();
    let mut grants = Vec::new();
    let mut styles = Vec::new();
    let mut problems = Vec::new();
    for install in installs {
        let sites: Option<Vec<String>> = match &install.site_access {
            SiteAccess::OnAllSites => None,
            SiteAccess::OnSpecificSites(sites) => {
                Some(sites.iter().map(|site| site.as_str().to_owned()).collect())
            }
            SiteAccess::OnClick => Some(Vec::new()),
        };
        let isolated = world_name(install.install, &InjectedWorld::Isolated);
        let content_bootstrap = match bootstrap(install) {
            Ok(source) => source,
            Err(problem) => {
                problems.push((install.install, problem));
                continue;
            }
        };
        let mut grant_rules = Vec::new();
        let mut own: Vec<ScriptEntry> = Vec::new();
        let mut failed = None;
        for (script, matcher) in &install.scripts {
            let world = match script.world {
                ExecutionWorld::Isolated => isolated.clone(),
                ExecutionWorld::Main => String::new(),
            };
            if !script.js.is_empty() {
                let files: Result<Vec<ScriptFile>, String> = script
                    .js
                    .iter()
                    .map(|path| read(&install.root, &install.extension_id, path))
                    .collect();
                let files = match files {
                    Ok(files) => files,
                    Err(problem) => {
                        failed = Some(problem);
                        break;
                    }
                };
                let entry_rule = rule(matcher.clone(), script.all_frames, install.file_access);
                if sites.is_none() {
                    grant_rules.push(entry_rule.clone());
                }
                own.push(ScriptEntry {
                    install_id: hex(install.install),
                    script_id: script.id.clone(),
                    bootstrap: if world.is_empty() {
                        String::new()
                    } else {
                        content_bootstrap.clone()
                    },
                    world,
                    run_at: match script.run_at {
                        InjectionTime::DocumentStart => RunAt::DocumentStart,
                        InjectionTime::DocumentEnd => RunAt::DocumentEnd,
                        InjectionTime::DocumentIdle => RunAt::DocumentIdle,
                    },
                    rule: entry_rule,
                    files,
                    css: Vec::new(),
                });
            }
            if !script.css.is_empty() && sites.as_ref().is_none_or(|sites| !sites.is_empty()) {
                let mut source = String::new();
                for path in &script.css {
                    match read(&install.root, &install.extension_id, path) {
                        Ok(file) => {
                            source.push_str(&file.source);
                            source.push('\n');
                        }
                        Err(problem) => failed = Some(problem),
                    }
                }
                let allow = match &sites {
                    Some(sites) => Some(sites.clone()),
                    None => webkit_patterns(&script.matches),
                };
                if let Some(allow) = allow {
                    styles.push(TabStyle {
                        install: install.install,
                        source,
                        allow,
                        block: script.exclude_matches.clone(),
                        all_frames: script.all_frames,
                    });
                }
            }
        }
        if let Some(problem) = failed {
            problems.push((install.install, problem));
            styles.retain(|style| style.install != install.install);
            continue;
        }
        let access = sites.as_deref().unwrap_or(&install.origins);
        if let Some(origins) = origins_rule(access, install.file_access) {
            grant_rules.push(origins);
        }
        if grant_rules.is_empty() {
            continue;
        }
        entries.extend(own);
        grants.push(WorldGrant {
            install: install.install,
            world: InjectedWorld::Isolated,
            matches: grant_rules,
            bootstrap: Arc::from(""),
        });
    }
    Content {
        table: ScriptTable {
            generation,
            entries,
        },
        grants,
        styles,
        problems,
    }
}
