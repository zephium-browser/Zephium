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
    MAX_INLINE_SCRIPT_SOURCE_BYTES, MAX_SCRIPT_SOURCE_BYTES, MAX_SCRIPT_TABLE_BYTES,
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

pub(super) fn validate_file(
    root: &std::path::Path,
    path: &str,
    kind: zephium_webext_runtime::scripting_registry::ResourceKind,
) -> Result<(), zephium_webext_runtime::ApiError> {
    use zephium_webext_runtime::{scripting_registry::ResourceKind, ApiError};
    let path = zephium_webext::manifest::normalize_resource(path)
        .filter(|path| !path.starts_with('_'))
        .ok_or_else(|| ApiError::other("Invalid script path."))?;
    let limit = if kind == ResourceKind::JavaScript {
        MAX_SCRIPT_SOURCE_BYTES
    } else {
        MAX_INLINE_SCRIPT_SOURCE_BYTES
    };
    let metadata = std::fs::symlink_metadata(root.join(&path))
        .map_err(|_| ApiError::other(format!("Could not load file: '{path}'.")))?;
    if !metadata.is_file() {
        return Err(ApiError::other(format!("Could not load file: '{path}'.")));
    }
    if metadata.len() > limit as u64 {
        return Err(ApiError::other(format!(
            "Script '{path}' exceeds the {limit}-byte source limit."
        )));
    }
    Ok(())
}

struct MaterializationBudget(usize);

impl MaterializationBudget {
    fn charge(&mut self, bytes: usize) -> Result<(), String> {
        self.0 = self
            .0
            .checked_sub(bytes)
            .ok_or("Content sources exceed the bounded table materialization budget")?;
        Ok(())
    }
}

fn read(
    root: &std::path::Path,
    extension: &str,
    path: &str,
    limit: usize,
    budget: &mut MaterializationBudget,
) -> Result<ScriptFile, String> {
    let normalized = zephium_webext::manifest::normalize_resource(path)
        .filter(|path| !path.starts_with('_'))
        .ok_or_else(|| format!("invalid content script path {path}"))?;
    let full = root.join(&normalized);
    let metadata = std::fs::symlink_metadata(&full).map_err(|_| format!("missing {path}"))?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(format!("{path} is not a file of at most {limit} bytes"));
    }
    let url = format!("chrome-extension://{extension}/{normalized}");
    budget.charge(url.len())?;
    let limit = limit.min(budget.0);
    if metadata.len() > limit as u64 {
        return Err("Content sources exceed the bounded table materialization budget".into());
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(&full)
        .and_then(|file| file.take(limit as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| format!("unreadable {path}"))?;
    if bytes.len() > limit {
        return Err(format!("{path} exceeds the {limit}-byte source limit"));
    }
    budget.charge(bytes.len())?;
    let source = String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8"))?;
    let source = if source.starts_with('\u{feff}') {
        source.trim_start_matches('\u{feff}').to_owned()
    } else {
        source
    };
    Ok(ScriptFile { source, url })
}

fn bootstrap(data: &serde_json::Value) -> Result<(String, Option<Arc<str>>), String> {
    let mut data = data.clone();
    let catalog = data["messages"]
        .as_object()
        .filter(|messages| !messages.is_empty())
        .map(|messages| Arc::<str>::from(serde_json::to_string(messages).expect("JSON messages")));
    if catalog.is_some() {
        data.as_object_mut()
            .expect("bootstrap object")
            .remove("messages");
    }
    #[cfg(feature = "native-tests")]
    let bundle =
        crate::diagnostics::bundle(zephium_webext_runtime::js::NATIVE_CONTENT_BUNDLE, false);
    #[cfg(not(feature = "native-tests"))]
    let bundle = zephium_webext_runtime::js::NATIVE_CONTENT_BUNDLE;
    let source = format!(
        "(function(messages){{'use strict';{};const bootstrap={};if(messages)bootstrap.messages=messages;({})('native-content',{},bootstrap);}}){}",
        include_str!("idle_callback.js"),
        data,
        bundle,
        zephium_webext_runtime::js::context_surface(
            &zephium_webext::manifest::Manifest::from_value(data["manifest"].clone()),
            data["contextKind"].as_str().unwrap_or("contentScript"),
        ),
        if catalog.is_some() { "" } else { "();" }
    );
    if source.len() > MAX_BOOTSTRAP_BYTES || source.contains('\0') {
        return Err("its content-script bootstrap exceeds the module limit".into());
    }
    Ok((source, catalog))
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
    let mut budget = MaterializationBudget(MAX_SCRIPT_TABLE_BYTES);
    let mut entries = Vec::new();
    let mut admissions = std::collections::BTreeMap::new();
    let mut messaging_worlds = std::collections::BTreeSet::new();
    let mut catalogs = std::collections::BTreeMap::new();
    let mut bootstraps = std::collections::BTreeMap::new();
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
        let (content_bootstrap, catalog) = match bootstrap(&install.bootstrap) {
            Ok(source) => source,
            Err(problem) => {
                problems.push((install.install, problem));
                continue;
            }
        };
        if let Err(problem) = budget.charge(
            content_bootstrap
                .len()
                .saturating_add(catalog.as_ref().map_or(0, |v| v.len())),
        ) {
            problems.push((install.install, problem));
            break;
        }
        let mut grant_rules = Vec::new();
        let mut own: Vec<ScriptEntry> = Vec::new();
        let mut failed = None;
        for (script, matcher) in &install.scripts {
            let world = match script.world {
                ExecutionWorld::Isolated => isolated.clone(),
                ExecutionWorld::Main => String::new(),
            };
            if !script.js.is_empty() {
                if !world.is_empty() {
                    if let Err(problem) = budget.charge(content_bootstrap.len()) {
                        failed = Some(problem);
                        break;
                    }
                }
                let files: Result<Vec<ScriptFile>, String> = script
                    .js
                    .iter()
                    .map(|path| {
                        read(
                            &install.root,
                            &install.extension_id,
                            path,
                            MAX_SCRIPT_SOURCE_BYTES,
                            &mut budget,
                        )
                    })
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
                    admission_world: isolated.clone(),
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
                    match read(
                        &install.root,
                        &install.extension_id,
                        path,
                        MAX_INLINE_SCRIPT_SOURCE_BYTES,
                        &mut budget,
                    ) {
                        Ok(file) => {
                            match install.messages.localize_css(&file.source) {
                                Ok(localized)
                                    if localized.len() <= MAX_INLINE_SCRIPT_SOURCE_BYTES =>
                                {
                                    if let Err(problem) =
                                        budget.charge(localized.len().saturating_add(1))
                                    {
                                        failed = Some(problem);
                                        break;
                                    }
                                    source.push_str(&localized)
                                }
                                _ => {
                                    failed = Some(format!(
                                        "localized CSS exceeds its source limit: {path}"
                                    ));
                                    break;
                                }
                            }
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
        bootstraps.insert(
            isolated,
            zephium_webkit_web_process_protocol::protocol::ScriptBootstrap {
                install_id: hex(install.install),
                source: content_bootstrap,
            },
        );
        {
            if let Some(catalog) = catalog {
                catalogs.insert(hex(install.install), catalog);
            }
        }
        admissions.insert(hex(install.install), install.install_generation);
        entries.extend(own);
        grants.push(WorldGrant {
            install_generation: install.install_generation,
            install: install.install,
            world: InjectedWorld::Isolated,
            matches: grant_rules,
            bootstrap: Arc::from(""),
        });
    }
    for install in installs.iter().filter(|install| install.allow_user_scripts) {
        use sha2::{Digest, Sha256};
        use zephium_webext_runtime::context::UserScriptWorldId;
        use zephium_webext_runtime::user_scripts::World;
        let Some(hosts) = origins_rule(&install.origins, install.file_access) else {
            continue;
        };
        let owner = hex(install.install);
        let mut user_data = install.bootstrap.clone();
        user_data["contextKind"] = serde_json::json!("userScript");
        user_data["userScriptMessaging"] = serde_json::json!(true);
        let (messaging_bootstrap, catalog) = match bootstrap(&user_data) {
            Ok(result) => result,
            Err(problem) => {
                problems.push((install.install, problem));
                continue;
            }
        };
        if let Err(problem) = budget.charge(catalog.as_ref().map_or(0, |v| v.len())) {
            problems.push((install.install, problem));
            break;
        }
        if let Some(catalog) = catalog {
            catalogs.insert(owner.clone(), catalog);
        }
        user_data["userScriptMessaging"] = serde_json::json!(false);
        let muted = match bootstrap(&user_data) {
            Ok((source, _)) => source,
            Err(problem) => {
                problems.push((install.install, problem));
                continue;
            }
        };
        let mut worlds = std::collections::BTreeMap::new();
        for id in install.user_scripts.world_ids() {
            let world = InjectedWorld::UserScript(
                UserScriptWorldId::parse(id).expect("validated user world"),
            );
            let name = world_name(install.install, &world);
            let selected = if install.user_scripts.messaging(id) {
                &messaging_bootstrap
            } else {
                &muted
            };
            if let Err(problem) = budget.charge(selected.len().saturating_mul(2)) {
                problems.push((install.install, problem));
                break;
            }
            let source = if install.user_scripts.messaging(id) {
                messaging_worlds.insert(name.clone());
                messaging_bootstrap.clone()
            } else {
                muted.to_owned()
            };
            bootstraps.insert(
                name.clone(),
                zephium_webkit_web_process_protocol::protocol::ScriptBootstrap {
                    install_id: owner.clone(),
                    source: source.clone(),
                },
            );
            grants.push(WorldGrant {
                install: install.install,
                install_generation: install.install_generation,
                world,
                matches: vec![hosts.clone()],
                bootstrap: Arc::from(""),
            });
            worlds.insert(id, (name, source));
        }
        admissions.insert(owner.clone(), install.install_generation);
        for script in install.user_scripts.scripts() {
            let prepared = (|| -> Result<ScriptEntry, String> {
                let id = script.world_id.as_deref().unwrap_or("");
                let (admission_world, bootstrap) =
                    worlds.get(id).ok_or("Unknown user-script world")?;
                if script.world != World::Main {
                    budget.charge(bootstrap.len())?;
                }
                let files = script
                    .js
                    .iter()
                    .map(|source| {
                        if let Some(path) = &source.file {
                            read(
                                &install.root,
                                &install.extension_id,
                                path,
                                MAX_SCRIPT_SOURCE_BYTES,
                                &mut budget,
                            )
                        } else if let Some(code) = &source.code {
                            let url = format!(
                                "chrome-extension://{}/_user_scripts/{:x}.js",
                                install.extension_id,
                                Sha256::digest(code.as_bytes())
                            );
                            budget.charge(code.len().saturating_add(url.len()))?;
                            Ok(ScriptFile {
                                source: code.clone(),
                                url,
                            })
                        } else {
                            Err("Missing user-script source".into())
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ScriptEntry {
                    install_id: owner.clone(),
                    script_id: format!("_user-{:x}", Sha256::digest(script.id.as_bytes())),
                    admission_world: admission_world.clone(),
                    world: if script.world == World::Main {
                        String::new()
                    } else {
                        admission_world.clone()
                    },
                    bootstrap: if script.world == World::Main {
                        String::new()
                    } else {
                        bootstrap.clone()
                    },
                    run_at: match script.run_at {
                        InjectionTime::DocumentStart => RunAt::DocumentStart,
                        InjectionTime::DocumentEnd => RunAt::DocumentEnd,
                        InjectionTime::DocumentIdle => RunAt::DocumentIdle,
                    },
                    rule: rule(
                        script
                            .matcher()
                            .map_err(|error| error.message().to_owned())?,
                        script.all_frames,
                        install.file_access,
                    ),
                    files,
                    css: Vec::new(),
                })
            })();
            match prepared {
                Ok(entry) => entries.push(entry),
                Err(problem) => problems.push((install.install, problem)),
            }
        }
    }
    Content {
        table: ScriptTable {
            admissions,
            messaging_worlds,
            generation,
            catalogs,
            bootstraps,
            entries,
        },
        grants,
        styles,
        problems,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn repeated_files_are_bounded_before_reading_the_next_copy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.js"), "x".repeat(1024)).unwrap();
        let mut budget = super::MaterializationBudget(1500);
        super::read(root.path(), "id", "a.js", 4096, &mut budget).unwrap();
        assert!(super::read(root.path(), "id", "a.js", 4096, &mut budget).is_err());
        assert!(budget.0 < 500);
        assert!(budget.charge(usize::MAX).is_err());
    }

    #[test]
    fn large_translation_catalog_fits_without_dropping_messages_or_raising_limits() {
        let messages: serde_json::Map<String, serde_json::Value> = (0..2500)
            .map(|i| {
                (
                    format!("message{i}"),
                    serde_json::json!("A translated message with a positional $1"),
                )
            })
            .collect();
        let data = serde_json::json!({"contextKind":"contentScript", "manifest":{"manifest_version":3,"permissions":["storage"]}, "messages":messages});
        #[cfg(feature = "native-tests")]
        let directory = tempfile::tempdir().unwrap();
        #[cfg(feature = "native-tests")]
        crate::diagnostics::begin(directory.path(), "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        let (source, catalog) = super::bootstrap(&data).unwrap();
        #[cfg(feature = "native-tests")]
        crate::diagnostics::end("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert!(source.len() <= zephium_webkit_web_process_protocol::protocol::MAX_BOOTSTRAP_BYTES);
        assert!(!source.contains("message2499"));
        assert!(catalog.unwrap().contains("message2499"));
        assert!(!source.contains("function workerScope"));
        assert!(!source.contains("sidePanel"));
        assert_eq!(
            zephium_webext_runtime::i18n::MAX_LOCALE_MESSAGES,
            zephium_webkit_web_process_protocol::scripts::MAX_CATALOG_MESSAGES
        );
    }
    #[test]
    fn package_file_budget_preflight_rejects_oversize_and_keeps_css_bound() {
        use zephium_webext_runtime::scripting_registry::ResourceKind;
        let root = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(root.path().join("content.js")).unwrap();
        file.set_len(super::MAX_SCRIPT_SOURCE_BYTES as u64).unwrap();
        assert!(super::validate_file(root.path(), "content.js", ResourceKind::JavaScript).is_ok());
        assert!(super::validate_file(root.path(), "content.js", ResourceKind::Css).is_err());
        file.set_len(super::MAX_SCRIPT_SOURCE_BYTES as u64 + 1)
            .unwrap();
        let error =
            super::validate_file(root.path(), "content.js", ResourceKind::JavaScript).unwrap_err();
        assert!(error.message().contains("16777216-byte source limit"));
        assert!(
            super::validate_file(root.path(), "../content.js", ResourceKind::JavaScript).is_err()
        );
    }
}
