//! Bounded user-script registrations and native world configuration.

use crate::pattern::ContentScriptMatcher;
use crate::{context::UserScriptWorldId, scripting_registry::InjectionTime, ApiError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Persistent user-script state is bounded independently of extension storage.
pub const MAX_STATE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum registrations per installation.
pub const MAX_SCRIPTS: usize = 1024;
/// Maximum explicitly configured user worlds per installation.
pub const MAX_WORLDS: usize = 16;

/// One ordered source; arbitrary code never becomes extension-world code.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Inline user-provided JavaScript.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Verified package-relative JavaScript path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// User-script world selection, separate from content-script worlds.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum World {
    /// Dedicated installation/worldId principal.
    #[default]
    UserScript,
    /// Explicit page-world execution, without extension authority.
    Main,
}

/// A normalized persistent registration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Script {
    /// Installation-local registration identifier.
    pub id: String,
    /// Included URL patterns.
    pub matches: Vec<String>,
    /// Ordered code/file sources.
    pub js: Vec<Source>,
    /// Excluded URL patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_matches: Vec<String>,
    /// Additional included glob patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_globs: Vec<String>,
    /// Excluded glob patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_globs: Vec<String>,
    /// Whether matching child frames are included.
    #[serde(default)]
    pub all_frames: bool,
    /// Preferred document lifecycle phase.
    #[serde(default)]
    pub run_at: InjectionTime,
    /// Execution principal.
    #[serde(default)]
    pub world: World,
    /// Dedicated user-world identifier; absent means the default world.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_id: Option<String>,
}

/// Explicit per-world overrides. The native fallback has no restrictive world CSP.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldConfiguration {
    /// Empty policy describes the native unrestricted world fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csp: Option<String>,
    /// User-world runtime messaging is disabled unless explicitly enabled.
    #[serde(default)]
    pub messaging: bool,
}

/// Immutable candidate state; adapters publish only after durable settlement.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    scripts: BTreeMap<String, Script>,
    worlds: BTreeMap<String, WorldConfiguration>,
}

fn invalid(message: &'static str) -> ApiError {
    ApiError::other(message)
}

/// Checks identifiers independently of JavaScript/schema validation.
pub fn world_id(value: Option<&str>, world: World) -> Result<String, ApiError> {
    if world == World::Main && value.is_some() {
        return Err(invalid("worldId is only available for USER_SCRIPT worlds."));
    }
    let value = value.unwrap_or("");
    UserScriptWorldId::parse(value).ok_or_else(|| invalid("Invalid user-script world ID."))?;
    Ok(value.to_owned())
}

/// Accepts only policies whose lack of additional restrictions is established.
pub fn validate_csp(csp: &str) -> Result<(), ApiError> {
    if csp.len() > 4096
        || !csp
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b';'))
    {
        return Err(invalid("This engine cannot enforce the requested user-script CSP. Only an empty policy is supported."));
    }
    Ok(())
}

/// Validates ordered sources before any candidate is adopted.
pub fn validate_sources(
    sources: &mut [Source],
    mut file: impl FnMut(&str) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if sources.is_empty() || sources.len() > 256 {
        return Err(invalid("A user script requires between 1 and 256 sources."));
    }
    for source in sources {
        match (&source.code, &mut source.file) {
            (Some(code), None) if code.len() <= MAX_STATE_BYTES && !code.contains('\0') => {}
            (None, Some(path)) => {
                *path = zephium_webext::manifest::normalize_resource(path)
                    .filter(|p| !p.is_empty() && !p.starts_with('_'))
                    .ok_or_else(|| invalid("Invalid user-script file."))?;
                file(path)?;
            }
            _ => {
                return Err(invalid(
                    "A source must contain exactly one bounded code or file value.",
                ))
            }
        }
    }
    Ok(())
}

impl Script {
    fn validate(&mut self, file: impl FnMut(&str) -> Result<(), ApiError>) -> Result<(), ApiError> {
        if self.id.is_empty()
            || self.id.len() > 256
            || self.id.starts_with('_')
            || self.id.chars().any(char::is_control)
        {
            return Err(invalid("Invalid user-script ID."));
        }
        let id = world_id(self.world_id.as_deref(), self.world)?;
        self.world_id = (!id.is_empty()).then_some(id);
        validate_sources(&mut self.js, file)?;
        self.matcher()?;
        Ok(())
    }

    /// User matches OR includeGlobs; the adapter separately intersects host grants.
    pub fn matcher(&self) -> Result<ContentScriptMatcher, ApiError> {
        ContentScriptMatcher::user_script(
            &self.matches,
            &self.exclude_matches,
            &self.include_globs,
            &self.exclude_globs,
        )
        .map_err(|_| invalid("Invalid user-script match pattern."))
    }
}

impl Registry {
    /// Current registrations in deterministic installation-local order.
    pub fn scripts(&self) -> impl Iterator<Item = &Script> {
        self.scripts.values()
    }
    /// The effective messaging setting; missing configuration is default-off.
    pub fn messaging(&self, world: &str) -> bool {
        self.worlds.get(world).is_some_and(|v| v.messaging)
    }
    /// Revalidates durable state instead of trusting serialized fields.
    pub fn restore(
        bytes: &[u8],
        mut file: impl FnMut(&str) -> Result<(), ApiError>,
    ) -> Result<Self, ApiError> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err(ApiError::too_many_pending());
        }
        let mut state: Self =
            serde_json::from_slice(bytes).map_err(|_| ApiError::malformed_arguments())?;
        for (id, script) in &mut state.scripts {
            if id != &script.id {
                return Err(ApiError::malformed_arguments());
            }
            script.validate(&mut file)?;
        }
        for (id, config) in &state.worlds {
            world_id(Some(id), World::UserScript)?;
            if let Some(csp) = &config.csp {
                validate_csp(csp)?;
            }
        }
        state.bytes()?;
        Ok(state)
    }
    /// Bounded durable snapshot; no partially accepted mutation escapes.
    pub fn bytes(&self) -> Result<Vec<u8>, ApiError> {
        let worlds: BTreeSet<_> = self
            .scripts
            .values()
            .filter(|s| s.world == World::UserScript)
            .map(|s| s.world_id.as_deref().unwrap_or(""))
            .chain(self.worlds.keys().map(String::as_str))
            .collect();
        if self.scripts.len() > MAX_SCRIPTS || worlds.len() > MAX_WORLDS {
            return Err(ApiError::too_many_pending());
        }
        let bytes = serde_json::to_vec(self).map_err(|_| ApiError::malformed_arguments())?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(ApiError::too_many_pending());
        }
        Ok(bytes)
    }
    /// Applies a generated-schema-validated operation to a private candidate.
    pub fn prepare(
        &self,
        name: &str,
        args: &[Value],
        mut file: impl FnMut(&str) -> Result<(), ApiError>,
    ) -> Result<(Value, Option<Self>), ApiError> {
        let mut candidate = self.clone();
        let first = args.first().unwrap_or(&Value::Null);
        let ids = || -> Result<Option<Vec<String>>, ApiError> {
            first
                .get("ids")
                .map(|v| {
                    serde_json::from_value::<Vec<String>>(v.clone())
                        .map_err(|_| ApiError::malformed_arguments())
                })
                .transpose()
        };
        match name {
            "userScripts.getScripts" => {
                let ids = ids()?;
                let scripts: Vec<_> = self
                    .scripts
                    .values()
                    .filter(|s| ids.as_ref().is_none_or(|ids| ids.contains(&s.id)))
                    .collect();
                return Ok((json!(scripts), None));
            }
            "userScripts.register" | "userScripts.update" => {
                let values = first.as_array().ok_or_else(ApiError::malformed_arguments)?;
                if values.len() > MAX_SCRIPTS {
                    return Err(ApiError::too_many_pending());
                }
                let mut seen = BTreeSet::new();
                for value in values {
                    let id = value["id"]
                        .as_str()
                        .ok_or_else(ApiError::malformed_arguments)?;
                    if !seen.insert(id) {
                        return Err(invalid("Duplicate user-script ID in batch."));
                    }
                    let value = if name == "userScripts.update" {
                        let old = candidate
                            .scripts
                            .get(id)
                            .ok_or_else(|| invalid("No registered user script with that ID."))?;
                        let mut merged = serde_json::to_value(old)
                            .map_err(|_| ApiError::malformed_arguments())?;
                        for (key, value) in value
                            .as_object()
                            .ok_or_else(ApiError::malformed_arguments)?
                        {
                            merged[key] = value.clone();
                        }
                        merged
                    } else {
                        if candidate.scripts.contains_key(id) {
                            return Err(invalid(
                                "A user script with that ID is already registered.",
                            ));
                        }
                        value.clone()
                    };
                    let mut script: Script = serde_json::from_value(value)
                        .map_err(|_| ApiError::malformed_arguments())?;
                    script.validate(&mut file)?;
                    candidate.scripts.insert(script.id.clone(), script);
                }
            }
            "userScripts.unregister" => {
                if let Some(ids) = ids()? {
                    if ids.len() > MAX_SCRIPTS {
                        return Err(ApiError::too_many_pending());
                    }
                    if ids.iter().any(|id| !candidate.scripts.contains_key(id)) {
                        return Err(invalid("No registered user script with that ID."));
                    }
                    for id in ids {
                        candidate.scripts.remove(&id);
                    }
                } else {
                    candidate.scripts.clear();
                }
            }
            "userScripts.configureWorld" => {
                let id = world_id(
                    first.get("worldId").and_then(Value::as_str),
                    World::UserScript,
                )?;
                let mut config = candidate.worlds.get(&id).cloned().unwrap_or_default();
                let previous = config.clone();
                if let Some(csp) = first.get("csp") {
                    let csp = csp.as_str().ok_or_else(ApiError::malformed_arguments)?;
                    validate_csp(csp)?;
                    config.csp = Some(csp.to_owned());
                }
                if let Some(messaging) = first.get("messaging") {
                    config.messaging = messaging
                        .as_bool()
                        .ok_or_else(ApiError::malformed_arguments)?;
                }
                if config == previous {
                    return Ok((Value::Null, None));
                }
                candidate.worlds.insert(id, config);
            }
            "userScripts.getWorldConfigurations" => {
                return Ok((
                    json!(self
                        .worlds
                        .iter()
                        .map(|(id, config)| {
                            let mut value = json!(config);
                            if !id.is_empty() {
                                value["worldId"] = json!(id);
                            }
                            value
                        })
                        .collect::<Vec<_>>()),
                    None,
                ));
            }
            "userScripts.resetWorldConfiguration" => {
                candidate
                    .worlds
                    .remove(&world_id(first.as_str(), World::UserScript)?);
            }
            _ => return Err(ApiError::unknown_method()),
        }
        candidate.bytes()?;
        Ok((Value::Null, Some(candidate)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepare(state: &Registry, name: &str, args: Value) -> Result<Registry, ApiError> {
        state
            .prepare(name, args.as_array().unwrap(), |_| Ok(()))
            .map(|(_, state)| state.unwrap())
    }
    #[test]
    fn batches_updates_restoration_and_refusals_are_atomic() {
        let initial = Registry::default();
        let registered = prepare(
            &initial,
            "userScripts.register",
            json!([[{"id":"a","matches":["https://example.com/*"],"js":[{"code":"let x=1"}]}]]),
        )
        .unwrap();
        assert_eq!(initial.scripts().count(), 0);
        assert!(prepare(&registered,"userScripts.register",json!([[{"id":"b","matches":["https://example.com/*"],"js":[{"code":"1"}]},{"id":"a","matches":[],"js":[]} ]])).is_err());
        assert_eq!(registered.scripts().count(), 1);
        let updated = prepare(
            &registered,
            "userScripts.update",
            json!([[{"id":"a","allFrames":true}]]),
        )
        .unwrap();
        assert!(updated.scripts().next().unwrap().all_frames);
        let restored = Registry::restore(&updated.bytes().unwrap(), |_| Ok(())).unwrap();
        assert_eq!(restored.scripts().next(), updated.scripts().next());
        assert!(prepare(
            &registered,
            "userScripts.update",
            json!([[{"id":"a","world":"MAIN","worldId":"other"}]])
        )
        .is_err());
        assert!(prepare(
            &registered,
            "userScripts.update",
            json!([[{"id":"a","js":[{"code":"1","file":"x.js"}]}]])
        )
        .is_err());
    }
    #[test]
    fn restrictive_policy_refuses_messaging_atomically_and_worlds_are_bounded() {
        let state = Registry::default();
        assert!(!state.messaging(""));
        for csp in [
            "script-src 'none'",
            "script-src 'self' 'unsafe-eval'",
            "script-src 'self' 'unsafe-inline' *",
        ] {
            assert!(prepare(
                &state,
                "userScripts.configureWorld",
                json!([{"csp":csp,"messaging":true}])
            )
            .is_err());
            assert!(!state.messaging(""));
        }
        let mut state = prepare(
            &state,
            "userScripts.configureWorld",
            json!([{"csp":" ; ","messaging":true}]),
        )
        .unwrap();
        assert!(state.messaging(""));
        for id in 1..MAX_WORLDS {
            state = prepare(
                &state,
                "userScripts.configureWorld",
                json!([{"worldId":id.to_string(),"messaging":true}]),
            )
            .unwrap();
        }
        assert!(prepare(
            &state,
            "userScripts.configureWorld",
            json!([{"worldId":"overflow","messaging":true}])
        )
        .is_err());
        assert!(
            !prepare(&state, "userScripts.resetWorldConfiguration", json!([]))
                .unwrap()
                .messaging("")
        );
    }
}
