//! Manifest host-access narrowing for native WebView2 packages.
//! This never implements extension APIs; Chromium enforces the resulting grants.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub const HOST_DIRECTORY: &str = "zephium-windows-host";
const READY_FILE: &str = "zephium-windows-prepared.txt";
const OBSERVER: &str = include_str!("windows/action-observer.js");
const WORKER_COMPAT: &str = include_str!("windows/worker-compat.js");
const HOST_SCRIPT: &str = include_str!("windows/action-host.js");
pub const POPUP_TARGET_SCRIPT: &str = include_str!("windows/popup-target.js");
pub const POPUP_SIZE_SCRIPT: &str = include_str!("windows/popup-size.js");

pub fn prepare(dir: &Path, sites: Option<&[String]>) -> Result<(), String> {
    let manifest = crate::manifest::Manifest::load(dir).map_err(|e| e.to_string())?;
    if manifest.manifest_version() != 3 {
        return Err("Windows currently supports Manifest V3 extensions.".into());
    }
    let mut raw = manifest.raw().clone();
    if let Some(sites) = sites {
        narrow_hosts(&mut raw, sites)?;
    }
    let owned = dir.join(HOST_DIRECTORY);
    fs::create_dir(&owned).map_err(|e| e.to_string())?;
    fs::write(
        owned.join("observer.js"),
        format!("{WORKER_COMPAT}\n{OBSERVER}"),
    )
    .map_err(|e| e.to_string())?;
    fs::write(owned.join("host.js"), HOST_SCRIPT).map_err(|e| e.to_string())?;
    fs::write(
        owned.join("host.html"),
        "<!doctype html><meta charset=utf-8><script src=host.js></script>",
    )
    .map_err(|e| e.to_string())?;
    if let Some(crate::manifest::Background::ServiceWorker { path, module }) = manifest.background()
    {
        let worker = dir.join(path.split(['?', '#']).next().ok_or("Invalid worker path")?);
        let source = fs::read(&worker).map_err(|e| e.to_string())?;
        fs::write(
            worker,
            crate::prepare::inject_worker(
                &source,
                module,
                &format!("/{HOST_DIRECTORY}/observer.js"),
            ),
        )
        .map_err(|e| e.to_string())?;
    }
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&raw).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(dir.join(READY_FILE), access_revision(sites)).map_err(|e| e.to_string())
}

/// WebView2 reads an unpacked extension without long-path support, so every
/// file's full path has to fit in MAX_PATH; past it, AddBrowserExtension only
/// reports E_FAIL.
pub const MAX_LOADABLE_PATH: usize = 259;

/// The longest path of any file under `tree`, relative to it, in UTF-16 units.
pub fn longest_relative_path(tree: &Path) -> std::io::Result<usize> {
    fn visit(tree: &Path, directory: &Path, longest: &mut usize) -> std::io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                visit(tree, &path, longest)?;
            } else {
                let relative = path.strip_prefix(tree).map_err(std::io::Error::other)?;
                *longest = (*longest).max(utf16_len(relative));
            }
        }
        Ok(())
    }
    let mut longest = 0;
    visit(tree, tree, &mut longest)?;
    Ok(longest)
}

/// Whether a package whose longest relative file path is `longest` can load
/// from `root`, and if not, why, in words the person can act on.
pub fn path_budget(root: &Path, longest: usize) -> Result<(), String> {
    let full = utf16_len(root) + 1 + longest;
    if full <= MAX_LOADABLE_PATH {
        return Ok(());
    }
    Err(format!(
        "This extension can't load on Windows: one of its files would sit at a path {full} characters long, \
         past the {MAX_LOADABLE_PATH} that Windows extensions can use. Its files are nested too deeply for \
         where this profile keeps its data."
    ))
}

fn utf16_len(path: &Path) -> usize {
    path.to_string_lossy().encode_utf16().count()
}

pub fn is_prepared(dir: &Path, revision: &str) -> bool {
    fs::read_to_string(dir.join(READY_FILE)).is_ok_and(|value| value == revision)
}

pub fn access_revision(sites: Option<&[String]>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"zephium-windows-manifest-v1");
    hash.update(OBSERVER);
    hash.update(WORKER_COMPAT);
    hash.update(HOST_SCRIPT);
    hash.update(serde_json::to_vec(&sites).expect("string lists serialize"));
    format!("windows-{:x}", hash.finalize())[..40].to_owned()
}

/// Sites are canonical host names, including subdomains. Existing path and
/// scheme restrictions remain in place. Empty intersections remove scripts.
pub fn narrow_hosts(manifest: &mut Value, sites: &[String]) -> Result<(), String> {
    for site in sites {
        let url = url::Url::parse(&format!("https://{site}/")).map_err(|_| "Invalid site")?;
        if url.host_str() != Some(site)
            || !url.username().is_empty()
            || url.port().is_some()
            || site.contains(['*', '/', '?', '#', '@', ':'])
        {
            return Err("Invalid site".into());
        }
    }
    for key in [
        "host_permissions",
        "optional_host_permissions",
        "permissions",
        "optional_permissions",
    ] {
        if let Some(values) = manifest.get_mut(key).and_then(Value::as_array_mut) {
            let mut narrowed = Vec::new();
            for value in values.iter() {
                let Some(pattern) = value.as_str() else {
                    return Err("Invalid permission".into());
                };
                if crate::manifest::is_match_pattern(pattern) {
                    narrowed.extend(intersections(pattern, sites).into_iter().map(Value::String));
                } else if pattern == "declarativeNetRequest" {
                    // Chromium otherwise allows block/upgrade rules without a
                    // host grant. The native alternative requires host access.
                    narrowed.push(Value::String("declarativeNetRequestWithHostAccess".into()));
                } else if pattern != "activeTab" {
                    narrowed.push(value.clone());
                }
            }
            *values = narrowed;
        }
    }
    if let Some(scripts) = manifest
        .get_mut("content_scripts")
        .and_then(Value::as_array_mut)
    {
        for script in scripts.iter_mut() {
            let Some(matches) = script["matches"].as_array() else {
                return Err("Missing script matches".into());
            };
            let narrowed: Vec<Value> = matches
                .iter()
                .filter_map(Value::as_str)
                .flat_map(|pattern| intersections(pattern, sites))
                .map(Value::String)
                .collect();
            script["matches"] = Value::Array(narrowed);
        }
        scripts.retain(|script| {
            script["matches"]
                .as_array()
                .is_some_and(|matches| !matches.is_empty())
        });
    }
    Ok(())
}

fn intersections(pattern: &str, sites: &[String]) -> Vec<String> {
    let expanded = if pattern == "<all_urls>" {
        "*://*/*"
    } else {
        pattern
    };
    let Some((scheme, rest)) = expanded.split_once("://") else {
        return Vec::new();
    };
    if !matches!(scheme, "*" | "http" | "https") {
        return Vec::new();
    }
    let Some((host, path)) = rest.split_once('/') else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for site in sites {
        let allowed = if site.parse::<std::net::IpAddr>().is_ok() {
            site.clone()
        } else {
            format!("*.{site}")
        };
        let intersection = if host == "*" || contains_host(host, &allowed) {
            Some(allowed.as_str())
        } else if contains_host(&allowed, host) {
            Some(host)
        } else {
            None
        };
        if let Some(host) = intersection {
            result.push(format!("{scheme}://{host}/{path}"));
        }
    }
    result.sort();
    result.dedup();
    result
}

fn contains_host(outer: &str, inner: &str) -> bool {
    if outer == inner {
        return true;
    }
    let Some(suffix) = outer.strip_prefix("*.") else {
        return false;
    };
    let candidate = inner.strip_prefix("*.").unwrap_or(inner);
    candidate == suffix
        || candidate
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn narrowing_cannot_widen_original_scheme_host_or_path() {
        let mut manifest = json!({"host_permissions":["https://api.example.com/private/*"],
            "optional_host_permissions":["<all_urls>"],"permissions":["storage","activeTab"],
            "content_scripts":[{"matches":["https://*.example.com/foo*"],"exclude_matches":["*://secret.example.com/*"],"js":["a.js"]},{"matches":["https://other.test/*"],"js":["b.js"]}]});
        narrow_hosts(&mut manifest, &["example.com".into()]).unwrap();
        assert_eq!(
            manifest["host_permissions"],
            json!(["https://api.example.com/private/*"])
        );
        assert_eq!(
            manifest["optional_host_permissions"],
            json!(["*://*.example.com/*"])
        );
        assert_eq!(manifest["permissions"], json!(["storage"]));
        assert_eq!(manifest["content_scripts"].as_array().unwrap().len(), 1);
        assert_eq!(
            manifest["content_scripts"][0]["matches"],
            json!(["https://*.example.com/foo*"])
        );
        assert_eq!(
            manifest["content_scripts"][0]["exclude_matches"],
            json!(["*://secret.example.com/*"])
        );
        assert!(intersections("https://notexample.com/*", &["example.com".into()]).is_empty());
        assert!(intersections("file:///*", &["example.com".into()]).is_empty());
    }
    #[test]
    fn empty_sites_deny_all_hosts_and_invalid_sites_are_rejected() {
        let mut manifest = json!({"host_permissions":["<all_urls>"],"content_scripts":[{"matches":["<all_urls>"],"js":["a.js"]}]});
        narrow_hosts(&mut manifest, &[]).unwrap();
        assert_eq!(manifest["host_permissions"], json!([]));
        assert_eq!(manifest["content_scripts"], json!([]));
        for site in [
            "*",
            "example.com/path",
            "user@example.com",
            "example.com:123",
        ] {
            assert!(narrow_hosts(&mut manifest, &[site.into()]).is_err());
        }
    }

    #[test]
    fn network_rules_require_hosts_and_absent_fields_stay_absent() {
        let mut manifest =
            json!({"permissions":["declarativeNetRequest"],"host_permissions":["<all_urls>"]});
        narrow_hosts(&mut manifest, &["127.0.0.1".into()]).unwrap();
        assert_eq!(
            manifest["permissions"],
            json!(["declarativeNetRequestWithHostAccess"])
        );
        assert_eq!(manifest["host_permissions"], json!(["*://127.0.0.1/*"]));
        assert!(manifest.get("optional_host_permissions").is_none());
        assert!(manifest.get("content_scripts").is_none());
    }

    #[test]
    fn preparation_retains_identity_and_only_marks_complete_packages_ready() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let manifest = json!({"manifest_version":3,"name":"Test","version":"1.0",
            "key":"public-key","host_permissions":["<all_urls>"],
            "background":{"service_worker":"worker.js"}});
        fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
        let sites = vec!["example.com".to_owned()];
        let revision = access_revision(Some(&sites));
        assert!(prepare(dir, Some(&sites)).is_err());
        assert!(!is_prepared(dir, &revision));

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
        fs::write(dir.join("worker.js"), "'use strict';\nself.fixture=true;").unwrap();
        prepare(dir, Some(&sites)).unwrap();
        assert!(is_prepared(dir, &revision));
        assert!(!is_prepared(dir, &access_revision(None)));
        let raw: Value =
            serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(raw["key"], "public-key");
        assert_eq!(raw["host_permissions"], json!(["*://*.example.com/*"]));
        assert!(fs::read_to_string(dir.join("worker.js"))
            .unwrap()
            .contains("observer.js"));
    }
}

#[cfg(test)]
mod path_budget_tests {
    use super::*;
    use std::path::PathBuf;

    // Bitwarden 2026.10.1's deepest file.
    const BITWARDEN_DEEPEST: &str =
        "images/at-risk-password-carousel/review_at-risk_logins.light.png";

    fn package_root(app_data: &str, identity: &str, package: &str) -> PathBuf {
        Path::new(app_data)
            .join(identity)
            .join("webext")
            .join("packages")
            .join("nngceckbapebfimnlniiiahkandclblb")
            .join(package)
    }

    fn short() -> String {
        crate::package::publication_identity("crx-a", "windows-1", "digest")
    }

    fn long() -> String {
        format!("pkg-{}", "0".repeat(64))
    }

    #[test]
    fn a_long_user_name_still_loads_once_package_names_are_short() {
        let app_data = format!(r"C:\Users\{}\AppData\Roaming", "u".repeat(60));
        let deepest = BITWARDEN_DEEPEST.len();
        assert!(path_budget(&package_root(&app_data, "app.zephium", &short()), deepest).is_ok());
        assert!(path_budget(&package_root(&app_data, "app.zephium", &long()), deepest).is_err());
    }

    #[test]
    fn redirected_packaged_app_data_fits_with_short_names_and_says_why_when_not() {
        // A process started by a packaged (MSIX) app sees Roaming redirected here.
        let app_data =
            r"C:\Users\user\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming";
        let deepest = BITWARDEN_DEEPEST.len();
        let identity = "app.zephium.performance";
        assert!(path_budget(&package_root(app_data, identity, &short()), deepest).is_ok());
        let refused = path_budget(&package_root(app_data, identity, &long()), deepest).unwrap_err();
        assert!(refused.contains("283 characters long"), "{refused}");
        assert!(refused.contains("259"), "{refused}");
    }

    #[test]
    fn the_longest_relative_path_counts_every_level() {
        let tree = tempfile::tempdir().unwrap();
        fs::create_dir_all(tree.path().join("a").join("bb")).unwrap();
        fs::write(tree.path().join("a").join("bb").join("nested.js"), "").unwrap();
        fs::write(tree.path().join("manifest.json"), "{}").unwrap();
        assert_eq!(
            longest_relative_path(tree.path()).unwrap(),
            "a/bb/nested.js".len()
        );
    }
}
