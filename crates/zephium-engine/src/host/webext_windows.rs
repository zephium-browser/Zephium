//! Windows owns native extension profiles and their bounded presentation views.
use crate::platform::imp::extensions as native;
#[path = "webext_windows_popup.rs"]
mod popup;

use super::resources::{NativeResourceClass, NativeResourceLease};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use webview2_com::take_pwstr;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::core::{Interface, PWSTR};
use windows::Win32::Foundation::E_ACCESSDENIED;
use wry::{WebViewBuilderExtWindows, WebViewExtWindows};
use zephium_core::extensions::{
    ExtensionActionIcon, ExtensionActionRejection, ExtensionActionRevision, ExtensionActionScope,
    ExtensionActionSnapshot, ExtensionActionSnapshotSettlement, ExtensionActionState,
    ExtensionBrowserSurfaceGeneration,
};
use zephium_core::extensions::{ExtensionActionRequest, ExtensionActionSettlement};
use zephium_core::extensions::{ExtensionRuntimeGeneration, ExtensionRuntimeInstance};
use zephium_core::ids::ItemId;
use zephium_core::ids::{ExtensionInstallId, ProfileId};
use zephium_core::ports::engine::{EngineEvent, WebExtensionLoad, WebExtensionLoaded};

struct ExtensionView {
    crash_observer: Option<crate::platform::imp::CrashObserver>,
    view: wry::WebView,
    resource: NativeResourceLease,
    alive: Rc<Cell<bool>>,
}

#[derive(Default)]
struct Action {
    recovery: HostRecovery,
    binding: Option<(ItemId, i64)>,
    state: Option<ExtensionActionState>,
    popup: Option<String>,
    native_tab: Option<i64>,
    open_request: Option<(ItemId, std::time::Instant, u8)>,
}

#[derive(Default)]
struct HostRecovery {
    started: Option<std::time::Instant>,
    attempts: usize,
    pending: bool,
}
impl HostRecovery {
    fn next(&mut self, now: std::time::Instant) -> Option<std::time::Duration> {
        if self.pending {
            return None;
        }
        if self
            .started
            .is_none_or(|at| now.saturating_duration_since(at).as_secs() >= 60)
        {
            self.started = Some(now);
            self.attempts = 0;
        }
        let millis = *[250, 1000, 4000].get(self.attempts)?;
        self.attempts += 1;
        self.pending = true;
        Some(std::time::Duration::from_millis(millis))
    }
}

/// An enabled environment cannot be confused with an ordinary cached one.
/// Failed startup stays failed for the exact native generation.
pub(super) struct Startup {
    path: PathBuf,
    environment: RefCell<Option<ICoreWebView2Environment>>,
    pub(super) initialized: Cell<bool>,
    failed: Cell<bool>,
}

impl Startup {
    pub(super) fn authenticate(
        &self,
        environment: &ICoreWebView2Environment,
        core: &ICoreWebView2,
    ) -> windows::core::Result<()> {
        if self.failed.get() {
            return Err(E_ACCESSDENIED.into());
        }
        let result = self.authenticate_inner(environment, core);
        if result.is_err() {
            eprintln!("extensions: native startup attestation failed: {result:?}");
            self.failed.set(true);
        }
        result
    }

    fn authenticate_inner(
        &self,
        environment: &ICoreWebView2Environment,
        core: &ICoreWebView2,
    ) -> windows::core::Result<()> {
        crate::platform::imp::attest_environment(environment, &self.path)
            .map_err(|_| windows::core::Error::from(E_ACCESSDENIED))?;
        let profile = native::profile(core)?;
        let mut name = PWSTR::null();
        let mut private = windows::core::BOOL::default();
        // SAFETY: retained profile and controller on their owning apartment.
        unsafe {
            profile.ProfileName(&mut name)?;
            profile.IsInPrivateModeEnabled(&mut private)?;
            let actual_environment = core.cast::<ICoreWebView2_2>()?.Environment()?;
            if !crate::platform::imp::same_environment(environment, &actual_environment) {
                return Err(E_ACCESSDENIED.into());
            }
        }
        let name = take_pwstr(name);
        // An unnamed controller uses the default profile. WebView2 reports
        // its API name as empty, although its storage folder is named Default.
        if !name.is_empty() || private.as_bool() {
            eprintln!(
                "extensions: unexpected native human profile {name:?} (private={})",
                private.as_bool()
            );
            return Err(E_ACCESSDENIED.into());
        }
        if let Some(expected) = self.environment.borrow().as_ref() {
            return if crate::platform::imp::same_environment(expected, environment) {
                Ok(())
            } else {
                Err(E_ACCESSDENIED.into())
            };
        }
        // The bootstrap is page-inert. Persisted third-party extensions must
        // await the desktop's current registry before any content view exists.
        for item in
            native::list(&profile).map_err(|_| windows::core::Error::from(E_ACCESSDENIED))?
        {
            let id = native::extension_id(&item)?;
            if !native::is_runtime_component(&id) {
                native::enable(&item, false)
                    .map_err(|_| windows::core::Error::from(E_ACCESSDENIED))?;
            }
        }
        *self.environment.borrow_mut() = Some(environment.clone());
        self.initialized.set(true);
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct WindowsExtensions {
    startups: HashMap<ProfileId, Rc<Startup>>,
    installs: HashMap<(ProfileId, ExtensionInstallId), Install>,
    generation: u64,
    popup: Option<Popup>,
    // A failed explicit child close must not destroy its native parent first.
    retained_popup_window: Option<std::rc::Weak<popup::PopupWindow>>,
    // Consume an anchor click that first deactivated the popup, so the same
    // click's later toolbar IPC dismisses instead of reopening it.
    dismissed_action: Option<(ExtensionRuntimeInstance, ItemId, std::time::Instant)>,
    navigation: HashMap<ProfileId, super::permits::ExtensionNavigationGrants>,
}

impl WindowsExtensions {
    pub(super) fn has_native_views_for_profile(&self, profile: ProfileId) -> bool {
        self.installs.keys().any(|(owner, _)| *owner == profile)
            || self
                .popup
                .as_ref()
                .is_some_and(|popup| popup.runtime.profile() == profile)
    }

    pub(super) fn navigation_grants(
        &mut self,
        profile: ProfileId,
    ) -> super::permits::ExtensionNavigationGrants {
        self.navigation.entry(profile).or_default().clone()
    }

    fn revoke_navigation(&mut self, profile: ProfileId, id: &str) {
        if let Some(grants) = self.navigation.get(&profile) {
            grants
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(id);
        }
    }
}

struct Popup {
    runtime: ExtensionRuntimeInstance,
    tab: ItemId,
    view: ExtensionView,
    window: popup::PopupWindow,
    escape: Option<popup::EscapeRegistration>,
}

struct Install {
    runtime: ExtensionRuntimeInstance,
    id: String,
    native: ICoreWebView2BrowserExtension,
    bridge: ExtensionView,
    action: Rc<RefCell<Action>>,
    options: Option<String>,
}

impl super::EngineHost {
    fn observe_extension_crashes(
        &self,
        view: &wry::WebView,
        runtime: ExtensionRuntimeInstance,
        alive: Rc<Cell<bool>>,
        popup_view: bool,
    ) -> Result<crate::platform::imp::CrashObserver, String> {
        let profile = runtime.profile();
        let observer = self
            .browser_process_exit_observers
            .get(&profile)
            .ok_or("Extension browser process observer unavailable.")?;
        let process_id = observer.expected_process_id();
        let generation = observer.generation();
        if crate::platform::imp::browser_process(view)
            .map_err(|e| e.to_string())?
            .id()
            != process_id
        {
            return Err("Extension controller process identity mismatch.".into());
        }
        crate::platform::imp::install_crash_handler(view, move |failure| {
            if !alive.get() {
                return;
            }
            match failure {
                crate::platform::imp::ProcessFailure::Browser => {
                    super::dispatch::with_profile_exit(profile, generation, move |host| {
                        host.on_profile_process_exit(profile, process_id, generation);
                    });
                }
                crate::platform::imp::ProcessFailure::Renderer => {
                    let alive = alive.clone();
                    super::dispatch::with_extension_lifecycle(runtime, popup_view, move |host| {
                        if !alive.get() {
                            return;
                        }
                        if popup_view {
                            if host
                                .windows_extensions
                                .popup
                                .as_ref()
                                .is_some_and(|p| p.runtime == runtime)
                            {
                                host.close_windows_extension_popup();
                            }
                        } else {
                            host.extension_host_crashed(runtime);
                        }
                    });
                }
            }
        })
        .map_err(|e| e.to_string())
    }

    fn extension_host_crashed(&mut self, runtime: ExtensionRuntimeInstance) {
        let Some(install) = self
            .windows_extensions
            .installs
            .get(&(runtime.profile(), runtime.install_id()))
            .filter(|install| install.runtime == runtime)
        else {
            return;
        };
        let delay = {
            let mut action = install.action.borrow_mut();
            action.state = None;
            action.popup = None;
            action.native_tab = None;
            action.open_request = None;
            action.recovery.next(std::time::Instant::now())
        };
        self.sink.emit(EngineEvent::ExtensionActionsInvalidated {
            profile: runtime.profile(),
        });
        let Some(delay) = delay else {
            return;
        };
        let dispatch = self.main_dispatch.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("extension-host-recovery".into())
            .spawn(move || {
                std::thread::sleep(delay);
                dispatch(Box::new(move || {
                    super::dispatch::with_extension_lifecycle(runtime, false, move |host| {
                        host.reload_extension_host(runtime);
                    })
                }));
            })
        {
            install.action.borrow_mut().recovery.pending = false;
            eprintln!("extensions: could not schedule host recovery: {error}");
        }
    }

    fn reload_extension_host(&mut self, runtime: ExtensionRuntimeInstance) {
        if self.windows_view_admission_blocked(runtime.profile()) {
            return;
        }
        let Some(install) = self
            .windows_extensions
            .installs
            .get(&(runtime.profile(), runtime.install_id()))
            .filter(|install| install.runtime == runtime && install.bridge.alive.get())
        else {
            return;
        };
        install.action.borrow_mut().recovery.pending = false;
        let url = format!(
            "chrome-extension://{}/{}/host.html",
            install.id,
            zephium_webext::windows::HOST_DIRECTORY
        );
        if install.bridge.view.load_url(&url).is_err() {
            self.extension_host_crashed(runtime);
        }
    }

    pub(super) fn windows_extension_profile_for_erasure(
        &self,
        profile: ProfileId,
    ) -> Option<windows::core::Result<ICoreWebView2Profile2>> {
        self.windows_extensions
            .installs
            .iter()
            .find(|((owner, _), _)| *owner == profile)
            .map(|(_, install)| {
                native::profile(&install.bridge.view.webview()).and_then(|profile| profile.cast())
            })
    }
    pub(super) fn windows_extension_startup(
        &mut self,
        profile: ProfileId,
        path: &Path,
    ) -> Result<Rc<Startup>, String> {
        if let Some(startup) = self.windows_extensions.startups.get(&profile) {
            if startup.failed.get() || startup.path != path {
                return Err("Extension environment admission is closed.".into());
            }
            return Ok(startup.clone());
        }
        if self.windows_extensions.startups.len()
            >= super::profiles::MAX_PROFILE_PERSISTENCE_BINDINGS
        {
            return Err("Extension environment capacity is exhausted.".into());
        }
        let startup = Rc::new(Startup {
            path: path.to_owned(),
            environment: RefCell::new(None),
            initialized: Cell::new(false),
            failed: Cell::new(false),
        });
        self.windows_extensions
            .startups
            .insert(profile, startup.clone());
        Ok(startup)
    }

    pub(super) fn forget_windows_extensions(&mut self, profile: ProfileId) {
        if let Some(grants) = self.windows_extensions.navigation.remove(&profile) {
            grants
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
        }
        if self
            .windows_extensions
            .popup
            .as_ref()
            .is_some_and(|popup| popup.runtime.profile() == profile)
        {
            self.close_windows_extension_popup();
        }
        let keys: Vec<_> = self
            .windows_extensions
            .installs
            .keys()
            .filter(|(owner, _)| *owner == profile)
            .copied()
            .collect();
        for key in keys {
            if let Some(install) = self.windows_extensions.installs.remove(&key) {
                install.bridge.alive.set(false);
                // Persisted enabled workers can keep WebView2's process group
                // alive after its last content controller closes. The desktop
                // registry restores the requested state on the next startup.
                if let Err(error) = native::enable(&install.native, false) {
                    eprintln!(
                        "extensions: retirement could not disable its native worker: {error}"
                    );
                    self.unverifiable_browser_processes.insert(profile);
                }
                self.close_extension_view(profile, install.bridge);
            }
        }
        self.windows_extensions.startups.remove(&profile);
    }

    pub(super) fn shutdown_windows_extensions(&mut self) {
        let profiles: Vec<_> = self.windows_extensions.startups.keys().copied().collect();
        for profile in profiles {
            self.forget_windows_extensions(profile);
        }
        self.windows_extensions.startups.clear();
    }

    pub(crate) fn load_web_extension(&mut self, profile: ProfileId, load: WebExtensionLoad) {
        let install = load.install;
        let result = self.load_windows_extension(profile, load);
        self.sink.emit(EngineEvent::WebExtensionSettled {
            profile,
            install,
            result,
        });
        self.sink
            .emit(EngineEvent::ExtensionActionsInvalidated { profile });
    }

    fn load_windows_extension(
        &mut self,
        profile: ProfileId,
        load: WebExtensionLoad,
    ) -> Result<WebExtensionLoaded, String> {
        if self.windows_view_admission_blocked(profile)
            || self.erasure_tombstones.contains(&profile)
        {
            return Err("This profile is unavailable.".into());
        }
        if self
            .profile_persistence_classes
            .get(&profile)
            .is_some_and(|class| *class == super::ProfilePersistenceClass::Ephemeral)
        {
            return Err("Extensions are unavailable in private profiles.".into());
        }
        // QA admission budget, not a WebView2 limit: five installed extensions
        // already measured ~406 MiB with one lab page. Keep observer resources
        // bounded separately from browsing/Work; reserve one slot for a popup.
        // The measured shared-view tradeoff is in docs/windows-extension-qa.md.
        if self.windows_extensions.installs.len() >= NativeResourceClass::Extension.limit() - 1
            && !self
                .windows_extensions
                .installs
                .contains_key(&(profile, load.install))
        {
            return Err(zephium_core::extensions::WINDOWS_EXTENSION_CAPACITY_MESSAGE.into());
        }
        let path = crate::erasure::prepare_profile_directory(&self.profiles_root, profile)
            .map_err(|e| e.to_string())?;
        self.ensure_windows_profile_environment_at_path(
            profile,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            path.clone(),
            true,
        )
        .map_err(|error| format!("Extension profile startup failed: {error:?}"))?;
        zephium_webext::ExtensionId::parse(&load.extension_id)
            .ok_or("Invalid extension identity.")?;
        let manifest =
            zephium_webext::manifest::Manifest::load(&load.root).map_err(|e| e.to_string())?;
        let loaded = WebExtensionLoaded {
            name: manifest.name().unwrap_or_else(|| load.extension_id.clone()),
            version: manifest.version().unwrap_or_default().to_owned(),
        };
        let next = self
            .windows_extensions
            .generation
            .checked_add(1)
            .ok_or("Extension generation exhausted.")?;
        let generation =
            ExtensionRuntimeGeneration::new(next).ok_or("Invalid extension generation.")?;
        self.windows_extensions.generation = next;
        self.unload_web_extension(profile, load.install);
        if self.windows_view_admission_blocked(profile) {
            return Err("Extension replacement could not retire the previous runtime.".into());
        }
        let runtime = ExtensionRuntimeInstance::new(profile, load.install, generation);
        let action = Rc::new(RefCell::new(Action::default()));
        // Profile objects are tied to their originating controller. Keep the
        // management controller alive for the entire native install lifetime.
        let bridge = match self.extension_view(profile, &load.extension_id, action.clone(), runtime)
        {
            Ok(bridge) => bridge,
            Err(error) => {
                if self
                    .windows_extensions
                    .startups
                    .get(&profile)
                    .is_some_and(|startup| startup.failed.get())
                {
                    self.quarantine_unverifiable_windows_profile(profile);
                }
                return Err(error);
            }
        };
        let mut rollback_failed = false;
        let result = (|| {
            let native_profile = native::profile(&bridge.view.webview())
                .map_err(|error| format!("Extension profile: {error}"))?;
            let item = native::add(&native_profile, &load.root)
                .map_err(|error| format!("AddBrowserExtension: {error}"))?;
            let result = (|| {
                if native::extension_id(&item).map_err(|e| e.to_string())? != load.extension_id {
                    return Err(
                        "Native extension identity does not match the verified package.".into(),
                    );
                }
                native::enable(&item, true)
                    .map_err(|error| format!("Enable extension: {error}"))?;
                bridge
                    .view
                    .load_url(&format!(
                        "chrome-extension://{}/{}/host.html",
                        load.extension_id,
                        zephium_webext::windows::HOST_DIRECTORY
                    ))
                    .map_err(|error| error.to_string())
            })();
            if let Err(error) = result {
                rollback_failed = native::enable(&item, false).is_err();
                return Err(error);
            }
            Ok(item)
        })();
        let item = match result {
            Ok(item) => item,
            Err(error) => {
                self.close_extension_view(profile, bridge);
                // Package rejection, ID mismatch and add/enable errors belong
                // to this install (Failed/Retry), not the browsing profile.
                // Quarantine only if startup attestation lost the exact native
                // environment/profile/private identity, or rollback cannot
                // prove a partially loaded extension disabled. Failed native
                // view closure separately retains its cleanup-debt lease.
                if rollback_failed
                    || self
                        .windows_extensions
                        .startups
                        .get(&profile)
                        .is_some_and(|startup| startup.failed.get())
                {
                    self.quarantine_unverifiable_windows_profile(profile);
                }
                return Err(error);
            }
        };
        self.windows_extensions
            .navigation_grants(profile)
            .lock()
            .map_err(|_| "Extension navigation grants are unavailable.")?
            .insert(load.extension_id.clone());
        let options = manifest
            .raw()
            .get("options_page")
            .and_then(Value::as_str)
            .or_else(|| {
                manifest
                    .raw()
                    .pointer("/options_ui/page")
                    .and_then(Value::as_str)
            })
            .and_then(|path| {
                valid_extension_url(
                    &runtime,
                    &format!("chrome-extension://{}/", load.extension_id),
                    path,
                )
            });
        self.windows_extensions.installs.insert(
            (profile, load.install),
            Install {
                runtime,
                id: load.extension_id,
                native: item,
                bridge,
                action,
                options,
            },
        );
        Ok(loaded)
    }

    pub(crate) fn unload_web_extension(&mut self, profile: ProfileId, install: ExtensionInstallId) {
        if self.windows_extensions.popup.as_ref().is_some_and(|popup| {
            popup.runtime.profile() == profile && popup.runtime.install_id() == install
        }) {
            self.close_windows_extension_popup();
        }
        if let Some(install) = self.windows_extensions.installs.remove(&(profile, install)) {
            self.windows_extensions
                .revoke_navigation(profile, &install.id);
            install.bridge.alive.set(false);
            let result = native::enable(&install.native, false);
            self.close_extension_view(profile, install.bridge);
            if let Err(error) = result {
                eprintln!("extensions: disable failed: {error}");
                self.quarantine_unverifiable_windows_profile(profile);
            }
        }
        self.sink
            .emit(EngineEvent::ExtensionActionsInvalidated { profile });
    }

    pub(crate) fn remove_web_extension(&mut self, profile: ProfileId, load: WebExtensionLoad) {
        self.close_windows_extension_popup();
        let mut native_change_started = false;
        let result = (|| -> Result<(), String> {
            if let Some(install) = self
                .windows_extensions
                .installs
                .remove(&(profile, load.install))
            {
                self.windows_extensions
                    .revoke_navigation(profile, &install.id);
                install.bridge.alive.set(false);
                native_change_started = true;
                let result = native::remove(&install.native);
                self.close_extension_view(profile, install.bridge);
                return result;
            }
            // Disabled installs have no observer. Use a temporary, accounted
            // controller to remove their native record without enabling them.
            let path = crate::erasure::prepare_profile_directory(&self.profiles_root, profile)
                .map_err(|error| error.to_string())?;
            self.ensure_windows_profile_environment_at_path(
                profile,
                std::time::Instant::now() + std::time::Duration::from_secs(5),
                path,
                true,
            )
            .map_err(|error| format!("Extension profile startup failed: {error:?}"))?;
            let runtime = ExtensionRuntimeInstance::new(
                profile,
                load.install,
                ExtensionRuntimeGeneration::INITIAL,
            );
            let bridge = self.extension_view(
                profile,
                &load.extension_id,
                Rc::new(RefCell::new(Action::default())),
                runtime,
            )?;
            let result = (|| {
                let native_profile =
                    native::profile(&bridge.view.webview()).map_err(|error| error.to_string())?;
                for item in native::list(&native_profile)? {
                    if native::extension_id(&item).map_err(|error| error.to_string())?
                        == load.extension_id
                    {
                        native_change_started = true;
                        native::remove(&item)?;
                    }
                }
                Ok(())
            })();
            self.close_extension_view(profile, bridge);
            result
        })();
        if let Err(error) = result {
            eprintln!("extensions: removal failed: {error}");
            if native_change_started {
                self.quarantine_unverifiable_windows_profile(profile);
            }
        }
    }

    fn close_extension_view(&mut self, profile: ProfileId, owned: ExtensionView) {
        let ExtensionView {
            crash_observer,
            mut view,
            resource,
            alive,
        } = owned;
        alive.set(false);
        drop(crash_observer);
        if let Err(debt) = view.close() {
            self.retain_windows_cleanup_debt(
                profile,
                super::OwnedWindowsCleanupDebt::new(debt, Some(resource)),
            );
        }
    }

    fn extension_view(
        &mut self,
        profile: ProfileId,
        extension: &str,
        action: Rc<RefCell<Action>>,
        runtime: ExtensionRuntimeInstance,
    ) -> Result<ExtensionView, String> {
        let resource = self
            .native_resources
            .try_acquire(NativeResourceClass::Extension)
            .map_err(|_| "Extension view capacity is exhausted.")?;
        let environment = self
            .environments
            .get(&profile)
            .cloned()
            .ok_or("Native environment is unavailable.")?;
        let startup = self
            .windows_extensions
            .startups
            .get(&profile)
            .cloned()
            .ok_or("Extension startup is unavailable.")?;
        let alive = Rc::new(Cell::new(true));
        let callback_alive = alive.clone();
        let sink = self.sink.clone();
        let url = format!(
            "chrome-extension://{extension}/{}/host.html",
            zephium_webext::windows::HOST_DIRECTORY
        );
        let expected = url.clone();
        let navigation_url = url.clone();
        let builder = wry::WebViewBuilder::new()
            .with_environment(environment)
            .with_controller_initialization_hook(|_, core| crate::reputation::configure(core))
            .with_browser_extension_startup_gate(move |environment, core| {
                startup.authenticate(environment, core)
            })
            .with_visible(false)
            .with_focused(false)
            .with_devtools(false)
            .with_autoplay(false)
            .with_fullscreen_enabled(false)
            .with_picture_in_picture_enabled(false)
            .with_general_autofill_enabled(false)
            .with_default_context_menus(false)
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_download_policy(wry::DownloadPolicy::DenyWithoutMetadata)
            .with_page_close_policy(wry::PageClosePolicy::Ignore)
            .with_new_window_req_handler(move |url, features| {
                super::dispatch::try_open_windows_extension_tab(runtime, &url, features)
            })
            .with_navigation_handler(move |target| {
                target == navigation_url || target == "about:blank"
            })
            .with_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0, 0).into(),
                size: wry::dpi::LogicalSize::new(1, 1).into(),
            })
            .with_ipc_handler(move |message| {
                if !callback_alive.get()
                    || message.uri().to_string() != expected
                    || message.body().len() > 32768
                {
                    return;
                }
                let Ok(value) = serde_json::from_str::<Value>(message.body()) else {
                    return;
                };
                if value["kind"] == "ready" {
                    sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
                    return;
                }
                if value["kind"] != "action" {
                    return;
                }
                let Ok(mut action) = action.try_borrow_mut() else {
                    return;
                };
                let Some((tab, window)) = action.binding else {
                    return;
                };
                if value["windowId"].as_i64() != Some(window) {
                    return;
                }
                let icon = value["icon"]
                    .as_array()
                    .filter(|bytes| bytes.len() == 4096)
                    .and_then(|bytes| {
                        bytes
                            .iter()
                            .map(|byte| byte.as_u64().and_then(|n| u8::try_from(n).ok()))
                            .collect::<Option<Vec<_>>>()
                    })
                    .and_then(|bytes| ExtensionActionIcon::from_rgba(bytes).ok());
                let popup = value["popup"]
                    .as_str()
                    .and_then(|value| valid_extension_url(&runtime, &expected, value));
                let revision = action
                    .state
                    .as_ref()
                    .map(|state| state.revision().next())
                    .unwrap_or(Some(ExtensionActionRevision::INITIAL));
                let Some(revision) = revision else {
                    return;
                };
                let Ok(state) = ExtensionActionState::new(
                    runtime,
                    ExtensionActionScope::Tab(tab),
                    revision,
                    value["title"].as_str().unwrap_or(""),
                    value["badge"].as_str().unwrap_or(""),
                    icon,
                    value["enabled"].as_bool().unwrap_or(false),
                    popup.is_some(),
                    false,
                ) else {
                    return;
                };
                if action
                    .state
                    .as_ref()
                    .is_some_and(|previous| previous.same_presentation(&state))
                    && action.popup == popup
                {
                    return;
                }
                action.popup = popup;
                action.native_tab = value["tabId"].as_i64();
                action.state = Some(state);
                drop(action);
                sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
            })
            .with_url("about:blank");
        match builder.build_as_child(&self.parent) {
            Ok(view) => {
                // This controller stays hidden for its lifetime. Use the same
                // cache-trimming hint as background tabs, without suspending
                // the action observer or the extension's native event delivery.
                // Popups use their own visible controller at the normal level.
                let _ = view.set_memory_usage_level(wry::MemoryUsageLevel::Low);
                let mut owned = ExtensionView {
                    crash_observer: None,
                    view,
                    resource,
                    alive,
                };
                match self.observe_extension_crashes(
                    &owned.view,
                    runtime,
                    owned.alive.clone(),
                    false,
                ) {
                    Ok(observer) => owned.crash_observer = Some(observer),
                    Err(error) => {
                        self.close_extension_view(profile, owned);
                        return Err(error);
                    }
                }
                Ok(owned)
            }
            Err(error) => {
                let mut resource = Some(resource);
                for debt in wry::pending_webview2_cleanup_debts() {
                    self.retain_windows_cleanup_debt(
                        profile,
                        super::OwnedWindowsCleanupDebt::new(debt, resource.take()),
                    );
                }
                Err(error.to_string())
            }
        }
    }

    pub(crate) fn extension_actions_snapshot(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        generation: ExtensionBrowserSurfaceGeneration,
    ) -> ExtensionActionSnapshotSettlement {
        let result = (|| {
            let surface = self
                .extension_browser_surfaces
                .get(&profile)
                .ok_or(ExtensionActionRejection::TabUnavailable)?;
            if surface.generation() != generation
                || !surface.tabs().any(|candidate| candidate.id() == tab)
                || self
                    .partitions
                    .get(&tab)
                    .is_none_or(|partition| partition.profile() != profile)
            {
                return Err(ExtensionActionRejection::TabUnavailable);
            }
            let view = self
                .views
                .get(&tab)
                .ok_or(ExtensionActionRejection::TabDiscarded)?;
            let window = extension_window(view)
                .map_err(|_| ExtensionActionRejection::NativeAdmissionFailed)?;
            let mut states = Vec::new();
            for ((owner, _), install) in &self.windows_extensions.installs {
                if *owner != profile {
                    continue;
                }
                {
                    let mut action = install.action.borrow_mut();
                    if action.binding != Some((tab, window)) {
                        action.state = None;
                        action.popup = None;
                    }
                    action.binding = Some((tab, window));
                    if let Some(state) = &action.state {
                        states.push(state.clone());
                    }
                }
                let _ = install
                    .bridge
                    .view
                    .evaluate_script(&format!("window.__zephiumRefresh?.({window})"));
            }
            ExtensionActionSnapshot::new(profile, tab, generation, states)
                .map_err(|_| ExtensionActionRejection::InvalidRequest)
        })();
        match result {
            Ok(snapshot) => ExtensionActionSnapshotSettlement::Applied(snapshot),
            Err(error) => ExtensionActionSnapshotSettlement::Rejected(error),
        }
    }

    pub(super) fn close_windows_extension_popup(&mut self) {
        let Some(popup) = self.windows_extensions.popup.take() else {
            return;
        };
        let restore_focus = popup.window.restore_owner_focus();
        drop(popup.escape);
        let ExtensionView {
            crash_observer,
            mut view,
            resource,
            alive,
        } = popup.view;
        alive.set(false);
        drop(crash_observer);
        if let Err(debt) = view.close() {
            let window = Rc::new(popup.window);
            self.windows_extensions.retained_popup_window = Some(Rc::downgrade(&window));
            self.retain_windows_cleanup_debt(
                popup.runtime.profile(),
                super::OwnedWindowsCleanupDebt::new(debt, Some(resource)).with_parent(window),
            );
        }
        if restore_focus {
            if let Some(view) = self.views.get(&popup.tab) {
                let _ = view.focus();
            }
        }
    }

    fn resize_windows_extension_popup(
        &mut self,
        hwnd: windows::Win32::Foundation::HWND,
        runtime: ExtensionRuntimeInstance,
        width: f64,
        height: f64,
    ) {
        let Some(popup) = self.windows_extensions.popup.as_ref().filter(|popup| {
            popup.window.0 == hwnd && popup.runtime == runtime && popup.view.alive.get()
        }) else {
            return;
        };
        if !popup.window.owner_active() {
            self.close_windows_extension_popup();
            return;
        }
        let result = popup
            .window
            .resize(width, height)
            .and_then(|(width, height)| {
                popup
                    .view
                    .view
                    .set_bounds(wry::Rect {
                        position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                        size: wry::dpi::PhysicalSize::new(width, height).into(),
                    })
                    .map_err(|e| e.to_string())
            });
        if result.is_err() {
            self.close_windows_extension_popup();
            return;
        }
        if popup.window.show() {
            let _ = popup.view.view.focus();
            let _ = popup
                .view
                .view
                .evaluate_script("window.__zephiumPopupShown?.()");
        }
    }

    pub(super) fn reconcile_windows_extension_popup(
        &mut self,
        surface: &zephium_core::extensions::ExtensionBrowserSurface,
    ) {
        let Some(popup) = &self.windows_extensions.popup else {
            return;
        };
        if popup.runtime.profile() != surface.profile() {
            return;
        }
        let next = surface.tabs().find(|tab| tab.id() == popup.tab);
        let previous = self
            .extension_browser_surfaces
            .get(&surface.profile())
            .and_then(|surface| surface.tabs().find(|tab| tab.id() == popup.tab));
        if !surface
            .windows()
            .iter()
            .any(|window| window.active() == Some(popup.tab))
            || next.is_none_or(|tab| !tab.resident() || tab.loading())
            || next.and_then(|tab| tab.url()) != previous.and_then(|tab| tab.url())
        {
            self.close_windows_extension_popup();
        }
    }

    pub(crate) fn invoke_extension_action(
        &mut self,
        request: ExtensionActionRequest,
    ) -> ExtensionActionSettlement {
        if request.is_menu() {
            return ExtensionActionSettlement::Rejected(
                ExtensionActionRejection::UnsupportedPlatform,
            );
        }
        let result = self.present_windows_extension_popup(request);
        result.unwrap_or_else(ExtensionActionSettlement::Rejected)
    }

    pub(crate) fn open_web_extension_options(&mut self, profile: ProfileId, extension_id: &str) {
        let Some(install) = self
            .windows_extensions
            .installs
            .values()
            .find(|install| install.runtime.profile() == profile && install.id == extension_id)
        else {
            return;
        };
        let Some(url) = install.options.as_ref() else {
            return;
        };
        let Some(tab) = self
            .extension_browser_surfaces
            .get(&profile)
            .and_then(|surface| surface.windows().iter().find_map(|window| window.active()))
        else {
            return;
        };
        install.action.borrow_mut().open_request = Some((
            tab,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            8,
        ));
        // The existing native adoption path retains Chromium's extension origin
        // and web_accessible_resources enforcement; this does not serve HTML.
        let _ = install.bridge.view.evaluate_script(&format!(
            "chrome.tabs.create({{url:{},active:true}})",
            json!(url)
        ));
    }

    pub(super) fn open_windows_extension_tab(
        &mut self,
        runtime: ExtensionRuntimeInstance,
        url: &str,
        features: wry::NewWindowFeatures,
    ) -> wry::NewWindowResponse {
        use std::sync::atomic::Ordering;
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsWindowVisible};
        let profile = runtime.profile();
        if !features.user_initiated
            || self.windows_view_admission_blocked(profile)
            || self.erasure_tombstones.contains(&profile)
            || self
                .windows_extensions
                .installs
                .get(&(profile, runtime.install_id()))
                .is_none_or(|install| install.runtime != runtime)
            || self.environments.get(&profile).is_none_or(|environment| {
                !crate::platform::imp::same_environment(environment, &features.opener.environment)
            })
        {
            return wry::NewWindowResponse::Deny;
        }
        // A manager may open a child only during a bounded, explicit toolbar /
        // options command. Unsolicited hidden-worker opens remain denied here.
        let popup = self.windows_extensions.popup.as_ref().filter(|popup| {
            popup.runtime == runtime
                && popup.view.alive.get()
                && popup.view.view.webview().as_raw() == features.opener.webview.as_raw()
                && unsafe { GetForegroundWindow() == popup.window.0 }
        });
        let (source, opener_window) = if let Some(popup) = popup {
            (popup.tab, popup.window.0)
        } else {
            let Some(install) = self
                .windows_extensions
                .installs
                .get(&(profile, runtime.install_id()))
            else {
                return wry::NewWindowResponse::Deny;
            };
            if !install.bridge.alive.get()
                || install.bridge.view.webview().as_raw() != features.opener.webview.as_raw()
            {
                return wry::NewWindowResponse::Deny;
            }
            let mut action = install.action.borrow_mut();
            let Some((tab, expires, remaining)) = action.open_request.as_mut() else {
                return wry::NewWindowResponse::Deny;
            };
            if *expires < std::time::Instant::now() || *remaining == 0 {
                return wry::NewWindowResponse::Deny;
            }
            let Some(owner) = self
                .extension_browser_surfaces
                .get(&profile)
                .and_then(|surface| {
                    surface
                        .windows()
                        .iter()
                        .find(|window| window.active() == Some(*tab))
                })
                .and_then(|window| self.stages.get(&window.id()))
                .and_then(|stage| stage.parent_window())
            else {
                return wry::NewWindowResponse::Deny;
            };
            if unsafe { GetForegroundWindow() } != owner {
                return wry::NewWindowResponse::Deny;
            }
            *remaining -= 1;
            (*tab, owner)
        };
        if self
            .partitions
            .get(&source)
            .is_none_or(|partition| partition.profile() != profile)
            || self
                .extension_browser_surfaces
                .get(&profile)
                .is_none_or(|surface| {
                    !surface
                        .windows()
                        .iter()
                        .any(|window| window.active() == Some(source))
                })
        {
            return wry::NewWindowResponse::Deny;
        }
        let Some(view) = self.views.get(&source) else {
            return wry::NewWindowResponse::Deny;
        };
        if !view.event_permit.allows_navigation(url) {
            return wry::NewWindowResponse::Deny;
        }
        let Some(activity) = view.navigation.activity_snapshot() else {
            return wry::NewWindowResponse::Deny;
        };
        if !view.presentation_permit.load(Ordering::Acquire)
            || !view.download_surface_intent.load(Ordering::Acquire)
        {
            return wry::NewWindowResponse::Deny;
        }
        let permit = view.event_permit.clone();
        self.adopt_native_tab(source, &permit, activity, url, features, move || {
            // Construction pumps native messages: closing or defocusing the
            // popup during that interval must invalidate its pending request.
            unsafe {
                GetForegroundWindow() == opener_window && IsWindowVisible(opener_window).as_bool()
            }
        })
    }

    fn present_windows_extension_popup(
        &mut self,
        request: ExtensionActionRequest,
    ) -> Result<ExtensionActionSettlement, ExtensionActionRejection> {
        let runtime = request.runtime();
        let profile = runtime.profile();
        let surface = self
            .extension_browser_surfaces
            .get(&profile)
            .ok_or(ExtensionActionRejection::TabUnavailable)?;
        if surface.generation() != request.surface_generation()
            || !surface
                .windows()
                .iter()
                .any(|window| window.active() == Some(request.tab()))
        {
            return Err(ExtensionActionRejection::TabUnavailable);
        }
        let owner = surface
            .windows()
            .iter()
            .find(|window| window.active() == Some(request.tab()))
            .and_then(|window| self.stages.get(&window.id()))
            .and_then(|stage| stage.parent_window())
            .ok_or(ExtensionActionRejection::TabUnavailable)?;
        let install = self
            .windows_extensions
            .installs
            .get(&(profile, runtime.install_id()))
            .ok_or(ExtensionActionRejection::RuntimeUnavailable)?;
        if install.runtime != runtime {
            return Err(ExtensionActionRejection::RuntimeSuperseded);
        }
        let mut action = install.action.borrow_mut();
        let state = action
            .state
            .as_ref()
            .ok_or(ExtensionActionRejection::ActionUnavailable)?;
        if state.revision() != request.action_revision()
            || action.binding.map(|(tab, _)| tab) != Some(request.tab())
        {
            return Err(ExtensionActionRejection::ActionUnavailable);
        }
        if !state.is_enabled() {
            return Err(ExtensionActionRejection::ActionDisabled);
        }
        let native_tab = action
            .native_tab
            .ok_or(ExtensionActionRejection::TabUnavailable)?;
        let Some(url) = action.popup.clone() else {
            action.open_request = Some((
                request.tab(),
                std::time::Instant::now() + std::time::Duration::from_secs(5),
                8,
            ));
            drop(action);
            install
                .bridge
                .view
                .evaluate_script(&format!("window.__zephiumClick?.({native_tab})"))
                .map_err(|_| ExtensionActionRejection::NativeAdmissionFailed)?;
            return Ok(ExtensionActionSettlement::Dispatched);
        };
        let window_id = action
            .binding
            .ok_or(ExtensionActionRejection::TabUnavailable)?
            .1;
        let extension_id = install.id.clone();
        drop(action);
        if self
            .windows_extensions
            .dismissed_action
            .take()
            .is_some_and(|(closed, tab, at)| {
                closed == runtime
                    && tab == request.tab()
                    && at.elapsed() < std::time::Duration::from_millis(500)
            })
        {
            return Ok(ExtensionActionSettlement::PopupDismissed);
        }
        if self
            .windows_extensions
            .popup
            .as_ref()
            .is_some_and(|popup| popup.runtime == runtime && popup.tab == request.tab())
        {
            self.close_windows_extension_popup();
            return Ok(ExtensionActionSettlement::PopupDismissed);
        }
        self.close_windows_extension_popup();
        if self
            .windows_extensions
            .retained_popup_window
            .as_ref()
            .is_some_and(|window| window.strong_count() != 0)
            || self.windows_view_admission_blocked(profile)
        {
            return Err(ExtensionActionRejection::PopupCapacityExceeded);
        }
        let view = self
            .views
            .get(&request.tab())
            .ok_or(ExtensionActionRejection::TabDiscarded)?;
        if extension_window(view).ok() != Some(window_id) {
            return Err(ExtensionActionRejection::TabUnavailable);
        }
        let resource = self
            .native_resources
            .try_acquire(NativeResourceClass::Extension)
            .map_err(|_| ExtensionActionRejection::PopupCapacityExceeded)?;
        let window = popup::PopupWindow::new(owner, request.anchor().rect())
            .map_err(|_| ExtensionActionRejection::PopupUnavailable)?;
        let environment = self
            .environments
            .get(&profile)
            .cloned()
            .ok_or(ExtensionActionRejection::RuntimeUnavailable)?;
        let startup = self
            .windows_extensions
            .startups
            .get(&profile)
            .cloned()
            .ok_or(ExtensionActionRejection::RuntimeUnavailable)?;
        let initialization = zephium_webext::windows::POPUP_TARGET_SCRIPT.replace(
            "__ZEPHIUM_BINDING__",
            &json!({"extensionId":extension_id,"tabId":native_tab,"windowId":window_id})
                .to_string(),
        );
        let base = format!("chrome-extension://{extension_id}/");
        let navigation_base = base.clone();
        let burst = Cell::new((std::time::Instant::now(), 0u8));
        let hwnd = window.0;
        let alive = Rc::new(Cell::new(true));
        let message_alive = alive.clone();
        let close_alive = alive.clone();
        let message_base = base.clone();
        let built = wry::WebViewBuilder::new()
            .with_environment(environment)
            .with_controller_initialization_hook(|_, core| crate::reputation::configure(core))
            .with_browser_extension_startup_gate(move |env, core| startup.authenticate(env, core))
            .with_initialization_script(&initialization)
            .with_initialization_script(zephium_webext::windows::POPUP_SIZE_SCRIPT)
            .with_url("about:blank")
            .with_background_color(popup::BACKGROUND)
            .with_visible(true)
            .with_focused(false)
            .with_ipc_handler(move |message| {
                if !message_alive.get()
                    || message.body().len() > 256
                    || valid_extension_url(&runtime, &message_base, &message.uri().to_string())
                        .is_none()
                {
                    return;
                }
                let Ok(value) = serde_json::from_str::<Value>(message.body()) else {
                    return;
                };
                if value["kind"] != "popup-size" {
                    return;
                }
                let (Some(width), Some(height)) =
                    (value["width"].as_f64(), value["height"].as_f64())
                else {
                    return;
                };
                super::dispatch::best_effort_with(move |host| {
                    host.resize_windows_extension_popup(hwnd, runtime, width, height);
                });
            })
            .with_page_close_handler(move || {
                if close_alive.get() {
                    popup::close(hwnd, false);
                }
            })
            .with_devtools(false)
            .with_autoplay(false)
            .with_fullscreen_enabled(false)
            .with_picture_in_picture_enabled(false)
            .with_general_autofill_enabled(false)
            .with_browser_accelerator_keys(false)
            .with_new_window_req_handler(move |url, features| {
                let now = std::time::Instant::now();
                let (started, count) = burst.get();
                let (started, count) = if now.duration_since(started).as_secs() >= 1 {
                    (now, 0)
                } else {
                    (started, count)
                };
                if count >= 8 {
                    return wry::NewWindowResponse::Deny;
                }
                burst.set((started, count + 1));
                super::dispatch::try_open_windows_extension_tab(runtime, &url, features)
            })
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_download_policy(wry::DownloadPolicy::DenyWithoutMetadata)
            .with_navigation_handler(move |target| {
                target == "about:blank"
                    || valid_extension_url(&runtime, &navigation_base, &target).is_some()
            })
            .with_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0, 0).into(),
                // A hidden conventional viewport lets percentage-height/iframe
                // layouts initialize before the content measurement shrinks it.
                size: wry::dpi::LogicalSize::new(400, 600).into(),
            })
            .build_as_child(&window);
        let view = match built {
            Ok(view) => view,
            Err(_) => {
                alive.set(false);
                let mut resource = Some(resource);
                let debts = wry::pending_webview2_cleanup_debts();
                let window = Rc::new(window);
                if !debts.is_empty() {
                    self.windows_extensions.retained_popup_window = Some(Rc::downgrade(&window));
                }
                for debt in debts {
                    self.retain_windows_cleanup_debt(
                        profile,
                        super::OwnedWindowsCleanupDebt::new(debt, resource.take())
                            .with_parent(window.clone()),
                    );
                }
                return Err(ExtensionActionRejection::PopupUnavailable);
            }
        };
        let crash_observer = self.observe_extension_crashes(&view, runtime, alive.clone(), true);
        let crash_failed = crash_observer.is_err();
        let escape = popup::EscapeRegistration::new(&view, hwnd, alive.clone());
        let escape_failed = escape.is_err();
        self.windows_extensions.popup = Some(Popup {
            runtime,
            tab: request.tab(),
            window,
            escape: escape.ok(),
            view: ExtensionView {
                crash_observer: crash_observer.ok(),
                view,
                resource,
                alive,
            },
        });
        if escape_failed || crash_failed {
            self.close_windows_extension_popup();
            return Err(ExtensionActionRejection::PopupUnavailable);
        }
        if self
            .windows_extensions
            .popup
            .as_ref()
            .unwrap()
            .view
            .view
            .load_url(&url)
            .is_err()
        {
            self.close_windows_extension_popup();
            return Err(ExtensionActionRejection::PopupUnavailable);
        }
        // Owned and loading, but not yet visible: its first content measurement
        // will reveal it. Do not report PopupPresented with a guessed size.
        Ok(ExtensionActionSettlement::Dispatched)
    }
}

fn valid_extension_url(
    _runtime: &ExtensionRuntimeInstance,
    base: &str,
    value: &str,
) -> Option<String> {
    let base = url::Url::parse(base).ok()?;
    let parsed = base.join(value).ok()?;
    (parsed.scheme() == "chrome-extension"
        && parsed.host_str() == base.host_str()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && !value.is_empty())
    .then(|| parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_recovery_coalesces_crashes_and_bounds_backoff() {
        let now = std::time::Instant::now();
        let mut retry = HostRecovery::default();
        for millis in [250, 1000, 4000] {
            assert_eq!(
                retry.next(now),
                Some(std::time::Duration::from_millis(millis))
            );
            assert_eq!(retry.next(now), None);
            retry.pending = false;
        }
        assert_eq!(retry.next(now), None);
        assert_eq!(
            retry.next(now + std::time::Duration::from_secs(60)),
            Some(std::time::Duration::from_millis(250))
        );
    }
}

#[cfg(test)]
#[path = "webext_windows_qualification.rs"]
mod qualification;

fn extension_window(view: &super::ObservedView) -> Result<i64, String> {
    if let Some(window) = view.extension_window.get() {
        return Ok(window);
    }
    let window = native::window_id(&view.webview())?;
    view.extension_window.set(Some(window));
    Ok(window)
}
