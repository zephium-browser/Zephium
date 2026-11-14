//! Atomic content-script registration and adapter-owned injection plans.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zephium_webext::manifest::{normalize_resource, ContentScript, Manifest, RunAt, World};

use crate::dispatch::{ApiTable, RegistrationError};
use crate::pattern::{ContentScriptMatcher, FrameDocument, MatchOptions};
use crate::{ApiError, Sender};

/// Maximum static plus dynamic registrations per install.
pub const MAX_REGISTERED_SCRIPTS: usize = 1_024;
/// Maximum serialized metadata for one install.
pub const MAX_REGISTRY_BYTES: usize = 8 * 1024 * 1024;
/// Maximum package files referenced by one script.
pub const MAX_FILES_PER_SCRIPT: usize = 256;

/// JavaScript execution world.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionWorld {
    /// The extension's isolated world.
    #[default]
    Isolated,
    /// The page's world, without extension APIs.
    Main,
}

/// JavaScript execution timing. CSS always precedes document construction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionTime {
    /// Before document scripts.
    DocumentStart,
    /// After document construction.
    DocumentEnd,
    /// After document end when the adapter's idle wrapper runs.
    #[default]
    DocumentIdle,
}

fn yes() -> bool {
    true
}

/// A normalized dynamic registration, also used for persisted snapshots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisteredScript {
    /// Install-local identifier; leading underscores are reserved.
    pub id: String,
    /// Included Chrome match patterns.
    pub matches: Vec<String>,
    /// Excluded Chrome match patterns.
    #[serde(default)]
    pub exclude_matches: Vec<String>,
    /// JavaScript package resources, in execution order.
    #[serde(default)]
    pub js: Vec<String>,
    /// CSS package resources, in application order.
    #[serde(default)]
    pub css: Vec<String>,
    /// Run in matching subframes as well as the top frame.
    #[serde(default)]
    pub all_frames: bool,
    /// Match related opaque documents by their authenticated precursor origin.
    #[serde(default)]
    pub match_origin_as_fallback: bool,
    /// Keep this registration across browser sessions.
    #[serde(default = "yes")]
    pub persist_across_sessions: bool,
    /// JavaScript timing.
    #[serde(default)]
    pub run_at: InjectionTime,
    /// JavaScript world.
    #[serde(default)]
    pub world: ExecutionWorld,
}

/// Package resource kind the adapter must validate before publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    /// JavaScript: check existence, readability and syntax.
    JavaScript,
    /// CSS: check existence and readability.
    Css,
}

#[derive(Clone, Debug)]
struct Entry {
    script: RegisteredScript,
    matcher: ContentScriptMatcher,
    static_glob_bytes: usize,
}

/// One static or dynamic bundle the adapter applies to future documents.
#[derive(Clone, Copy, Debug)]
pub struct Injection<'a> {
    /// Bundle metadata; static identifiers use a reserved underscore prefix.
    pub script: &'a RegisteredScript,
    /// The complete compiled declaration, retaining static globs and related-frame options.
    pub matcher: &'a ContentScriptMatcher,
}

/// One install's registry. Clone to stage a mutation before durable publication.
#[derive(Clone, Debug, Default)]
pub struct ScriptRegistry {
    static_scripts: Vec<Entry>,
    dynamic: BTreeMap<String, Entry>,
}

impl ScriptRegistry {
    /// Builds static scripts, validating every referenced resource through `check`.
    pub fn from_manifest(
        manifest: &Manifest,
        mut check: impl FnMut(&str, ResourceKind) -> Result<(), ApiError>,
    ) -> Result<Self, ApiError> {
        let scripts = manifest.content_scripts();
        if scripts.len() > MAX_REGISTERED_SCRIPTS {
            return Err(ApiError::too_many_pending());
        }
        let mut registry = Self::default();
        for (index, script) in scripts.into_iter().enumerate() {
            let static_glob_bytes =
                serde_json::to_vec(&(&script.include_globs, &script.exclude_globs))
                    .map_err(|_| ApiError::malformed_arguments())?
                    .len();
            let matcher = matcher(&script)?;
            let registered = from_static(index, script);
            validate_files(&registered, &mut check)?;
            registry.static_scripts.push(Entry {
                script: registered,
                matcher,
                static_glob_bytes,
            });
            registry.check_budget()?;
        }
        registry.check_budget()?;
        Ok(registry)
    }

    /// Registers a batch atomically. No changes survive a rejected member.
    pub fn register(
        &mut self,
        scripts: Vec<RegisteredScript>,
        mut check: impl FnMut(&str, ResourceKind) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        if self
            .dynamic
            .len()
            .saturating_add(self.static_scripts.len())
            .saturating_add(scripts.len())
            > MAX_REGISTERED_SCRIPTS
        {
            return Err(ApiError::too_many_pending());
        }
        let mut candidate = self.clone();
        for script in scripts {
            if candidate.dynamic.contains_key(&script.id) {
                return Err(ApiError::other(format!(
                    "Duplicate script ID '{}'",
                    script.id
                )));
            }
            let entry = prepare(script, &mut check)?;
            candidate.dynamic.insert(entry.script.id.clone(), entry);
        }
        candidate.check_budget()?;
        *self = candidate;
        Ok(())
    }

    /// Updates only supplied properties, with all-or-nothing validation.
    pub fn update(
        &mut self,
        patches: &[Value],
        mut check: impl FnMut(&str, ResourceKind) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        if patches.len() > MAX_REGISTERED_SCRIPTS {
            return Err(ApiError::too_many_pending());
        }
        let mut candidate = self.clone();
        let mut seen = BTreeSet::new();
        for patch in patches {
            let patch = patch
                .as_object()
                .ok_or_else(ApiError::malformed_arguments)?;
            let id = patch
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(ApiError::malformed_arguments)?;
            if !seen.insert(id) {
                return Err(ApiError::other(format!("Duplicate script ID '{id}'")));
            }
            let old = self.dynamic.get(id).ok_or_else(|| missing_script(id))?;
            let mut merged =
                serde_json::to_value(&old.script).map_err(|_| ApiError::malformed_arguments())?;
            let object = merged
                .as_object_mut()
                .ok_or_else(ApiError::malformed_arguments)?;
            object.extend(patch.clone());
            let script =
                serde_json::from_value(merged).map_err(|_| ApiError::malformed_arguments())?;
            let entry = prepare(script, &mut check)?;
            candidate.dynamic.insert(id.to_owned(), entry);
        }
        candidate.check_budget()?;
        *self = candidate;
        Ok(())
    }

    /// Removes selected dynamic scripts, or all when no IDs are supplied.
    pub fn unregister(&mut self, ids: Option<&[String]>) -> Result<(), ApiError> {
        if let Some(ids) = ids {
            if ids.len() > MAX_REGISTERED_SCRIPTS {
                return Err(ApiError::too_many_pending());
            }
            if let Some(id) = ids.iter().find(|id| !self.dynamic.contains_key(*id)) {
                return Err(missing_script(id));
            }
            for id in ids {
                self.dynamic.remove(id);
            }
        } else {
            self.dynamic.clear();
        }
        Ok(())
    }

    /// Dynamic registrations matching an optional ID filter, in stable ID order.
    pub fn get_registered(&self, ids: Option<&[String]>) -> Vec<RegisteredScript> {
        self.dynamic
            .values()
            .filter(|entry| ids.is_none_or(|ids| ids.contains(&entry.script.id)))
            .map(|entry| entry.script.clone())
            .collect()
    }

    /// Persist this snapshot in the install's store before acknowledging mutation.
    pub fn snapshot(&self) -> Vec<RegisteredScript> {
        self.dynamic
            .values()
            .filter(|entry| entry.script.persist_across_sessions)
            .map(|entry| entry.script.clone())
            .collect()
    }

    /// Replaces dynamic state from a validated durable snapshot, atomically.
    pub fn restore(
        &mut self,
        scripts: Vec<RegisteredScript>,
        check: impl FnMut(&str, ResourceKind) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        if scripts.iter().any(|script| !script.persist_across_sessions) {
            return Err(ApiError::malformed_arguments());
        }
        let mut candidate = self.clone();
        candidate.dynamic.clear();
        candidate.register(scripts, check)?;
        *self = candidate;
        Ok(())
    }

    /// All compiled declarations in static manifest order, then dynamic ID order.
    /// Adapters may publish these for future documents only after applying host
    /// permissions and view admission; this iterator itself grants no authority.
    pub fn declarations(&self) -> impl Iterator<Item = Injection<'_>> {
        self.static_scripts
            .iter()
            .chain(self.dynamic.values())
            .map(|entry| Injection {
                script: &entry.script,
                matcher: &entry.matcher,
            })
    }

    /// Produces a plan only after the adapter verifies current host/site access.
    /// `authorized` must include ordinary-tab ownership and current generation;
    /// Work views, interface views and revoked hosts must pass false.
    pub fn injection_plan<'a>(
        &'a self,
        document: &FrameDocument<'_>,
        top_frame: bool,
        file_access: bool,
        authorized: bool,
    ) -> Vec<Injection<'a>> {
        if !authorized {
            return Vec::new();
        }
        self.static_scripts
            .iter()
            .chain(self.dynamic.values())
            .filter(|entry| {
                (top_frame || entry.script.all_frames)
                    && entry.matcher.matches(document, file_access)
            })
            .map(|entry| Injection {
                script: &entry.script,
                matcher: &entry.matcher,
            })
            .collect()
    }

    fn check_budget(&self) -> Result<(), ApiError> {
        let mut bytes = 0_usize;
        for entry in self.static_scripts.iter().chain(self.dynamic.values()) {
            bytes = bytes.saturating_add(entry.static_glob_bytes);
            bytes = bytes.saturating_add(
                serde_json::to_vec(&entry.script)
                    .map_err(|_| ApiError::malformed_arguments())?
                    .len(),
            );
            if bytes > MAX_REGISTRY_BYTES {
                return Err(ApiError::too_many_pending());
            }
        }
        Ok(())
    }
}

fn missing_script(id: &str) -> ApiError {
    if id.len() > 256 {
        return ApiError::malformed_arguments();
    }
    ApiError::other(format!(
        "Script with ID '{id}' does not exist or is not fully registered"
    ))
}

fn matcher(script: &ContentScript) -> Result<ContentScriptMatcher, ApiError> {
    if script.matches.is_empty() {
        return Err(ApiError::other("Content script requires matches."));
    }
    ContentScriptMatcher::new(
        &script.matches,
        &script.exclude_matches,
        &script.include_globs,
        &script.exclude_globs,
        MatchOptions {
            match_about_blank: script.match_about_blank,
            match_origin_as_fallback: script.match_origin_as_fallback,
        },
    )
    .map_err(|_| ApiError::other("Invalid content script match pattern or glob."))
}

fn from_static(index: usize, script: ContentScript) -> RegisteredScript {
    RegisteredScript {
        id: format!("_manifest_{index}"),
        matches: script.matches,
        exclude_matches: script.exclude_matches,
        js: script.js,
        css: script.css,
        all_frames: script.all_frames,
        match_origin_as_fallback: script.match_origin_as_fallback,
        persist_across_sessions: true,
        run_at: match script.run_at {
            RunAt::DocumentStart => InjectionTime::DocumentStart,
            RunAt::DocumentEnd => InjectionTime::DocumentEnd,
            RunAt::DocumentIdle => InjectionTime::DocumentIdle,
        },
        world: match script.world {
            World::Isolated => ExecutionWorld::Isolated,
            World::Main => ExecutionWorld::Main,
        },
    }
}

fn validate_files(
    script: &RegisteredScript,
    check: &mut impl FnMut(&str, ResourceKind) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if script.js.is_empty() && script.css.is_empty() {
        return Err(ApiError::other(
            "Content script requires JavaScript or CSS files.",
        ));
    }
    if script.js.len().saturating_add(script.css.len()) > MAX_FILES_PER_SCRIPT {
        return Err(ApiError::too_many_pending());
    }
    for (paths, kind) in [
        (&script.js, ResourceKind::JavaScript),
        (&script.css, ResourceKind::Css),
    ] {
        for path in paths {
            if path.len() > 4096
                || normalize_resource(path).as_deref() != Some(path)
                || path.contains(['?', '#'])
                || path.starts_with("_zephium/")
            {
                return Err(ApiError::other("Invalid content script resource path."));
            }
            check(path, kind)?;
        }
    }
    Ok(())
}

fn prepare(
    mut script: RegisteredScript,
    check: &mut impl FnMut(&str, ResourceKind) -> Result<(), ApiError>,
) -> Result<Entry, ApiError> {
    if script.id.is_empty()
        || script.id.starts_with('_')
        || script.id.len() > 256
        || script.id.chars().any(char::is_control)
    {
        return Err(ApiError::other("Invalid content script ID."));
    }
    for path in script.js.iter_mut().chain(&mut script.css) {
        *path = normalize_resource(path)
            .ok_or_else(|| ApiError::other("Invalid content script resource path."))?;
    }
    validate_files(&script, check)?;
    let matcher = matcher(&ContentScript {
        matches: script.matches.clone(),
        exclude_matches: script.exclude_matches.clone(),
        match_origin_as_fallback: script.match_origin_as_fallback,
        ..ContentScript::default()
    })?;
    Ok(Entry {
        script,
        matcher,
        static_glob_bytes: 0,
    })
}

/// Adapter access to the authenticated install's registry and package.
pub trait ScriptingHost {
    /// Returns the registry belonging to this sender's profile and install.
    fn script_registry(&self, sender: &Sender) -> Result<&ScriptRegistry, ApiError>;
    /// Validates a package file, including JavaScript syntax, before registration.
    fn validate_script_resource(
        &self,
        sender: &Sender,
        path: &str,
        kind: ResourceKind,
    ) -> Result<(), ApiError>;
    /// Atomically persists `snapshot()`, publishes to future documents, and adopts
    /// the candidate. On failure the old registry and publication must survive.
    fn commit_script_registry(
        &mut self,
        sender: &Sender,
        candidate: ScriptRegistry,
    ) -> Result<(), ApiError>;
}

fn filter(args: &[Value]) -> Result<Option<Vec<String>>, ApiError> {
    let Some(object) = args.first().filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    object
        .get("ids")
        .map(|ids| {
            let ids: Vec<String> =
                serde_json::from_value(ids.clone()).map_err(|_| ApiError::malformed_arguments())?;
            if ids.len() > MAX_REGISTERED_SCRIPTS {
                return Err(ApiError::too_many_pending());
            }
            Ok(ids)
        })
        .transpose()
}

/// Registers the four dynamic content-script methods.
pub fn register<S: ScriptingHost + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in [
        "scripting.registerContentScripts",
        "scripting.updateContentScripts",
        "scripting.unregisterContentScripts",
        "scripting.getRegisteredContentScripts",
    ] {
        table.register_generated(name, |host: &mut S, invocation| {
            let result = (|| {
                let sender = invocation.sender();
                let mut candidate = host.script_registry(sender)?.clone();
                let args = invocation.args();
                match invocation.name() {
                    "scripting.registerContentScripts" => candidate.register(
                        serde_json::from_value(args[0].clone())
                            .map_err(|_| ApiError::malformed_arguments())?,
                        |path, kind| host.validate_script_resource(sender, path, kind),
                    )?,
                    "scripting.updateContentScripts" => candidate.update(
                        args[0]
                            .as_array()
                            .ok_or_else(ApiError::malformed_arguments)?,
                        |path, kind| host.validate_script_resource(sender, path, kind),
                    )?,
                    "scripting.unregisterContentScripts" => {
                        candidate.unregister(filter(args)?.as_deref())?
                    }
                    _ => {
                        let mut scripts = serde_json::to_value(
                            candidate.get_registered(filter(args)?.as_deref()),
                        )
                        .map_err(|_| ApiError::malformed_arguments())?;
                        for script in scripts.as_array_mut().into_iter().flatten() {
                            let object = script
                                .as_object_mut()
                                .ok_or_else(ApiError::malformed_arguments)?;
                            for name in ["css", "excludeMatches", "js"] {
                                if object
                                    .get(name)
                                    .and_then(Value::as_array)
                                    .is_some_and(Vec::is_empty)
                                {
                                    object.remove(name);
                                }
                            }
                        }
                        return Ok(scripts);
                    }
                }
                host.commit_script_registry(sender, candidate)?;
                Ok(Value::Null)
            })();
            invocation.reply(result)
        })?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "scripting_registry/tests.rs"]
mod dispatcher_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::DocumentOrigin;
    use serde_json::json;
    use url::Url;

    fn script(id: &str) -> RegisteredScript {
        serde_json::from_value(
            json!({"id":id,"matches":["https://example.com/*"],"js":["content.js"]}),
        )
        .unwrap()
    }
    fn accept(_: &str, _: ResourceKind) -> Result<(), ApiError> {
        Ok(())
    }

    #[test]
    fn batches_are_atomic_and_updates_preserve_unspecified_properties() {
        let mut registry = ScriptRegistry::default();
        registry.register(vec![script("a")], accept).unwrap();
        assert!(registry
            .register(vec![script("b"), script("a")], accept)
            .is_err());
        assert_eq!(registry.get_registered(None).len(), 1);
        assert!(registry
            .update(
                &[json!({"id":"a","world":"MAIN"}), json!({"id":"missing"})],
                accept
            )
            .is_err());
        assert_eq!(
            registry.get_registered(None)[0].world,
            ExecutionWorld::Isolated
        );
        registry
            .update(&[json!({"id":"a","world":"MAIN","allFrames":true})], accept)
            .unwrap();
        let entry = &registry.get_registered(None)[0];
        assert_eq!(entry.js, ["content.js"]);
        assert_eq!(entry.world, ExecutionWorld::Main);
        assert!(registry
            .unregister(Some(&["a".into(), "missing".into()]))
            .is_err());
        assert_eq!(registry.get_registered(None).len(), 1);
    }

    #[test]
    fn persistence_file_failures_and_malformed_inputs() {
        let mut registry = ScriptRegistry::default();
        let mut temporary = script("temporary");
        temporary.persist_across_sessions = false;
        registry
            .register(vec![script("a"), temporary], accept)
            .unwrap();
        assert_eq!(registry.snapshot().len(), 1);
        let mut restarted = ScriptRegistry::default();
        restarted.restore(registry.snapshot(), accept).unwrap();
        assert_eq!(restarted.get_registered(None).len(), 1);
        assert!(registry
            .register(vec![script("bad")], |_, _| Err(ApiError::other(
                "File missing."
            )))
            .is_err());
        for value in [
            json!({"id":"_bad","matches":["<all_urls>"],"js":["x.js"]}),
            json!({"id":"bad","matches":[],"js":["x.js"]}),
            json!({"id":"bad","matches":["https://example.com/path"],"matchOriginAsFallback":true,"css":["x.css"]}),
            json!({"id":"bad","matches":["<all_urls>"],"js":["../x.js"]}),
        ] {
            assert!(registry
                .register(vec![serde_json::from_value(value).unwrap()], accept)
                .is_err());
        }
        assert_eq!(registry.get_registered(None).len(), 2);
    }

    #[test]
    fn static_globs_related_frames_css_and_authorization() {
        let manifest = Manifest::from_value(
            json!({"content_scripts":[{"matches":["https://example.com/*"],"include_globs":["*/news/*"],"exclude_globs":["*private*"],"js":["x.js"],"css":["x.css"],"all_frames":true,"match_about_blank":true}]}),
        );
        let registry = ScriptRegistry::from_manifest(&manifest, accept).unwrap();
        let declarations: Vec<_> = registry.declarations().collect();
        assert_eq!(declarations.len(), 1);
        assert_eq!(
            declarations[0].matcher.include_globs()[0].as_str(),
            "*/news/*"
        );
        assert_eq!(
            declarations[0].matcher.exclude_globs()[0].as_str(),
            "*private*"
        );
        assert!(
            declarations[0]
                .matcher
                .match_set()
                .options()
                .match_about_blank
        );
        let creator = Url::parse("https://example.com/news/story").unwrap();
        let origin = DocumentOrigin::of_url(&creator);
        let url = Url::parse("about:blank").unwrap();
        let document = FrameDocument {
            url: &url,
            origin: &origin,
            creator_url: Some(&creator),
        };
        assert_eq!(
            registry.injection_plan(&document, false, false, true).len(),
            1
        );
        assert!(registry
            .injection_plan(&document, true, false, false)
            .is_empty());
        let private = Url::parse("https://example.com/news/private").unwrap();
        let doc = FrameDocument {
            url: &private,
            origin: &origin,
            creator_url: None,
        };
        assert!(registry.injection_plan(&doc, true, false, true).is_empty());
        assert!(registry.get_registered(None).is_empty());
    }
}
