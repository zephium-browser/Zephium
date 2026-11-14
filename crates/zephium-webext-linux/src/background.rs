//! Extension backgrounds: a hidden, never-mapped owner page per running
//! extension that starts the service-worker script in a real dedicated
//! Worker, and the generated resources that page and worker load.
//!
//! - `/_zephium/background.html` loads `/_zephium/owner.js`, the binding's
//!   `owner` entry, which starts the worker and relays its frames;
//! - the worker starts at `<script>?zephium-worker`, a wrapper served from the
//!   script's own directory so relative `importScripts` and chunk URLs resolve
//!   as in Chrome; it loads `/_zephium/worker.js` (error hooks and the
//!   binding's `worker` entry) and then the script itself.

use std::path::Path;

use serde_json::{json, Value};
use zephium_webext::manifest::{Background, Manifest};
use zephium_webext_runtime::context::{ContextKind, ExtensionId};
use zephium_webext_runtime::js::{surface, BUNDLE, OWNER_BUNDLE, WORKER_BUNDLE};

use crate::scheme::{MemoryResource, SchemeError, WORKER_QUERY};

/// The owner page every background view loads.
pub const OWNER_PAGE: &str = "/_zephium/background.html";
const OWNER_SCRIPT: &str = "/_zephium/owner.js";
const WORKER_SCRIPT: &str = "/_zephium/worker.js";

/// The service-worker script a manifest declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundScript {
    /// Package path without a leading slash.
    pub path: String,
    /// `"type": "module"`.
    pub module: bool,
}

/// Why the runtime cannot run an extension. Shown to the person at install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallRefusal {
    /// Only Manifest V3 runs here.
    ManifestVersion(u8),
    /// `background.scripts` without a service worker (Manifest V2 style).
    BackgroundScripts,
    /// `background.page` without a service worker (Manifest V2 style).
    BackgroundPage,
    /// `background.service_worker` is not a valid package path.
    InvalidServiceWorker,
    /// The declared service-worker script is not in the package.
    MissingServiceWorker(String),
}

impl std::fmt::Display for InstallRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManifestVersion(version) => {
                write!(formatter, "Manifest version {version} is not supported.")
            }
            Self::BackgroundScripts => {
                formatter.write_str("'background.scripts' requires manifest version of 2 or lower.")
            }
            Self::BackgroundPage => {
                formatter.write_str("'background.page' requires manifest version of 2 or lower.")
            }
            Self::InvalidServiceWorker => {
                formatter.write_str("Invalid value for 'background.service_worker'.")
            }
            Self::MissingServiceWorker(path) => {
                write!(formatter, "Could not load background script '{path}'.")
            }
        }
    }
}

/// Checks the background a manifest declares against what this runtime
/// runs. `None`: the extension has no background, which is valid.
pub fn check_background(
    manifest: &Manifest,
    root: &Path,
) -> Result<Option<BackgroundScript>, InstallRefusal> {
    if manifest.manifest_version() != 3 {
        return Err(InstallRefusal::ManifestVersion(manifest.manifest_version()));
    }
    let raw = &manifest.raw()["background"];
    if !raw["service_worker"].is_null() {
        let Some(Background::ServiceWorker { path, module }) = manifest.background() else {
            return Err(InstallRefusal::InvalidServiceWorker);
        };
        if path.starts_with('_') || path.contains("..") {
            return Err(InstallRefusal::InvalidServiceWorker);
        }
        let file = root.join(&path);
        let regular = std::fs::symlink_metadata(&file).is_ok_and(|meta| meta.is_file());
        if !regular {
            return Err(InstallRefusal::MissingServiceWorker(path));
        }
        return Ok(Some(BackgroundScript { path, module }));
    }
    if !raw["scripts"].is_null() {
        return Err(InstallRefusal::BackgroundScripts);
    }
    if !raw["page"].is_null() {
        return Err(InstallRefusal::BackgroundPage);
    }
    Ok(None)
}

/// The binding's name for a context kind.
pub(crate) fn kind_name(kind: ContextKind) -> &'static str {
    match kind {
        ContextKind::Background => "background",
        ContextKind::Popup => "popup",
        ContextKind::Options => "options",
        ContextKind::ExtensionTab => "extensionTab",
        ContextKind::Offscreen => "offscreen",
        ContextKind::ExtensionFrame => "extensionFrame",
        ContextKind::ContentScript => "contentScript",
        ContextKind::UserScript => "userScript",
        ContextKind::ExternalPage => "externalPage",
    }
}

/// Everything one context's binding is told about itself.
#[derive(Clone, Debug)]
pub(crate) struct Bootstrap<'a> {
    pub extension_id: &'a ExtensionId,
    pub kind: ContextKind,
    pub manifest: &'a Value,
    pub messages: &'a Value,
    pub ui_language: &'a str,
    pub incognito: bool,
    pub unpacked: bool,
    pub permissions: &'a [String],
}

impl Bootstrap<'_> {
    pub(crate) fn to_value(&self, worker: Option<Value>) -> Value {
        let mut value = json!({
            "extensionId": self.extension_id.as_str(),
            "scheme": "chrome-extension",
            "contextKind": kind_name(self.kind),
            "manifest": self.manifest,
            "messages": self.messages,
            "uiLanguage": self.ui_language,
            "incognito": self.incognito,
            "unpacked": self.unpacked,
            "permissions": self.permissions,
        });
        if let (Some(worker), Some(object)) = (worker, value.as_object_mut()) {
            object.insert("worker".into(), worker);
        }
        value
    }
}

/// `(BUNDLE)('page', surface, bootstrap)` for an extension page's default
/// world, injected at document start before any page script.
pub(crate) fn page_script(bootstrap: &Value) -> String {
    format!(
        "(function(){{'use strict';try{{({BUNDLE})('page',{},{});}}catch(e){{}}}})();",
        surface(),
        bootstrap
    )
}

/// The resources one background start publishes.
pub(crate) struct Launch {
    /// The owner page URL to load.
    pub url: String,
    /// `/_zephium/*` resources.
    pub resources: Vec<(&'static str, MemoryResource)>,
    /// The worker wrapper for the script path.
    pub wrapper: MemoryResource,
    /// The script path the wrapper is served for.
    pub script: String,
}

fn script_url(extension: &ExtensionId, path: &str) -> String {
    format!("chrome-extension://{}/{path}", extension.as_str())
}

// Runs in the worker before the binding: reports console.error (which the
// binding also uses for unchecked lastError and listener exceptions),
// unhandled rejections and, while installing, the classic scripts imported
// (the set a restarted worker may import again), then starts the binding.
const WORKER_HOOKS: &str = r#"(function(B,S,D){'use strict';
const g=globalThis,J=JSON.stringify,St=String,U=URL,then=Promise.prototype.then,apply=Reflect.apply,isErr=(v)=>v instanceof Error;
let client=null,reports=0;const early=[];
const text=(v)=>{let s;try{s=isErr(v)?(v.stack&&St(v.stack).indexOf(St(v.message))!==-1?St(v.stack):St(v.name)+': '+St(v.message)):typeof v==='string'?v:J(v);}catch(e){s='';}if(typeof s!=='string')s=St(s);return s.length>1024?s.slice(0,1024):s;};
const send=(k,d)=>{if(reports>=320)return;if(client===null){if(early.length<16)early.push([k,d]);return;}reports++;try{apply(then,client.call('zephium.workerReport',J([k,d]),true),[undefined,()=>{}]);}catch(e){}};
const c=g.console;if(c&&typeof c.error==='function'){const original=c.error;const error=function error(...a){try{send('console',a.map(text).join(' '));}catch(e){}return apply(original,this,a);};try{Object.defineProperty(c,'error',{value:error,writable:true,enumerable:true,configurable:true});}catch(e){}}
g.addEventListener('unhandledrejection',(e)=>send('rejection','Uncaught (in promise) '+text(e.reason)));
const w=D&&D.worker,ni=g.importScripts;
if(w&&w.install===true&&w.type!=='module'&&typeof ni==='function'){g.importScripts=function importScripts(...u){const r=apply(ni,this,u);try{send('imported',u.map((x)=>new U(St(x),w.scriptURL).href));}catch(e){}return r;};}
const context=B(S,D);client=context.client;
for(const d of early.splice(0))send(d[0],d[1]);
send('booted',null);
})"#;

pub(crate) fn launch(
    extension: &ExtensionId,
    script: &BackgroundScript,
    bootstrap: &Bootstrap<'_>,
    install: bool,
    installed_scripts: &[String],
    unlimited_storage: bool,
    worker_surface: &str,
) -> Result<Launch, SchemeError> {
    let scope = script_url(extension, &script.path);
    let kind = if script.module { "module" } else { "classic" };
    let quota: u64 = if unlimited_storage { 1 << 30 } else { 10 << 20 };
    let worker = json!({
        "type": kind,
        "scriptURL": scope,
        "install": install,
        "installedScripts": installed_scripts,
        "storage": {"quota": quota, "usage": 0, "persisted": true},
    });
    let boot = bootstrap.to_value(Some(worker));
    let worker_js = format!("{WORKER_HOOKS}({WORKER_BUNDLE},{worker_surface},{boot});");
    let owner = json!({
        "workerURL": format!("{scope}?{WORKER_QUERY}"),
        "type": kind,
    });
    let owner_js = format!("({OWNER_BUNDLE})({owner});");
    let worker_url = script_url(extension, &WORKER_SCRIPT[1..]);
    let quoted = |url: &str| serde_json::to_string(url).unwrap_or_default();
    let wrapper = if script.module {
        format!(
            "import {};\nimport {};\n",
            quoted(&worker_url),
            quoted(&scope)
        )
    } else {
        format!(
            "importScripts({});\nimportScripts({});\n",
            quoted(&worker_url),
            quoted(&scope)
        )
    };
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>background</title><script src=\"{OWNER_SCRIPT}\"></script>"
    );
    Ok(Launch {
        url: script_url(extension, &OWNER_PAGE[1..]),
        resources: vec![
            (
                OWNER_PAGE,
                MemoryResource::new(html.into_bytes(), "text/html")?,
            ),
            (
                OWNER_SCRIPT,
                MemoryResource::new(owner_js.into_bytes(), "text/javascript")?,
            ),
            (
                WORKER_SCRIPT,
                MemoryResource::new(worker_js.into_bytes(), "text/javascript")?,
            ),
        ],
        wrapper: MemoryResource::new(wrapper.into_bytes(), "text/javascript")?,
        script: format!("/{}", script.path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(value: Value) -> Manifest {
        Manifest::from_value(value)
    }

    #[test]
    fn refuses_what_this_runtime_cannot_run() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("sw.js"), "").unwrap();
        let root = directory.path();
        let check = |value| check_background(&manifest(value), root);
        assert_eq!(
            check(json!({"manifest_version": 2, "background": {"scripts": ["a.js"]}})),
            Err(InstallRefusal::ManifestVersion(2))
        );
        assert_eq!(
            check(json!({"manifest_version": 3, "background": {"scripts": ["a.js"]}})),
            Err(InstallRefusal::BackgroundScripts)
        );
        assert_eq!(
            check(json!({"manifest_version": 3, "background": {"page": "a.html"}})),
            Err(InstallRefusal::BackgroundPage)
        );
        assert_eq!(
            check(json!({"manifest_version": 3, "background": {"service_worker": "missing.js"}})),
            Err(InstallRefusal::MissingServiceWorker("missing.js".into()))
        );
        assert_eq!(check(json!({"manifest_version": 3})), Ok(None));
        assert_eq!(
            check(
                json!({"manifest_version": 3, "background": {"service_worker": "/sw.js", "type": "module", "scripts": ["firefox.js"]}})
            ),
            Ok(Some(BackgroundScript {
                path: "sw.js".into(),
                module: true
            }))
        );
    }

    #[test]
    fn launch_serves_a_wrapper_beside_the_script() {
        let id = ExtensionId::parse("abcdefghijklmnopabcdefghijklmnop").unwrap();
        let raw = json!({"manifest_version": 3});
        let messages = json!({});
        let bootstrap = Bootstrap {
            extension_id: &id,
            kind: ContextKind::Background,
            manifest: &raw,
            messages: &messages,
            ui_language: "en-US",
            incognito: false,
            unpacked: false,
            permissions: &[],
        };
        for module in [false, true] {
            let script = BackgroundScript {
                path: "js/sw.js".into(),
                module,
            };
            let worker_surface =
                zephium_webext_runtime::js::background_surface(&manifest(raw.clone()));
            let launch =
                launch(&id, &script, &bootstrap, true, &[], false, &worker_surface).unwrap();
            assert_eq!(launch.script, "/js/sw.js");
            assert_eq!(
                launch.url,
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop/_zephium/background.html"
            );
            assert_eq!(launch.resources.len(), 3);
        }
    }
}
