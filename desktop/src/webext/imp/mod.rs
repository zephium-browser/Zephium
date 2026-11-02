//! The shared desktop implementation: the registry on disk and handing extensions
//! to the shell.

mod install;
mod response;
mod updates;

pub(in crate::webext) use install::{
    choose_file, confirm, discard, prepare, prepare_file, review_update, take_pending,
};
pub(in crate::webext) use updates::start_updates;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use zephium_app::{Handle, WebExtensionStatus};
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_core::ports::engine::WebExtensionLoad;
use zephium_webext::limits::{self, Platform};
use zephium_webext::manifest::Manifest;
use zephium_webext::{archive, crx, permissions, ExtensionId};

use super::{Access, Entry, Original, Registry, WebExtensionView, WebExtensions};

// Publication, registry joins and pruning share one process-wide lease,
// including the background update coordinator's WebExtensions instance.
static PUBLICATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
static RESTORE_FINISHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RETRYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PREPARING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DEFER_PRUNE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct NativePreparation {
    baseline: Vec<u8>,
    loads: Vec<WebExtensionLoad>,
    failures: Vec<(ExtensionInstallId, String)>,
    changed: bool,
    // Prevent pruning the newly published tree until registry/native admission.
    _preparing: Preparing,
    retry: Option<ExtensionInstallId>,
}

struct RestoreFinished;
impl Drop for RestoreFinished {
    fn drop(&mut self) {
        RESTORE_FINISHED.store(true, std::sync::atomic::Ordering::Release);
    }
}

struct Retrying;
impl Drop for Retrying {
    fn drop(&mut self) {
        RETRYING.store(false, std::sync::atomic::Ordering::Release);
    }
}

struct Preparing;
impl Drop for Preparing {
    fn drop(&mut self) {
        PREPARING.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;

mod platform;
use platform::{compat_revision, prepare_package};

impl WebExtensions {
    fn packages(&self, id: &str) -> PathBuf {
        self.root.join("packages").join(id)
    }

    fn registry_path(&self, profile: ProfileId) -> PathBuf {
        self.root.join("profiles").join(format!("{profile}.json"))
    }

    fn registry(&self, profile: ProfileId) -> Registry {
        std::fs::read(self.registry_path(profile))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    #[cfg(test)]
    fn save(&self, profile: ProfileId, registry: &Registry) -> Result<(), String> {
        let _registry_io = zephium_core::extensions::REGISTRY_IO
            .lock()
            .map_err(|_| "Extension state is unavailable.")?;
        self.save_unlocked(profile, registry)
    }

    fn save_new(
        &self,
        profile: ProfileId,
        registry: &Registry,
        ticket: &zephium_core::extensions::ExtensionPublicationTicket,
    ) -> Result<(), String> {
        let root =
            std::fs::canonicalize(&self.root).map_err(|_| "The extension profile was removed.")?;
        let _registry_io = zephium_core::extensions::REGISTRY_IO
            .lock()
            .map_err(|_| "Extension state is unavailable.")?;
        if !ticket.allows(&root, profile) {
            return Err("The extension profile was removed.".into());
        }
        self.save_unlocked(profile, registry)
    }

    fn save_existing(&self, profile: ProfileId, registry: &Registry) -> Result<(), String> {
        if self.save_holder(profile, registry)? {
            Ok(())
        } else {
            Err("The extension profile was removed.".into())
        }
    }

    fn save_holder(&self, profile: ProfileId, registry: &Registry) -> Result<bool, String> {
        let _registry_io = zephium_core::extensions::REGISTRY_IO
            .lock()
            .map_err(|_| "Extension state is unavailable.")?;
        match std::fs::metadata(self.registry_path(profile)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
            Ok(metadata) if !metadata.is_file() => {
                return Err("Extension registry is unavailable.".into())
            }
            Ok(_) => {}
        }
        self.save_unlocked(profile, registry)?;
        Ok(true)
    }

    fn save_unlocked(&self, profile: ProfileId, registry: &Registry) -> Result<(), String> {
        let path = self.registry_path(profile);
        let parent = path.parent().ok_or("invalid registry path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(registry).map_err(|e| e.to_string())?;
        std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
    }

    fn profiles(&self) -> Vec<ProfileId> {
        std::fs::read_dir(self.root.join("profiles"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                ProfileId::parse(name.strip_suffix(".json")?)
            })
            .collect()
    }

    /// Hands a profile's enabled extensions to the shell, rebuilding any
    /// package prepared by an older compatibility layer.
    fn load(
        &self,
        entry: &Entry,
        install: ExtensionInstallId,
        start_background: bool,
    ) -> WebExtensionLoad {
        WebExtensionLoad {
            install,
            extension_id: entry.id.clone(),
            root: self.packages(&entry.id).join(&entry.package),
            permissions: entry.permissions.clone(),
            external_native_messaging:
                zephium_webext::permissions::external_native_messaging_allowed(
                    &entry.reviewed_permissions,
                    entry.publisher_verified,
                ) || (entry.publisher_verified && entry.native_messaging_granted),
            native_messaging_grantable: entry.publisher_verified,
            match_patterns: granted_sites(entry),
            start_background,
        }
    }

    /// Deletes package files no registry refers to: removed extensions,
    /// which are erased by the browser before their files go, and builds
    /// replaced by updates or compatibility-layer changes.
    fn prune(&self) {
        let root = self.root.clone();
        std::thread::spawn(move || {
            // Catalog icons ship with the frame now; older builds cached them here.
            let _ = std::fs::remove_dir_all(root.join("catalog-icons"));
            let Ok(_publication) = PUBLICATION.lock() else {
                return;
            };
            if PREPARING.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                return;
            }
            if DEFER_PRUNE.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let extensions = WebExtensions {
                root,
                state: std::sync::Mutex::new(None),
            };
            let mut kept: HashMap<String, Vec<String>> = HashMap::new();
            for profile in extensions.profiles() {
                for entry in extensions.registry(profile).extensions {
                    let files = kept.entry(entry.id.clone()).or_default();
                    files.push(entry.package.clone());
                    let source = if entry.source.is_empty() {
                        &entry.version
                    } else {
                        &entry.source
                    };
                    for original in ["crx", "zip", "src"] {
                        files.push(format!("{source}.{original}"));
                    }
                }
            }
            prune_packages(&extensions.root.join("packages"), &kept);
        });
    }

    fn apply(&self, shell: &Handle, profile: ProfileId, registry: &mut Registry) {
        let callback = shell.callback_handle();
        let _ = self.apply_with(
            profile,
            registry,
            || callback.is_accepting_commands(),
            |command| callback.dispatch(command),
        );
    }

    fn apply_with(
        &self,
        profile: ProfileId,
        registry: &mut Registry,
        active: impl Fn() -> bool,
        dispatch: impl FnOnce(zephium_app::Command) -> bool,
    ) -> bool {
        if !active() {
            return false;
        }
        let Some(prepared) = self.prepare_profile(profile, registry, None) else {
            return false;
        };
        self.commit_prepared(profile, registry, prepared, active, dispatch)
    }

    fn prepare_profile(
        &self,
        profile: ProfileId,
        registry: &mut Registry,
        expected_install: Option<ExtensionInstallId>,
    ) -> Option<NativePreparation> {
        let preparing = {
            let _publication = PUBLICATION.lock().ok()?;
            PREPARING
                .fetch_update(
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                    |count| count.checked_add(1),
                )
                .ok()?;
            Preparing
        };
        let baseline = std::fs::read(self.registry_path(profile)).ok()?;
        *registry = serde_json::from_slice(&baseline).ok()?;
        if expected_install.is_some_and(|install| {
            !registry.extensions.iter().any(|entry| {
                entry.enabled && ExtensionInstallId::parse(&entry.install) == Some(install)
            })
        }) {
            return None;
        }
        let (loads, failures, changed) = self.native_loads(profile, registry);
        Some(NativePreparation {
            baseline,
            loads,
            failures,
            changed,
            _preparing: preparing,
            retry: expected_install,
        })
    }

    fn commit_prepared(
        &self,
        profile: ProfileId,
        registry: &Registry,
        prepared: NativePreparation,
        active: impl Fn() -> bool,
        dispatch: impl FnOnce(zephium_app::Command) -> bool,
    ) -> bool {
        // Preparation owns no profile lifetime. A delete or a newer grant,
        // disable or uninstall wins while archives are rebuilt off-thread.
        let Ok(_publication) = PUBLICATION.lock() else {
            return false;
        };
        let Ok(_registry_io) = zephium_core::extensions::REGISTRY_IO.lock() else {
            return false;
        };
        if !active()
            || std::fs::read(self.registry_path(profile)).ok().as_deref()
                != Some(prepared.baseline.as_slice())
        {
            return false;
        }
        if prepared.changed && self.save_unlocked(profile, registry).is_err() {
            eprintln!("extensions: could not commit prepared packages; cache cleanup deferred");
            DEFER_PRUNE.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        drop(_registry_io);
        dispatch(zephium_app::Command::SetWebExtensions {
            profile,
            extensions: prepared.loads,
            failures: prepared.failures,
            retry: prepared.retry,
        })
    }

    fn native_loads(
        &self,
        profile: ProfileId,
        registry: &mut Registry,
    ) -> (
        Vec<WebExtensionLoad>,
        Vec<(ExtensionInstallId, String)>,
        bool,
    ) {
        let mut changed = false;
        let mut loads = Vec::new();
        let mut failures = Vec::new();
        for entry in registry.extensions.iter_mut() {
            // Disabled packages are prepared with their current grants when
            // explicitly enabled, rather than during startup.
            if !entry.enabled {
                continue;
            }
            let revision = compat_revision(&entry.access);
            if entry.compat != revision
                || entry.source.is_empty()
                || entry.prepared_digest.is_empty()
            {
                match self.rebuild(profile, entry, &revision) {
                    Ok(()) => changed = true,
                    Err(error) => {
                        eprintln!("extensions: could not rebuild {}: {error}", entry.id);
                        if failures.len()
                            < zephium_core::extensions::MAX_EXTENSION_INSTALLS_PER_PROFILE
                        {
                            if let Some(install) = ExtensionInstallId::parse(&entry.install) {
                                failures.push((install, "The extension package could not be prepared. Retry, or reinstall it from its original source.".into()));
                            }
                        }
                        continue;
                    }
                }
            }
            if !entry.enabled {
                continue;
            }
            let Some(install) = ExtensionInstallId::parse(&entry.install) else {
                continue;
            };
            let start_background = entry.started != revision;
            if start_background {
                entry.started = revision.clone();
                changed = true;
            }
            loads.push(self.load(entry, install, start_background));
        }
        (loads, failures, changed)
    }

    fn rebuild(
        &self,
        _profile: ProfileId,
        entry: &mut Entry,
        revision: &str,
    ) -> Result<(), String> {
        use zephium_webext::package::{publish_directory, tree_digest};
        let packages = self.packages(&entry.id);
        let key = if entry.source.is_empty() {
            &entry.version
        } else {
            &entry.source
        };
        let original_path = |kind: &str| packages.join(format!("{key}.{kind}"));
        let original = if original_path("crx").is_file() {
            Original::Crx(std::fs::read(original_path("crx")).map_err(|e| e.to_string())?)
        } else if original_path("zip").is_file() {
            Original::Zip(std::fs::read(original_path("zip")).map_err(|e| e.to_string())?)
        } else {
            Original::Folder(original_path("src"))
        };
        if !entry.sideloaded && !matches!(original, Original::Crx(_)) {
            return Err(
                "The verified Store package is unavailable; review its signed update again.".into(),
            );
        }
        let source = install::original_identity(&original)?;
        if !entry.source.is_empty() && entry.source != source {
            return Err("The original package identity changed.".into());
        }
        // A package rebuild must not recreate a deleted profile's staging.
        // TempDir also cleans abandoned/failed work without a profile lease.
        std::fs::create_dir_all(&packages).map_err(|error| error.to_string())?;
        let temporary = tempfile::Builder::new()
            .prefix(".rebuild-")
            .tempdir_in(&packages)
            .map_err(|error| error.to_string())?;
        let staged = temporary.path().join("package");
        let result = (|| {
            let limits = archive::Limits::default();
            match &original {
                Original::Crx(bytes) => {
                    let expected = ExtensionId::parse(&entry.id).ok_or("invalid extension id")?;
                    let verified =
                        crx::verify(bytes, Some(&expected)).map_err(|e| e.to_string())?;
                    archive::extract(verified.zip, &staged, &limits).map_err(|e| e.to_string())?;
                    platform::record_signed_key(&staged, verified.public_key)?;
                }
                Original::Zip(bytes) => {
                    archive::extract(bytes, &staged, &limits).map_err(|e| e.to_string())?;
                }
                Original::Folder(path) => {
                    archive::copy_dir(path, &staged, &limits).map_err(|e| e.to_string())?;
                }
            }
            let manifest = Manifest::load(&staged).map_err(|error| error.to_string())?;
            if manifest.version() != Some(entry.version.as_str()) {
                return Err("The original package version changed.".into());
            }
            let reviewed_permissions = (!entry.sideloaded).then(|| {
                manifest
                    .permissions()
                    .into_iter()
                    .filter(|permission| entry.permissions.contains(permission))
                    .collect::<Vec<_>>()
            });
            prepare_package(&staged, &entry.access)?;
            let prepared_digest = tree_digest(&staged).map_err(|e| e.to_string())?;
            let package =
                zephium_webext::package::publication_identity(&source, revision, &prepared_digest);
            install::keep_original(&packages, &source, &original)?;
            publish_directory(&staged, &packages.join(&package)).map_err(|e| e.to_string())?;
            entry.package = package;
            entry.source = source;
            entry.prepared_digest = prepared_digest;
            entry.compat = revision.to_owned();
            if let Some(reviewed_permissions) = reviewed_permissions {
                entry.reviewed_permissions = reviewed_permissions;
                entry.publisher_verified = true;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(staged);
        }
        result
    }
}

/// The sites an extension is granted under the user's access choice.
fn granted_sites(entry: &Entry) -> Vec<String> {
    match &entry.access {
        Access::All => entry.hosts.clone(),
        Access::Click => Vec::new(),
        Access::Sites { sites } => sites
            .iter()
            .flat_map(|site| [format!("*://{site}/*"), format!("*://*.{site}/*")])
            .collect(),
    }
}

/// A site as the user typed it, reduced to its host: `https://www.x.com/a`
/// becomes `www.x.com`.
fn site_host(site: &str) -> Option<String> {
    let site = site.trim();
    let site = site.split_once("://").map_or(site, |(_, rest)| rest);
    let host = site.split(['/', '?', '#']).next()?.split(':').next()?;
    let host = host.trim_start_matches("*.").to_ascii_lowercase();
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    valid.then_some(host)
}

#[cfg(test)]
#[test]
fn sites_reduce_to_hosts() {
    assert_eq!(
        site_host("https://www.github.com/a?b").as_deref(),
        Some("www.github.com")
    );
    assert_eq!(site_host("*.example.com").as_deref(), Some("example.com"));
    assert_eq!(site_host("localhost:8080").as_deref(), Some("localhost"));
    assert_eq!(site_host("  "), None);
    assert_eq!(site_host("a b.com"), None);
}

fn prune_packages(packages: &Path, kept: &HashMap<String, Vec<String>>) {
    for extension in std::fs::read_dir(packages).into_iter().flatten().flatten() {
        let name = extension.file_name().to_string_lossy().into_owned();
        let Some(files) = kept.get(&name) else {
            let _ = std::fs::remove_dir_all(extension.path());
            continue;
        };
        for file in std::fs::read_dir(extension.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if files.iter().any(|kept| file.file_name() == kept.as_str()) {
                continue;
            }
            let path = file.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
        }
    }
}

#[cfg(test)]
#[test]
fn pruning_keeps_only_the_packages_registries_use() {
    let packages = tempfile::tempdir().unwrap();
    for path in ["kept/1.2-aaaa", "kept/1.1-aaaa", "removed/1.0-aaaa"] {
        std::fs::create_dir_all(packages.path().join(path)).unwrap();
    }
    for path in ["kept/1.2.crx", "kept/1.1.crx", "removed/1.0.crx"] {
        std::fs::write(packages.path().join(path), b"crx").unwrap();
    }
    let kept = HashMap::from([(
        "kept".to_string(),
        vec!["1.2-aaaa".to_string(), "1.2.crx".to_string()],
    )]);
    prune_packages(packages.path(), &kept);
    let mut left: Vec<_> = std::fs::read_dir(packages.path().join("kept"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["1.2-aaaa", "1.2.crx"]);
    assert!(!packages.path().join("removed").exists());
}

/// How many extensions are on and how many are installed, over every profile.
pub(in crate::webext) fn counts(extensions: &WebExtensions) -> (usize, usize) {
    extensions
        .profiles()
        .into_iter()
        .flat_map(|profile| extensions.registry(profile).extensions)
        .fold((0, 0), |(enabled, total), entry| {
            (enabled + usize::from(entry.enabled), total + 1)
        })
}

pub(in crate::webext) fn restore(extensions: &WebExtensions, shell: &zephium_app::CallbackHandle) {
    let _finished = RestoreFinished;
    for profile in extensions.profiles() {
        if !shell.is_accepting_commands() {
            break;
        }
        let mut registry = Registry::default();
        extensions.apply_with(
            profile,
            &mut registry,
            || shell.is_accepting_commands(),
            |command| shell.dispatch(command),
        );
    }
    extensions.prune();
}

pub(in crate::webext) async fn target(
    shell: &Handle,
    tab: Option<ItemId>,
) -> Result<zephium_app::WebExtensionTarget, String> {
    let receiver = shell.web_extension_target(tab);
    tauri::async_runtime::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2)))
        .await
        .map_err(|_| "The browser is unavailable.")?
        .map_err(|_| "The browser did not respond.")?
        .ok_or_else(|| "Extensions can't be installed in this window.".to_owned())
}

pub(in crate::webext) fn publication_ticket(
    extensions: &WebExtensions,
    profile: ProfileId,
) -> Result<zephium_core::extensions::ExtensionPublicationTicket, String> {
    let root = std::fs::canonicalize(&extensions.root)
        .map_err(|_| "The extension profile was removed.")?;
    zephium_core::extensions::REGISTRY_IO
        .lock()
        .map_err(|_| "Extension state is unavailable.")?
        .register(root, profile)
        .ok_or_else(|| "Too many extension installations are pending.".into())
}

pub(in crate::webext) async fn list(
    shell: &Handle,
    extensions: &WebExtensions,
) -> Result<Vec<WebExtensionView>, String> {
    // Before the session exists there is nothing to list yet; the build
    // still supports extensions.
    let Ok(profile) = target(shell, None).await.map(|target| target.profile) else {
        return Ok(Vec::new());
    };
    // Sample before enqueueing the status read: a read ordered before startup
    // delivery still says starting and follows the existing refresh schedule.
    let restore_finished = RESTORE_FINISHED.load(std::sync::atomic::Ordering::Acquire);
    let receiver = shell.web_extension_status(profile);
    let status: HashMap<String, WebExtensionStatus> =
        tauri::async_runtime::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(2)))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|(install, status)| (install.to_string(), status))
            .collect();
    let root = extensions.root.clone();
    Ok(extensions
        .registry(profile)
        .extensions
        .into_iter()
        .map(|entry| {
            let (state, error) =
                extension_state(entry.enabled, status.get(&entry.install), restore_finished);
            let manifest =
                Manifest::load(&root.join("packages").join(&entry.id).join(&entry.package)).ok();
            let warnings = manifest
                .as_ref()
                .map(permissions::warnings)
                .unwrap_or_default();
            let limitations = manifest
                .as_ref()
                .map(|manifest| {
                    limits::limitations(manifest, &entry.reviewed_permissions, platform())
                })
                .unwrap_or_default();
            let has_options = manifest.as_ref().is_some_and(|manifest| {
                let raw = manifest.raw();
                raw.get("options_page").is_some()
                    || raw
                        .get("options_ui")
                        .and_then(|ui| ui.get("page"))
                        .is_some()
            });
            let (access, sites) = match &entry.access {
                Access::All => ("all", Vec::new()),
                Access::Click => ("click", Vec::new()),
                Access::Sites { sites } => ("sites", sites.clone()),
            };
            WebExtensionView {
                profile_id: profile.to_string(),
                install_id: entry.install,
                id: entry.id,
                name: entry.name,
                version: entry.version,
                description: entry.description,
                enabled: entry.enabled,
                icon: entry.icon,
                state: state.to_owned(),
                error,
                warnings,
                limitations,
                access: access.to_owned(),
                sites,
                site_scoped: !entry.hosts.is_empty(),
                has_options,
                held_update: entry.held_update,
                sideloaded: entry.sideloaded,
                pinned: entry.pinned,
            }
        })
        .collect())
}

fn platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Macos
    }
}

fn extension_state(
    enabled: bool,
    status: Option<&WebExtensionStatus>,
    restore_finished: bool,
) -> (&'static str, Option<String>) {
    match (enabled, status) {
        (false, _) => ("off", None),
        (true, Some(WebExtensionStatus::Running(_))) => ("running", None),
        (true, Some(WebExtensionStatus::Failed(error))) => ("failed", Some(error.clone())),
        (true, None) if restore_finished => (
            "failed",
            Some("The browser could not start this extension. Retry to start it again.".into()),
        ),
        (true, _) => ("starting", None),
    }
}

enum RegistryMutation {
    Enabled { id: String, enabled: bool },
    Pinned { id: String, pinned: bool },
    Access { id: String, access: Access },
    Uninstall { id: String },
}

async fn publish_mutation(
    root: PathBuf,
    profile: ProfileId,
    mutation: RegistryMutation,
    active: impl Fn() -> bool + Send + 'static,
    dispatch: impl Fn(zephium_app::Command) -> bool + Send + 'static,
) -> Result<(), String> {
    crate::webext::publication(move || execute_mutation(root, profile, mutation, active, dispatch))
        .await
}

fn execute_mutation(
    root: PathBuf,
    profile: ProfileId,
    mutation: RegistryMutation,
    active: impl Fn() -> bool,
    dispatch: impl Fn(zephium_app::Command) -> bool,
) -> Result<(), String> {
    let extensions = WebExtensions {
        root,
        state: Default::default(),
    };
    let publication = PUBLICATION
        .lock()
        .map_err(|_| "Extension state is unavailable.")?;
    if !active() {
        return Err("The browser is unavailable.".into());
    }
    let mut registry = extensions.registry(profile);
    let id = match &mutation {
        RegistryMutation::Enabled { id, .. }
        | RegistryMutation::Pinned { id, .. }
        | RegistryMutation::Access { id, .. }
        | RegistryMutation::Uninstall { id } => id,
    };
    let position = registry
        .extensions
        .iter()
        .position(|entry| &entry.id == id)
        .ok_or("That extension isn't installed.")?;
    let removed = match mutation {
        RegistryMutation::Enabled { enabled, .. } => {
            registry.extensions[position].enabled = enabled;
            None
        }
        RegistryMutation::Access { access, .. } => {
            registry.extensions[position].access = access;
            None
        }
        RegistryMutation::Pinned { pinned, .. } => {
            // Where its button sits is the frame's business; the runtime
            // has nothing to reload.
            registry.extensions[position].pinned = pinned;
            return extensions.save_existing(profile, &registry);
        }
        RegistryMutation::Uninstall { .. } => Some(registry.extensions.remove(position)),
    };
    extensions.save_existing(profile, &registry)?;
    if let Some(entry) = removed {
        // Keep packages until the next launch: native may need to load one to erase its data.
        if let Some(install) = ExtensionInstallId::parse(&entry.install) {
            dispatch(zephium_app::Command::RemoveWebExtension {
                profile,
                extension: Box::new(extensions.load(&entry, install, false)),
            });
        }
    }
    drop(publication);
    extensions.apply_with(profile, &mut registry, active, dispatch);
    Ok(())
}

async fn mutate_profile(
    shell: &Handle,
    extensions: &WebExtensions,
    profile: ProfileId,
    mutation: RegistryMutation,
) -> Result<(), String> {
    let active = shell.callback_handle();
    let dispatch = active.clone();
    publish_mutation(
        extensions.root.clone(),
        profile,
        mutation,
        move || active.is_accepting_commands(),
        move |command| dispatch.dispatch(command),
    )
    .await
}

pub(in crate::webext) async fn set_enabled(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let profile = target(shell, None).await?.profile;
    mutate_profile(
        shell,
        extensions,
        profile,
        RegistryMutation::Enabled {
            id: id.to_owned(),
            enabled,
        },
    )
    .await
}

pub(in crate::webext) async fn set_pinned(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
    pinned: bool,
) -> Result<(), String> {
    let profile = target(shell, None).await?.profile;
    mutate_profile(
        shell,
        extensions,
        profile,
        RegistryMutation::Pinned {
            id: id.to_owned(),
            pinned,
        },
    )
    .await
}

pub(in crate::webext) async fn retry(
    shell: &Handle,
    extensions: &WebExtensions,
    profile: ProfileId,
    install: ExtensionInstallId,
) -> Result<(), String> {
    RETRYING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .map_err(|_| "Another extension retry is still running. Try again shortly.")?;
    let retrying = Retrying;
    if target(shell, None).await?.profile != profile {
        return Err("The extension profile changed. Open Extensions again.".into());
    }
    let root = extensions.root.clone();
    let callback = shell.callback_handle();
    crate::webext::publication(move || {
        let _retrying = retrying;
        if !callback.is_accepting_commands() {
            return Err("The browser is unavailable.".into());
        }
        let extensions = WebExtensions {
            root,
            state: Default::default(),
        };
        let mut registry = Registry::default();
        let prepared = extensions
            .prepare_profile(profile, &mut registry, Some(install))
            .ok_or("The installed extension changed. Open Extensions again.")?;
        if !extensions.commit_prepared(
            profile,
            &registry,
            prepared,
            || callback.is_accepting_commands(),
            |command| callback.dispatch(command),
        ) {
            return Err("The installed extension changed. Open Extensions again.".into());
        }
        Ok(())
    })
    .await
}

pub(in crate::webext) async fn set_access(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
    mode: &str,
    sites: Vec<String>,
) -> Result<(), String> {
    let access = match mode {
        "all" => Access::All,
        "click" if !cfg!(target_os = "windows") => Access::Click,
        "sites" => {
            let mut sites: Vec<String> = sites.iter().filter_map(|site| site_host(site)).collect();
            sites.sort();
            sites.dedup();
            Access::Sites { sites }
        }
        _ => return Err("Unknown site access.".into()),
    };
    let profile = target(shell, None).await?.profile;
    mutate_profile(
        shell,
        extensions,
        profile,
        RegistryMutation::Access {
            id: id.to_owned(),
            access,
        },
    )
    .await
}

pub(in crate::webext) async fn open_options(shell: &Handle, id: &str) -> Result<(), String> {
    let profile = target(shell, None).await?.profile;
    shell.dispatch(zephium_app::Command::OpenWebExtensionOptions {
        profile,
        extension_id: id.to_owned(),
    });
    Ok(())
}

pub(in crate::webext) fn answer_access(
    shell: &Handle,
    extensions: &WebExtensions,
    request: zephium_ipc::WebExtensionAccessRequestView,
    allowed: bool,
) -> Result<(), String> {
    let profile = ProfileId::parse(&request.profile_id).ok_or("Invalid profile.")?;
    let number: u64 = request.request.parse().map_err(|_| "Invalid request.")?;
    if allowed {
        let _publication = PUBLICATION
            .lock()
            .map_err(|_| "Extension state is unavailable.")?;
        let mut registry = extensions.registry(profile);
        if let Some(entry) = registry
            .extensions
            .iter_mut()
            .find(|entry| entry.id == request.extension_id)
        {
            if request.permissions.iter().any(|p| p == "nativeMessaging") {
                entry.native_messaging_granted = entry.publisher_verified;
            }
            for permission in request.permissions {
                if !entry.permissions.contains(&permission) {
                    entry.permissions.push(permission);
                }
            }
            for pattern in request.patterns {
                if !entry.hosts.contains(&pattern) {
                    entry.hosts.push(pattern);
                }
            }
            extensions.save_existing(profile, &registry)?;
        }
    }
    shell.dispatch(zephium_app::Command::AnswerWebExtensionAccess {
        profile,
        request: number,
        allowed,
    });
    Ok(())
}

pub(in crate::webext) fn access_removed(
    extensions: &WebExtensions,
    removal: zephium_ipc::WebExtensionAccessRemovalView,
) -> Result<(), String> {
    let profile = ProfileId::parse(&removal.profile_id).ok_or("Invalid profile.")?;
    let _publication = PUBLICATION
        .lock()
        .map_err(|_| "Extension state is unavailable.")?;
    let mut registry = extensions.registry(profile);
    let Some(entry) = registry
        .extensions
        .iter_mut()
        .find(|entry| entry.id == removal.extension_id)
    else {
        return Ok(());
    };
    // Access the person accepted at install stays; only what was granted
    // later can be given up.
    let native = entry.native_messaging_granted
        && removal.permissions.iter().any(|p| p == "nativeMessaging");
    entry.native_messaging_granted &= !native;
    let reviewed = |permission: &String| {
        entry.reviewed_permissions.contains(permission) || permission == "nativeMessaging"
    };
    let before = (entry.permissions.len(), entry.hosts.len());
    entry
        .permissions
        .retain(|permission| reviewed(permission) || !removal.permissions.contains(permission));
    entry.hosts.retain(|host| !removal.patterns.contains(host));
    if !native && before == (entry.permissions.len(), entry.hosts.len()) {
        return Ok(());
    }
    extensions.save_existing(profile, &registry)
}

pub(in crate::webext) async fn uninstall(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
) -> Result<(), String> {
    let profile = target(shell, None).await?.profile;
    mutate_profile(
        shell,
        extensions,
        profile,
        RegistryMutation::Uninstall { id: id.to_owned() },
    )
    .await
}

#[cfg(test)]
#[test]
fn hardening_rejected_startup_delivery_settles_as_failed_instead_of_starting_forever() {
    assert_eq!(extension_state(true, None, false).0, "starting");
    let (state, error) = extension_state(true, None, true);
    assert_eq!(state, "failed");
    assert!(error.unwrap().contains("Retry"));
    assert_eq!(
        extension_state(true, Some(&WebExtensionStatus::Loading), true).0,
        "starting"
    );
    assert_eq!(extension_state(false, None, true), ("off", None));
}

#[cfg(test)]
#[tokio::test(flavor = "current_thread")]
async fn registry_mutations_leave_the_async_executor_responsive() {
    for mutation in [
        RegistryMutation::Enabled {
            id: "missing".into(),
            enabled: false,
        },
        RegistryMutation::Access {
            id: "missing".into(),
            access: Access::Click,
        },
        RegistryMutation::Uninstall {
            id: "missing".into(),
        },
    ] {
        let (ready, held) = std::sync::mpsc::channel();
        let (progress, observed) = std::sync::mpsc::channel();
        let supervisor = std::thread::spawn(move || {
            let lock = PUBLICATION.lock().unwrap();
            ready.send(()).unwrap();
            let responsive = observed.recv_timeout(Duration::from_secs(2)).is_ok();
            drop(lock);
            responsive
        });
        held.recv_timeout(Duration::from_secs(3)).unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_owned();
        let (entered, started) = tokio::sync::oneshot::channel();
        let operation = tokio::spawn(async move {
            entered.send(()).unwrap();
            publish_mutation(path, ProfileId::generate(), mutation, || true, |_| true).await
        });
        started.await.unwrap();
        let _ = progress.send(());
        assert!(operation.await.unwrap().is_err());
        assert!(
            supervisor.join().unwrap(),
            "a profile mutation blocked the async executor on PUBLICATION"
        );
    }
}
