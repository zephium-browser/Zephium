//! One host-authorized, revocable package service per profile WebContext.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use glib::prelude::*;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use url::Url;
use webkit2gtk::{SecurityManagerExt, URISchemeRequestExt, URISchemeResponseExt, WebContextExt};
use zephium_core::ids::ExtensionInstallId;
use zephium_webext::manifest::Manifest;
use zephium_webext_runtime::pattern::HostPattern;

use crate::csp::{PolicyError, ResponsePolicy};
use crate::package::{ExtensionUri, PackageError, PreparedPackage, ResourcePath};

const MAX_INSTALLS: usize = 128;
const MAX_WAR_ENTRIES: usize = 128;
const MAX_WAR_VALUES: usize = 1024;
const MAX_MEMORY_BYTES: usize = 1024 * 1024;
const MAX_RUNTIME_BYTES: usize = 8 * MAX_MEMORY_BYTES;
const MAX_RUNTIME_RESOURCES: usize = 128;
const MAX_ACTIVE_STREAMS: usize = 128;

/// Facts from the engine's live view registry, never request headers or JS claims.
#[derive(Clone, Debug)]
pub enum Requester {
    ExtensionOwned {
        install: ExtensionInstallId,
    },
    Tab {
        committed_top_level: Url,
        /// Present only for an admitted extension document of this install generation.
        extension_install: Option<ExtensionInstallId>,
    },
}

pub trait RequestHost {
    /// Return None for unregistered, stale, Work, preview, or privileged frame views.
    /// Keep committed origins and install generations synchronized with navigation.
    /// Refuse opaque/sandboxed origins; a document URL alone is not origin authority.
    /// Retire owned identity on cross-origin navigation and install replacement.
    fn requester(&self, view: &webkit2gtk::WebView) -> Option<Requester>;
}

pub trait FaviconProvider {
    /// Called only for an owning install's view or admitted extension tab.
    /// The host must check favicon permission and return a fixed raster, never raw page data.
    fn favicon(&self, install: ExtensionInstallId, uri: &str) -> Option<MemoryResource>;
}

#[derive(Clone)]
pub struct MemoryResource {
    bytes: Arc<[u8]>,
    mime_type: String,
}

impl MemoryResource {
    pub fn new(bytes: Vec<u8>, mime_type: &str) -> Result<Self, SchemeError> {
        if bytes.len() > MAX_MEMORY_BYTES
            || mime_type.is_empty()
            || mime_type.len() > 128
            || !mime_type
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        {
            return Err(SchemeError::InvalidResource);
        }
        Ok(Self {
            bytes: bytes.into(),
            mime_type: mime_type.to_owned(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SchemeError {
    #[error("extension scheme already registered on this context")]
    AlreadyRegistered,
    #[error("WebKit security manager unavailable")]
    MissingSecurityManager,
    #[error("profile retired")]
    Retired,
    #[error("invalid or duplicate install")]
    InvalidInstall,
    #[error("invalid web accessible resources manifest")]
    InvalidManifest,
    #[error("invalid or oversized runtime resource")]
    InvalidResource,
    #[error("session alias generation failed")]
    Random,
    #[error(transparent)]
    Package(#[from] PackageError),
    #[error(transparent)]
    Policy(#[from] PolicyError),
}

struct Install {
    id: String,
    alias: String,
    package: PreparedPackage,
    policy: ResponsePolicy,
    war: Vec<WarEntry>,
    active: Arc<AtomicBool>,
    runtime: BTreeMap<String, MemoryResource>,
}

thread_local! {
    static REGISTERED: RefCell<Vec<glib::WeakRef<webkit2gtk::WebContext>>> = const { RefCell::new(Vec::new()) };
}

pub struct Scheme {
    host: Rc<dyn RequestHost>,
    favicon: Option<Rc<dyn FaviconProvider>>,
    installs: RefCell<BTreeMap<ExtensionInstallId, Install>>,
    active: Arc<AtomicBool>,
    streams: Arc<std::sync::atomic::AtomicUsize>,
}

impl Scheme {
    /// Register directly before creating profile views. Reserve `chrome-extension`
    /// exclusively for this service: do not also register it through Wry/Tauri.
    /// Keep the returned owner alive for the profile; dropping it refuses all loads.
    pub fn register(
        context: &webkit2gtk::WebContext,
        host: Rc<dyn RequestHost>,
        favicon: Option<Rc<dyn FaviconProvider>>,
    ) -> Result<Rc<Self>, SchemeError> {
        let security = context
            .security_manager()
            .ok_or(SchemeError::MissingSecurityManager)?;
        REGISTERED.with(|registered| {
            let mut registered = registered.borrow_mut();
            registered.retain(|item| item.upgrade().is_some());
            if registered
                .iter()
                .any(|item| item.upgrade().as_ref() == Some(context))
            {
                return Err(SchemeError::AlreadyRegistered);
            }
            registered.push(context.downgrade());
            Ok(())
        })?;
        let owner = Rc::new(Self {
            host,
            favicon,
            installs: RefCell::new(BTreeMap::new()),
            active: Arc::new(AtomicBool::new(true)),
            streams: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        security.register_uri_scheme_as_secure("chrome-extension");
        security.register_uri_scheme_as_cors_enabled("chrome-extension");
        let weak = Rc::downgrade(&owner);
        context.register_uri_scheme("chrome-extension", move |request| {
            if let Some(owner) = weak.upgrade() {
                owner.serve(request);
            } else {
                refuse(request, 403);
            }
        });
        Ok(owner)
    }

    /// Publish a prepared immutable directory after installation/consent succeeds.
    /// The returned random alias lasts only for this admission and profile session.
    pub fn add_install(
        &self,
        install: ExtensionInstallId,
        extension_id: &str,
        root: &Path,
        manifest: &Manifest,
    ) -> Result<String, SchemeError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(SchemeError::Retired);
        }
        let mut installs = self.installs.borrow_mut();
        if !valid_id(extension_id)
            || installs.len() >= MAX_INSTALLS
            || installs.contains_key(&install)
            || installs
                .values()
                .any(|entry| entry.id == extension_id || entry.alias == extension_id)
        {
            return Err(SchemeError::InvalidInstall);
        }
        if manifest.manifest_version() != 3 {
            return Err(SchemeError::InvalidManifest);
        }
        let package = PreparedPackage::open(root)?;
        let policy = ResponsePolicy::from_manifest(manifest)?;
        let war = WarEntry::parse(manifest)?;
        let alias = loop {
            let mut bytes = [0; 16];
            SystemRandom::new()
                .fill(&mut bytes)
                .map_err(|_| SchemeError::Random)?;
            let alias: String = bytes
                .into_iter()
                .flat_map(|b| [char::from(b'a' + (b >> 4)), char::from(b'a' + (b & 15))])
                .collect();
            if alias != extension_id
                && !installs
                    .values()
                    .any(|entry| entry.id == alias || entry.alias == alias)
            {
                break alias;
            }
        };
        installs.insert(
            install,
            Install {
                id: extension_id.to_owned(),
                alias: alias.clone(),
                package,
                policy,
                war,
                active: Arc::new(AtomicBool::new(true)),
                runtime: BTreeMap::new(),
            },
        );
        Ok(alias)
    }

    /// Call on disable, uninstall, or replacement, before deleting package files.
    /// Cancels future reads as well as new requests; already delivered bytes cannot be recalled.
    pub fn remove_install(&self, install: ExtensionInstallId) -> bool {
        if let Some(entry) = self.installs.borrow_mut().remove(&install) {
            entry.active.store(false, Ordering::Release);
            true
        } else {
            false
        }
    }

    pub fn retire(&self) {
        self.active.store(false, Ordering::Release);
        self.installs.borrow_mut().clear();
    }

    pub fn set_runtime_resource(
        &self,
        install: ExtensionInstallId,
        path: &str,
        resource: MemoryResource,
    ) -> Result<(), SchemeError> {
        let path = ResourcePath::parse(path)?;
        if !path.is_reserved() {
            return Err(SchemeError::InvalidResource);
        }
        let mut installs = self.installs.borrow_mut();
        let entry = installs
            .get_mut(&install)
            .ok_or(SchemeError::InvalidInstall)?;
        let total: usize = entry.runtime.values().map(|item| item.bytes.len()).sum();
        let previous = entry
            .runtime
            .get(path.as_str())
            .map_or(0, |item| item.bytes.len());
        if (entry.runtime.len() >= MAX_RUNTIME_RESOURCES
            && !entry.runtime.contains_key(path.as_str()))
            || total - previous + resource.bytes.len() > MAX_RUNTIME_BYTES
        {
            return Err(SchemeError::InvalidResource);
        }
        entry.runtime.insert(path.as_str().to_owned(), resource);
        Ok(())
    }

    /// Use at navigation policy admission too, before promoting a web tab to an
    /// extension document. This checks authority, not resource existence.
    pub fn allows(&self, requester: &Requester, uri: &str) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        let Ok(uri) = ExtensionUri::parse(uri) else {
            return false;
        };
        let installs = self.installs.borrow();
        admitted(&installs, requester, &uri).is_some()
    }

    /// Host API injection must refuse sandbox documents, including alias URLs.
    /// None means the profile, install or URI is not admitted.
    pub fn is_sandboxed(&self, uri: &str) -> Option<bool> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }
        let uri = ExtensionUri::parse(uri).ok()?;
        self.installs
            .borrow()
            .values()
            .find(|entry| entry.id == uri.host() || entry.alias == uri.host())
            .map(|entry| entry.policy.is_sandboxed(uri.resource().as_str()))
    }

    fn serve(&self, request: &webkit2gtk::URISchemeRequest) {
        if !self.active.load(Ordering::Acquire) {
            refuse(request, 403);
            return;
        }
        // SAFETY: WebKit owns these NUL-terminated strings for the live request.
        let method = unsafe {
            bounded_native_string(
                webkit2gtk::ffi::webkit_uri_scheme_request_get_http_method(request.as_ptr()),
                4,
            )
        };
        if !matches!(method.as_deref(), Some("GET" | "HEAD")) {
            refuse(request, 405);
            return;
        }
        // SAFETY: Same borrowed request lifetime; bound before allocating a Rust copy.
        let Some(raw_uri) = (unsafe {
            bounded_native_string(
                webkit2gtk::ffi::webkit_uri_scheme_request_get_uri(request.as_ptr()),
                crate::package::MAX_URI_LENGTH,
            )
        }) else {
            refuse(request, 403);
            return;
        };
        let Ok(uri) = ExtensionUri::parse(&raw_uri) else {
            refuse(request, 403);
            return;
        };
        let Some(requester) = request
            .web_view()
            .and_then(|view| self.host.requester(&view))
        else {
            refuse(request, 403);
            return;
        };
        let installs = self.installs.borrow();
        let Some((install, entry, public)) = admitted(&installs, &requester, &uri) else {
            refuse(request, 403);
            return;
        };
        if self.streams.load(Ordering::Acquire) >= MAX_ACTIVE_STREAMS {
            refuse(request, 503);
            return;
        }
        let memory = if uri.resource().is_reserved() {
            entry.runtime.get(uri.resource().as_str()).cloned()
        } else if uri.resource().is_favicon() {
            self.favicon
                .as_ref()
                .and_then(|provider| provider.favicon(install, &raw_uri))
        } else {
            None
        };
        let body: Result<(Box<dyn Read + Send>, u64, String), PackageError> =
            if uri.resource().is_reserved() || uri.resource().is_favicon() {
                memory
                    .map(|resource| {
                        let length = resource.bytes.len() as u64;
                        (
                            Box::new(io::Cursor::new(resource.bytes)) as Box<dyn Read + Send>,
                            length,
                            resource.mime_type,
                        )
                    })
                    .ok_or(PackageError::NotFound)
            } else {
                entry.package.open_resource(uri.resource()).map(|resource| {
                    (
                        Box::new(resource.file.take(resource.length)) as Box<dyn Read + Send>,
                        resource.length,
                        resource.mime_type.to_owned(),
                    )
                })
            };
        let (body, length, mime) = match body {
            Ok(body) => body,
            Err(error) => {
                refuse(request, error.status_code());
                return;
            }
        };
        let Ok(length) = i64::try_from(length) else {
            refuse(request, 403);
            return;
        };
        self.streams.fetch_add(1, Ordering::AcqRel);
        let stream = gio::ReadInputStream::new(RevocableReader {
            body,
            profile_active: self.active.clone(),
            install_active: entry.active.clone(),
            streams: self.streams.clone(),
            head: method.as_deref() == Some("HEAD"),
        });
        let response = webkit2gtk::URISchemeResponse::new(
            &stream,
            if method.as_deref() == Some("HEAD") {
                0
            } else {
                length
            },
        );
        response.set_status(200, None);
        response.set_content_type(&mime);
        let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
        headers.append("Content-Type", &mime);
        headers.append("Content-Length", &length.to_string());
        for (name, value) in entry.policy.headers_for(uri.resource().as_str(), public) {
            headers.append(name, value);
        }
        // URLs omit the publication digest, and caches must not outlive admission.
        headers.append("Cache-Control", "no-store");
        response.set_http_headers(headers);
        request.finish_with_response(&response);
    }
}

impl Drop for Scheme {
    fn drop(&mut self) {
        self.retire();
    }
}

struct RevocableReader {
    body: Box<dyn Read + Send>,
    profile_active: Arc<AtomicBool>,
    install_active: Arc<AtomicBool>,
    streams: Arc<std::sync::atomic::AtomicUsize>,
    head: bool,
}
impl Read for RevocableReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.profile_active.load(Ordering::Acquire)
            || !self.install_active.load(Ordering::Acquire)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "extension retired",
            ));
        }
        if self.head {
            Ok(0)
        } else {
            self.body.read(buffer)
        }
    }
}
impl Drop for RevocableReader {
    fn drop(&mut self) {
        self.streams.fetch_sub(1, Ordering::AcqRel);
    }
}

unsafe fn bounded_native_string(pointer: *const std::ffi::c_char, limit: usize) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    for length in 0..=limit {
        // SAFETY: The caller provides a live C string; stop at its first NUL.
        if unsafe { *pointer.add(length) } == 0 {
            // SAFETY: These bytes precede the terminator just observed.
            let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) };
            return std::str::from_utf8(bytes).ok().map(str::to_owned);
        }
    }
    None
}

fn refuse(request: &webkit2gtk::URISchemeRequest, status: u32) {
    let stream = gio::MemoryInputStream::new();
    let response = webkit2gtk::URISchemeResponse::new(&stream, 0);
    response.set_status(status, None);
    response.set_content_type("text/plain");
    let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
    headers.append("Content-Type", "text/plain");
    headers.append("Cache-Control", "no-store");
    headers.append("X-Content-Type-Options", "nosniff");
    headers.append("Content-Security-Policy", "default-src 'none'; sandbox");
    headers.append("Content-Security-Policy", crate::csp::MINIMUM_CSP);
    response.set_http_headers(headers);
    request.finish_with_response(&response);
}

fn valid_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

fn admitted<'a>(
    installs: &'a BTreeMap<ExtensionInstallId, Install>,
    requester: &Requester,
    uri: &ExtensionUri,
) -> Option<(ExtensionInstallId, &'a Install, bool)> {
    if uri.resource().as_str() == "_metadata" || uri.resource().as_str().starts_with("_metadata/") {
        return None;
    }
    let (&install, entry) = installs
        .iter()
        .find(|(_, entry)| entry.id == uri.host() || entry.alias == uri.host())?;
    let requester_install = match requester {
        Requester::ExtensionOwned { install } => {
            installs.get_key_value(install).map(|(key, _)| *key)
        }
        Requester::Tab {
            committed_top_level,
            extension_install,
        } => extension_install.filter(|key| {
            installs.get(key).is_some_and(|entry| {
                committed_top_level.scheme() == "chrome-extension"
                    && committed_top_level.port().is_none()
                    && committed_top_level.username().is_empty()
                    && committed_top_level.password().is_none()
                    && committed_top_level
                        .host_str()
                        .is_some_and(|host| host == entry.id || host == entry.alias)
            })
        }),
    };
    if uri.resource().is_reserved() {
        return matches!(requester, Requester::ExtensionOwned { .. })
            .then_some(())
            .filter(|()| requester_install == Some(install))
            .map(|()| (install, entry, false));
    }
    if requester_install == Some(install) {
        return Some((install, entry, false));
    }
    if uri.resource().is_favicon() {
        return None;
    }
    let allowed = entry.war.iter().any(|war| {
        if war.dynamic && uri.host() != entry.alias {
            return false;
        }
        if !war
            .resources
            .iter()
            .any(|pattern| star_matches(pattern, uri.resource().as_str()))
        {
            return false;
        }
        if let Some(source) = requester_install.and_then(|key| installs.get(&key)) {
            return war
                .extension_ids
                .iter()
                .any(|id| id == "*" || id == &source.id);
        }
        matches!(requester, Requester::Tab { committed_top_level, extension_install: None }
            if matches!(committed_top_level.scheme(), "http" | "https")
                && war.matches.iter().any(|pattern| pattern.matches_url(committed_top_level)))
    });
    allowed.then_some((install, entry, true))
}

struct WarEntry {
    resources: Vec<String>,
    matches: Vec<HostPattern>,
    extension_ids: Vec<String>,
    dynamic: bool,
}
impl WarEntry {
    fn parse(manifest: &Manifest) -> Result<Vec<Self>, SchemeError> {
        let Some(value) = manifest.web_accessible_resources() else {
            return Ok(Vec::new());
        };
        let entries = value
            .as_array()
            .filter(|entries| entries.len() <= MAX_WAR_ENTRIES)
            .ok_or(SchemeError::InvalidManifest)?;
        let mut total = 0;
        entries
            .iter()
            .map(|entry| {
                let object = entry.as_object().ok_or(SchemeError::InvalidManifest)?;
                let list = |key: &str| -> Result<Vec<String>, SchemeError> {
                    match object.get(key) {
                        None => Ok(Vec::new()),
                        Some(value) => value
                            .as_array()
                            .filter(|values| values.len() <= MAX_WAR_VALUES)
                            .ok_or(SchemeError::InvalidManifest)?
                            .iter()
                            .map(|value| {
                                value
                                    .as_str()
                                    .filter(|s| {
                                        !s.is_empty()
                                            && s.len() <= 4096
                                            && !s.chars().any(char::is_control)
                                    })
                                    .map(str::to_owned)
                                    .ok_or(SchemeError::InvalidManifest)
                            })
                            .collect(),
                    }
                };
                let mut resources = list("resources")?;
                let origins = list("matches")?;
                let extension_ids = list("extension_ids")?;
                total += resources.len() + origins.len() + extension_ids.len();
                if total > MAX_WAR_VALUES
                    || resources.is_empty()
                    || (origins.is_empty() && extension_ids.is_empty())
                {
                    return Err(SchemeError::InvalidManifest);
                }
                for resource in &mut resources {
                    *resource = resource.trim_start_matches('/').to_owned();
                    if resource.is_empty()
                        || resource.contains(['\\', '?', '#', '%'])
                        || resource
                            .split('/')
                            .any(|part| matches!(part, ".." | "." | ""))
                    {
                        return Err(SchemeError::InvalidManifest);
                    }
                }
                let matches = origins
                    .iter()
                    .map(|origin| {
                        if origin != "<all_urls>"
                            && !origin.split_once("://").is_some_and(|(_, rest)| {
                                rest.find('/').is_some_and(|at| &rest[at..] == "/*")
                            })
                        {
                            return Err(SchemeError::InvalidManifest);
                        }
                        HostPattern::parse(origin).map_err(|_| SchemeError::InvalidManifest)
                    })
                    .collect::<Result<_, _>>()?;
                if extension_ids.iter().any(|id| id != "*" && !valid_id(id))
                    || (extension_ids.iter().any(|id| id == "*") && extension_ids.len() != 1)
                {
                    return Err(SchemeError::InvalidManifest);
                }
                let dynamic = match object.get("use_dynamic_url") {
                    None => false,
                    Some(Value::Bool(value)) => *value,
                    _ => return Err(SchemeError::InvalidManifest),
                };
                Ok(Self {
                    resources,
                    matches,
                    extension_ids,
                    dynamic,
                })
            })
            .collect()
    }
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
mod native_tests;
#[cfg(test)]
mod tests;
