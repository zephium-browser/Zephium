//! Response policy for extension package and runtime resources.

use serde_json::Value;
use thiserror::Error;
use zephium_webext::manifest::{normalize_resource, Manifest};

use crate::package::ResourcePath;

/// An independent policy on every response, including sandbox pages.
pub const MINIMUM_CSP: &str = "script-src 'self' 'wasm-unsafe-eval'; object-src 'self'";

/// Chrome's default manifest policy does not enable WebAssembly.
pub const DEFAULT_EXTENSION_CSP: &str = "script-src 'self'; object-src 'self';";

/// Chrome's default opaque-origin sandbox policy.
pub const DEFAULT_SANDBOX_CSP: &str = "sandbox allow-scripts allow-forms allow-popups allow-modals; script-src 'self' 'unsafe-inline' 'unsafe-eval'; child-src 'self';";

const MAX_POLICY_BYTES: usize = 64 * 1024;
const MAX_SANDBOX_PAGES: usize = 1024;

/// A manifest cannot be served safely with the requested response policy.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("only manifest version 3 is supported")]
    UnsupportedManifestVersion,
    #[error("invalid manifest field {0}")]
    InvalidManifestField(&'static str),
    #[error("response policy exceeds its resource limit")]
    ResourceLimit,
    #[error("extension page CSP allows an insecure executable source")]
    InsecureExecutableSource,
    #[error("sandbox CSP must enforce an opaque origin")]
    InvalidSandboxPolicy,
    #[error("invalid sandbox page path")]
    InvalidSandboxPage,
}

/// Prepared policies; the caller still owns resource admission and MIME types.
#[derive(Debug, Clone)]
pub struct ResponsePolicy {
    extension: String,
    sandbox: String,
    sandbox_pages: Vec<String>,
    embedder: Option<String>,
    opener: Option<String>,
}

impl ResponsePolicy {
    /// Validates manifest policy without trusting package preparation to do so.
    pub fn from_manifest(manifest: &Manifest) -> Result<Self, PolicyError> {
        if manifest.manifest_version() != 3 {
            return Err(PolicyError::UnsupportedManifestVersion);
        }
        let raw = manifest.raw();
        let csp = raw.get("content_security_policy");
        if csp.is_some_and(|value| !value.is_object()) {
            return Err(PolicyError::InvalidManifestField("content_security_policy"));
        }
        let extension = policy_value(
            csp.and_then(|value| value.get("extension_pages")),
            DEFAULT_EXTENSION_CSP,
            "content_security_policy.extension_pages",
        )?;
        validate_extension(&extension)?;
        let sandbox = policy_value(
            csp.and_then(|value| value.get("sandbox")),
            DEFAULT_SANDBOX_CSP,
            "content_security_policy.sandbox",
        )?;
        validate_sandbox(&sandbox)?;

        let mut sandbox_pages = Vec::new();
        if let Some(sandbox) = raw.get("sandbox") {
            let pages = sandbox
                .as_object()
                .and_then(|sandbox| sandbox.get("pages"))
                .and_then(Value::as_array)
                .ok_or(PolicyError::InvalidManifestField("sandbox.pages"))?;
            if pages.len() > MAX_SANDBOX_PAGES {
                return Err(PolicyError::ResourceLimit);
            }
            for page in pages {
                let page = page.as_str().ok_or(PolicyError::InvalidSandboxPage)?;
                if page.len() > crate::package::MAX_URI_LENGTH {
                    return Err(PolicyError::ResourceLimit);
                }
                let normalized = normalize_resource(page).ok_or(PolicyError::InvalidSandboxPage)?;
                let path = normalized.split(['?', '#']).next().unwrap_or(&normalized);
                let path = ResourcePath::parse(&format!("/{path}"))
                    .map_err(|_| PolicyError::InvalidSandboxPage)?;
                sandbox_pages.push(path.as_str().to_ascii_lowercase());
            }
        }

        Ok(Self {
            extension,
            sandbox,
            sandbox_pages,
            embedder: isolation_value(raw, "cross_origin_embedder_policy")?,
            opener: isolation_value(raw, "cross_origin_opener_policy")?,
        })
    }

    /// Whether a canonical decoded relative package path has sandbox policy.
    /// Chrome compares sandbox path patterns without ASCII case sensitivity.
    /// Host-generated `_zephium` resources are outside package declarations.
    pub fn is_sandboxed(&self, path: &str) -> bool {
        if path == "_zephium" || path.starts_with("_zephium/") {
            return false;
        }
        let path = path.to_ascii_lowercase();
        self.sandbox_pages
            .iter()
            .any(|pattern| star_matches(pattern, &path))
    }

    /// Headers to append, retaining both independent CSP header fields.
    /// `web_accessible` must come from host authorization for this request.
    pub fn headers_for(&self, path: &str, web_accessible: bool) -> Vec<(&'static str, &str)> {
        let policy = if self.is_sandboxed(path) {
            &self.sandbox
        } else {
            &self.extension
        };
        // Separate headers intersect; appending text to one policy would not.
        let mut headers = vec![
            ("Content-Security-Policy", policy.as_str()),
            ("Content-Security-Policy", MINIMUM_CSP),
            ("X-Content-Type-Options", "nosniff"),
        ];
        if let Some(embedder) = &self.embedder {
            headers.push(("Cross-Origin-Embedder-Policy", embedder));
        }
        if let Some(opener) = &self.opener {
            headers.push(("Cross-Origin-Opener-Policy", opener));
        }
        if web_accessible {
            headers.push(("Access-Control-Allow-Origin", "*"));
            headers.push(("Cross-Origin-Resource-Policy", "cross-origin"));
        }
        headers
    }
}

fn policy_value(
    value: Option<&Value>,
    default: &str,
    field: &'static str,
) -> Result<String, PolicyError> {
    let policy = match value {
        Some(value) => value
            .as_str()
            .ok_or(PolicyError::InvalidManifestField(field))?,
        None => default,
    };
    if policy.len() > MAX_POLICY_BYTES {
        return Err(PolicyError::ResourceLimit);
    }
    // Chromium excludes policy lists and non-ASCII parser differentials.
    if !policy
        .bytes()
        .all(|byte| matches!(byte, b'\t' | 0x0c) || ((b' '..=b'~').contains(&byte) && byte != b','))
    {
        return Err(PolicyError::InvalidManifestField(field));
    }
    Ok(policy.replace('\u{c}', " "))
}

fn isolation_value(raw: &Value, field: &'static str) -> Result<Option<String>, PolicyError> {
    let Some(value) = raw.get(field) else {
        return Ok(None);
    };
    let value = value
        .as_object()
        .ok_or(PolicyError::InvalidManifestField(field))?;
    let Some(value) = value.get("value") else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or(PolicyError::InvalidManifestField(field))?;
    if value.len() > MAX_POLICY_BYTES {
        return Err(PolicyError::ResourceLimit);
    }
    if value.is_empty() || !value.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        return Err(PolicyError::InvalidManifestField(field));
    }
    Ok(Some(value.to_owned()))
}

fn directive<'a>(policy: &'a str, name: &str) -> Option<Vec<&'a str>> {
    policy.split(';').find_map(|part| {
        let mut words = part.split_ascii_whitespace();
        words
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case(name))
            .then(|| words.collect())
    })
}

fn validate_extension(policy: &str) -> Result<(), PolicyError> {
    let script = directive(policy, "script-src").or_else(|| directive(policy, "default-src"));
    let script = script.ok_or(PolicyError::InsecureExecutableSource)?;
    let permitted = |source: &&str| {
        ["'self'", "'none'", "'wasm-unsafe-eval'"]
            .iter()
            .any(|allowed| source.eq_ignore_ascii_case(allowed))
    };
    if !script.iter().all(permitted) {
        return Err(PolicyError::InsecureExecutableSource);
    }
    for name in [
        "script-src-elem",
        "script-src-attr",
        "worker-src",
        "object-src",
    ] {
        if directive(policy, name).is_some_and(|sources| !sources.iter().all(permitted)) {
            return Err(PolicyError::InsecureExecutableSource);
        }
    }
    Ok(())
}

fn validate_sandbox(policy: &str) -> Result<(), PolicyError> {
    if directive(policy, "sandbox").is_none() {
        return Err(PolicyError::InvalidSandboxPolicy);
    }
    for part in policy.split(';') {
        let mut words = part.split_ascii_whitespace();
        if words
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("sandbox"))
            && words.any(|word| word.eq_ignore_ascii_case("allow-same-origin"))
        {
            return Err(PolicyError::InvalidSandboxPolicy);
        }
    }
    Ok(())
}

fn star_matches(pattern: &str, path: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let mut parts = parts.peekable();
    if parts.peek().is_none() {
        return rest.is_empty();
    }
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return rest.ends_with(part);
        }
        let Some(index) = rest.find(part) else {
            return false;
        };
        rest = &rest[index + part.len()..];
    }
    true
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    fn manifest(value: Value) -> Manifest {
        let mut raw = json!({"manifest_version": 3});
        raw.as_object_mut()
            .unwrap()
            .extend(value.as_object().unwrap().clone());
        Manifest::from_value(raw)
    }

    fn policies<'a>(headers: &[(&str, &'a str)]) -> Vec<&'a str> {
        headers
            .iter()
            .filter_map(|(name, value)| (*name == "Content-Security-Policy").then_some(*value))
            .collect()
    }

    #[test]
    fn defaults_keep_independent_floor_and_nosniff() {
        let policy = ResponsePolicy::from_manifest(&manifest(json!({}))).unwrap();
        let headers = policy.headers_for("popup.html", false);
        assert_eq!(policies(&headers), [DEFAULT_EXTENSION_CSP, MINIMUM_CSP]);
        assert!(headers.contains(&("X-Content-Type-Options", "nosniff")));
        assert!(!headers
            .iter()
            .any(|(name, _)| name.starts_with("Cross-Origin-")));
        assert!(!headers
            .iter()
            .any(|(name, _)| *name == "Access-Control-Allow-Origin"));
    }

    #[test]
    fn declared_policy_is_not_rewritten_or_merged_with_floor() {
        let supplied = "default-src https:; script-src 'self' 'wasm-unsafe-eval'; object-src 'none'; img-src data: https:";
        let policy = ResponsePolicy::from_manifest(&manifest(json!({
            "content_security_policy": {"extension_pages": supplied}
        })))
        .unwrap();
        assert_eq!(
            policies(&policy.headers_for("worker.js", false)),
            [supplied, MINIMUM_CSP]
        );
    }

    #[test]
    fn sandbox_uses_opaque_policy_and_case_insensitive_path_patterns() {
        let supplied = "sandbox allow-scripts; script-src https: 'unsafe-eval'";
        let policy = ResponsePolicy::from_manifest(&manifest(json!({
            "sandbox": {"pages": ["/./sandbox.html?mode=test", "tools/*.html", "space%20name.html"]},
            "content_security_policy": {"sandbox": supplied}
        })))
        .unwrap();
        for path in [
            "sandbox.html",
            "SANDBOX.html",
            "tools/nested/a.html",
            "space name.html",
        ] {
            assert!(policy.is_sandboxed(path), "{path}");
            assert_eq!(
                policies(&policy.headers_for(path, false)),
                [supplied, MINIMUM_CSP]
            );
        }
        for path in ["sandbox.html.js", "tools/a.html.js", "other.html"] {
            assert!(!policy.is_sandboxed(path), "{path}");
        }
    }

    #[test]
    fn default_sandbox_keeps_floor_despite_its_inline_and_eval_tokens() {
        let policy = ResponsePolicy::from_manifest(&manifest(json!({
            "sandbox": {"pages": ["sandbox.html"]}
        })))
        .unwrap();
        assert_eq!(
            policies(&policy.headers_for("sandbox.html", true)),
            [DEFAULT_SANDBOX_CSP, MINIMUM_CSP]
        );
    }

    #[test]
    fn package_sandbox_wildcards_cannot_sandbox_host_generated_runtime() {
        let policy = ResponsePolicy::from_manifest(&manifest(json!({
            "sandbox": {"pages": ["*"]}
        })))
        .unwrap();
        for path in [
            "_zephium",
            "_zephium/background.html",
            "_zephium/bootstrap/owner.js",
        ] {
            assert!(!policy.is_sandboxed(path), "{path}");
            assert_eq!(
                policies(&policy.headers_for(path, false)),
                [DEFAULT_EXTENSION_CSP, MINIMUM_CSP]
            );
        }
        for path in [
            "popup.html",
            "_zephium-other/page.html",
            "_ZEPHIUM/package.html",
        ] {
            assert!(policy.is_sandboxed(path), "{path}");
        }
    }

    #[test]
    fn sandbox_cannot_keep_extension_origin_or_omit_sandbox() {
        for supplied in [
            "script-src 'self'",
            "sandbox allow-scripts allow-same-origin",
            "SaNdBoX ALLOW-SAME-ORIGIN",
            "sandbox; sandbox allow-same-origin",
        ] {
            assert_eq!(
                ResponsePolicy::from_manifest(&manifest(json!({
                    "content_security_policy": {"sandbox": supplied}
                })))
                .unwrap_err(),
                PolicyError::InvalidSandboxPolicy
            );
        }
    }

    #[test]
    fn rejects_header_injection_policy_lists_and_invalid_manifest_types() {
        for supplied in [
            "script-src 'self'\r\nX-Injected: yes",
            "script-src 'self', script-src *",
            "script-src 'self'\0",
            "script-src 'self'\u{7f}",
            "script-src 'self' é",
        ] {
            assert!(ResponsePolicy::from_manifest(&manifest(json!({
                "content_security_policy": {"extension_pages": supplied}
            })))
            .is_err());
        }
        for supplied in [
            json!({"manifest_version": 2}),
            json!({"content_security_policy": null}),
            json!({"content_security_policy": "script-src 'self'"}),
            json!({"content_security_policy": {"extension_pages": 12}}),
            json!({"content_security_policy": {"sandbox": false}}),
            json!({"sandbox": {"pages": [12]}}),
            json!({"sandbox": {"pages": ["../sandbox.html"]}}),
            json!({"sandbox": {"pages": ["%2e%2e/sandbox.html"]}}),
        ] {
            assert!(ResponsePolicy::from_manifest(&manifest(supplied)).is_err());
        }
    }

    #[test]
    fn rejects_remote_and_eval_executable_sources_including_fallbacks() {
        for supplied in [
            "script-src 'self' 'unsafe-eval'",
            "script-src https://remote.example",
            "script-src 'self'; script-src-elem https:",
            "script-src 'self'; script-src-attr 'unsafe-inline'",
            "script-src 'self'; worker-src blob:",
            "script-src 'self'; object-src *",
            "default-src https:",
            "style-src 'self'",
        ] {
            assert_eq!(
                ResponsePolicy::from_manifest(&manifest(json!({
                    "content_security_policy": {"extension_pages": supplied}
                })))
                .unwrap_err(),
                PolicyError::InsecureExecutableSource
            );
        }
        for supplied in [
            "script-src",
            "default-src 'none'",
            "SCRIPT-SRC\t'SELF'; script-src https:",
        ] {
            assert!(ResponsePolicy::from_manifest(&manifest(json!({
                "content_security_policy": {"extension_pages": supplied}
            })))
            .is_ok());
        }
    }

    #[test]
    fn cross_origin_headers_follow_explicit_manifest_and_host_war_admission() {
        let policy = ResponsePolicy::from_manifest(&manifest(json!({
            "cross_origin_embedder_policy": {"value": "require-corp"},
            "cross_origin_opener_policy": {"value": "same-origin"}
        })))
        .unwrap();
        for public in [false, true] {
            let headers = policy.headers_for("resource.js", public);
            assert!(headers.contains(&("Cross-Origin-Embedder-Policy", "require-corp")));
            assert!(headers.contains(&("Cross-Origin-Opener-Policy", "same-origin")));
            assert_eq!(
                headers.contains(&("Cross-Origin-Resource-Policy", "cross-origin")),
                public
            );
            assert_eq!(
                headers.contains(&("Access-Control-Allow-Origin", "*")),
                public
            );
        }
        for value in [
            json!("same-origin\r\nX-Injected: yes"),
            json!(false),
            json!(""),
        ] {
            assert!(ResponsePolicy::from_manifest(&manifest(json!({
                "cross_origin_opener_policy": {"value": value}
            })))
            .is_err());
        }
    }

    #[test]
    fn bounds_manifest_policy_resources() {
        assert_eq!(
            ResponsePolicy::from_manifest(&manifest(json!({
                "content_security_policy": {"extension_pages": " ".repeat(MAX_POLICY_BYTES + 1)}
            })))
            .unwrap_err(),
            PolicyError::ResourceLimit
        );
        assert_eq!(
            ResponsePolicy::from_manifest(&manifest(json!({
                "sandbox": {"pages": vec!["sandbox.html"; MAX_SANDBOX_PAGES + 1]}
            })))
            .unwrap_err(),
            PolicyError::ResourceLimit
        );
    }

    #[derive(Clone, Copy, Debug)]
    enum Capability {
        SelfScript,
        RemoteScript,
        InlineScript,
        Eval,
        Wasm,
        SelfWorker,
        RemoteWorker,
        SelfObject,
        RemoteObject,
    }

    // Reference source-list evaluator, separate from the production validator.
    fn allows(policy: &str, capability: Capability) -> bool {
        let directives: Vec<Vec<String>> = policy
            .split(';')
            .map(|part| {
                part.split_ascii_whitespace()
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .collect();
        let names: &[&str] = match capability {
            Capability::SelfScript | Capability::RemoteScript | Capability::InlineScript => {
                &["script-src-elem", "script-src", "default-src"]
            }
            Capability::Eval | Capability::Wasm => &["script-src", "default-src"],
            Capability::SelfWorker | Capability::RemoteWorker => {
                &["worker-src", "child-src", "script-src", "default-src"]
            }
            Capability::SelfObject | Capability::RemoteObject => &["object-src", "default-src"],
        };
        let sources = names.iter().find_map(|name| {
            directives
                .iter()
                .find(|words| words.first().is_some_and(|word| word == name))
                .map(|words| &words[1..])
        });
        let Some(sources) = sources else {
            return true;
        };
        let has = |source: &str| sources.iter().any(|word| word == source);
        match capability {
            Capability::SelfScript | Capability::SelfWorker | Capability::SelfObject => {
                has("'self'") || has("*")
            }
            Capability::RemoteScript | Capability::RemoteWorker | Capability::RemoteObject => {
                has("https:") || has("https://remote.example") || has("*")
            }
            Capability::InlineScript => {
                has("'unsafe-inline'") && !has("'nonce-test'") && !has("'strict-dynamic'")
            }
            Capability::Eval => has("'unsafe-eval'"),
            Capability::Wasm => has("'wasm-unsafe-eval'") || has("'unsafe-eval'"),
        }
    }

    proptest! {
        #[test]
        fn effective_policy_never_weaker_than_minimum(
            directives in prop::collection::vec((
                prop::sample::select(vec!["script-src", "SCRIPT-SRC", "script-src-elem", "script-src-attr", "default-src", "worker-src", "child-src", "object-src", "unknown-directive"]),
                prop::collection::vec(prop::sample::select(vec!["'self'", "'none'", "*", "https:", "https://remote.example", "'unsafe-inline'", "'unsafe-eval'", "'wasm-unsafe-eval'", "'nonce-test'", "'strict-dynamic'", "blob:", "data:"]), 0..8),
            ), 0..16),
            public in any::<bool>(),
            sandbox in any::<bool>(),
        ) {
            let mut supplied = String::new();
            for (name, sources) in directives {
                supplied.push_str(name);
                for source in sources { supplied.push(' '); supplied.push_str(source); }
                supplied.push(';');
            }
            let value = if sandbox {
                json!({"sandbox": {"pages": ["test.html"]}, "content_security_policy": {"sandbox": format!("sandbox allow-scripts;{supplied}")}})
            } else {
                json!({"content_security_policy": {"extension_pages": supplied}})
            };
            if let Ok(policy) = ResponsePolicy::from_manifest(&manifest(value)) {
                let headers = policy.headers_for("test.html", public);
                let policies = policies(&headers);
                prop_assert_eq!(policies.len(), 2);
                prop_assert_eq!(policies[1], MINIMUM_CSP);
                for capability in [Capability::SelfScript, Capability::RemoteScript, Capability::InlineScript, Capability::Eval, Capability::Wasm, Capability::SelfWorker, Capability::RemoteWorker, Capability::SelfObject, Capability::RemoteObject] {
                    let effective = policies.iter().all(|policy| allows(policy, capability));
                    prop_assert!(!effective || allows(MINIMUM_CSP, capability), "{capability:?} escaped minimum via {policies:?}");
                }
            }
        }
    }
}
