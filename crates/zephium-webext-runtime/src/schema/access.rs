use serde_json::{Map, Value};

use super::{array, catalog, flag};
use crate::context::{ContextKind, ContextMask};
use crate::permissions::Gate;

/// Chromium feature metadata projected onto the runtime's context kinds.
/// Zephium's security ceiling is applied separately at registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    /// Required API permission.
    pub gate: Gate,
    /// Contexts admitted by the generated feature rules.
    pub contexts: ContextMask,
}

/// Resolves generated method or event-listener metadata. Unsupported permission
/// expressions fail closed instead of collapsing distinct authorization paths.
pub fn access(name: &str) -> Option<Access> {
    let catalog = catalog().ok()?;
    let method = catalog.methods.get(super::source_method(name))?;
    project(&method.feature_rules, &catalog.namespace_rules, 0)
}

fn project(
    rules: &'static [Value],
    namespaces: &'static std::collections::HashMap<String, Vec<Value>>,
    depth: usize,
) -> Option<Access> {
    if depth >= 16 || rules.is_empty() {
        return None;
    }
    let mut ordered: Vec<_> = rules.iter().collect();
    ordered.sort_by_key(|rule| rule.get("name").and_then(Value::as_str).map(str::len));
    let mut parent = Some(Map::new());
    let mut alternatives = Vec::new();
    for entry in ordered {
        let rule = entry.get("rules")?;
        let choices = rule
            .as_array()
            .map_or_else(|| vec![rule], |v| v.iter().collect());
        alternatives.clear();
        for choice in &choices {
            let mut inherited = if flag(choice, "noparent") {
                Map::new()
            } else {
                parent.clone()?
            };
            for (key, value) in choice.as_object()? {
                inherited.insert(key.clone(), value.clone());
            }
            alternatives.push(inherited);
        }
        let default = choices
            .iter()
            .position(|v| flag(v, "default_parent"))
            .or_else(|| (choices.len() == 1).then_some(0));
        parent = default.map(|index| alternatives[index].clone());
    }
    let mut result: Option<Access> = None;
    for rule in alternatives {
        if rule
            .get("extension_types")
            .and_then(Value::as_array)
            .is_some_and(|types| !types.iter().any(|v| v.as_str() == Some("extension")))
        {
            continue;
        }
        let contexts = rule.get("contexts")?;
        let mut mask = if contexts.as_str() == Some("all") {
            ContextMask::ALL
        } else {
            ContextMask::NONE
        };
        if let Some(contexts) = contexts.as_array() {
            for context in contexts {
                mask = mask.union(match context.as_str()? {
                    "privileged_extension" | "blessed_extension" => {
                        ContextMask::EXTENSION_PAGES.without(ContextKind::Offscreen)
                    }
                    "unprivileged_extension" | "unblessed_extension" => {
                        ContextMask::of(ContextKind::ExtensionFrame)
                    }
                    "offscreen_extension" => ContextMask::of(ContextKind::Offscreen),
                    "content_script" => ContextMask::of(ContextKind::ContentScript),
                    "user_script" => ContextMask::of(ContextKind::UserScript),
                    "web_page" | "privileged_web_page" => {
                        ContextMask::of(ContextKind::ExternalPage)
                    }
                    "webui" | "webui_untrusted" => ContextMask::NONE,
                    _ => return None,
                });
            }
        }
        if mask.is_empty() {
            continue;
        }
        let mut gate = Gate::Open;
        if let Some(dependencies) = rule.get("dependencies") {
            for dependency in dependencies.as_array()? {
                let dependency = dependency.as_str()?;
                let required = if let Some(permission) = dependency.strip_prefix("permission:") {
                    // Recover the static string from the source rules, not the merged copy.
                    Gate::Requires(permission_name(rules, permission)?)
                } else if let Some(api) = dependency.strip_prefix("api:") {
                    let inherited = project(namespaces.get(api)?, namespaces, depth + 1)?;
                    mask = mask.intersection(inherited.contexts);
                    inherited.gate
                } else {
                    return None;
                };
                if gate != Gate::Open && required != Gate::Open && gate != required {
                    return None;
                }
                if required != Gate::Open {
                    gate = required;
                }
            }
        }
        if mask.is_empty() {
            continue;
        }
        if let Some(current) = &mut result {
            if current.gate != gate {
                return None;
            }
            current.contexts = current.contexts.union(mask);
        } else {
            result = Some(Access {
                gate,
                contexts: mask,
            });
        }
    }
    result
}

fn permission_name(rules: &'static [Value], permission: &str) -> Option<&'static str> {
    for entry in rules {
        let rule = entry.get("rules")?;
        let choices = rule
            .as_array()
            .map_or_else(|| vec![rule], |v| v.iter().collect());
        for choice in choices {
            for dependency in array(choice, "dependencies") {
                if let Some(name) = dependency.as_str()?.strip_prefix("permission:") {
                    if name == permission {
                        return Some(name);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{collections::HashMap, sync::OnceLock};

    #[test]
    fn inheritance_preserves_permissions_and_replaces_contexts() {
        static RULES: OnceLock<Vec<Value>> = OnceLock::new();
        static NAMESPACES: OnceLock<HashMap<String, Vec<Value>>> = OnceLock::new();
        let rules = RULES.get_or_init(|| vec![
            json!({"name":"sample.call", "rules":{"contexts":["content_script"]}}),
            json!({"name":"sample", "rules":[
                {"default_parent":true,"contexts":["privileged_extension"],"dependencies":["permission:storage"]},
                {"contexts":["webui"]}
            ]}),
        ]);
        assert_eq!(
            project(rules, NAMESPACES.get_or_init(HashMap::new), 0),
            Some(Access {
                gate: Gate::Requires("storage"),
                contexts: ContextMask::of(ContextKind::ContentScript),
            })
        );
        assert_eq!(
            access("storage.session.get").unwrap().gate,
            Gate::Requires("storage")
        );
        assert_eq!(
            access("windows.onCreated.addListener").unwrap().gate,
            Gate::Open
        );
        assert!(access("runtime.sendMessage")
            .unwrap()
            .contexts
            .contains(ContextKind::ExternalPage));
        assert!(access("runtime.onMessage.addListener")
            .unwrap()
            .contexts
            .contains(ContextKind::UserScript));
        assert!(!access("i18n.getMessage")
            .unwrap()
            .contexts
            .contains(ContextKind::UserScript));
        assert!(access("unknown.call").is_none());
    }

    #[test]
    fn unsupported_or_ambiguous_rules_fail_closed() {
        static CASES: OnceLock<Vec<Vec<Value>>> = OnceLock::new();
        static NAMESPACES: OnceLock<HashMap<String, Vec<Value>>> = OnceLock::new();
        let cases = CASES.get_or_init(|| vec![
            vec![json!({"name":"sample", "rules":{"contexts":["MAIN"]}})],
            vec![json!({"name":"sample", "rules":{"dependencies":["permission:storage"]}})],
            vec![json!({"name":"sample", "rules":{"contexts":["privileged_extension"], "dependencies":["api:missing"]}})],
            vec![json!({"name":"sample", "rules":{"contexts":["privileged_extension"], "dependencies":["permission:storage","permission:alarms"]}})],
            vec![json!({"name":"sample", "rules":[
                {"contexts":["privileged_extension"],"dependencies":["permission:storage"]},
                {"contexts":["content_script"]}
            ]})],
            vec![json!({"name":"sample", "rules":[
                {"contexts":["privileged_extension"],"dependencies":["permission:storage"]},
                {"contexts":["content_script"],"dependencies":["permission:alarms"]}
            ]}), json!({"name":"sample.call", "rules":{"contexts":["content_script"]}})],
        ]);
        for rules in cases {
            assert_eq!(
                project(rules, NAMESPACES.get_or_init(HashMap::new), 0),
                None,
                "{rules:?}"
            );
        }
    }
}
