//! Keeps each profile's running extensions equal to what the browser has
//! installed and enabled.

use std::collections::HashMap;

use zephium_core::ids::ExtensionInstallId;
use zephium_core::ports::engine::{WebExtensionLoad, WebExtensionLoaded};

use super::*;

#[derive(Default)]
pub(super) struct WebExtensionState {
    // Restored at launch before the session exists; applied once it does.
    deferred: HashMap<ProfileId, Vec<WebExtensionLoad>>,
    loaded: HashMap<ProfileId, HashMap<ExtensionInstallId, WebExtensionLoad>>,
    status: HashMap<ExtensionInstallId, WebExtensionStatus>,
    preparation_failures: HashMap<ProfileId, Vec<(ExtensionInstallId, String)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebExtensionStatus {
    Loading,
    Running(WebExtensionLoaded),
    Failed(String),
}

/// Where an installation from the current tab would go.
#[derive(Clone, Debug)]
pub struct WebExtensionTarget {
    pub profile: ProfileId,
    /// The active tab's address when it is a Chrome Web Store listing.
    pub listing_url: Option<String>,
}

impl Shell {
    pub(super) fn web_extension_profile_admitted(&self, profile: ProfileId) -> bool {
        !self.profile_deletion.states.contains_key(&profile)
            && self
                .profiles
                .get(profile)
                .is_some_and(|profile| profile.kind != ProfileKind::Incognito)
    }

    pub(super) fn retire_web_extensions(&mut self, profile: ProfileId) {
        self.web_extensions.deferred.remove(&profile);
        self.web_extensions.preparation_failures.remove(&profile);
        if let Some(installs) = self.web_extensions.loaded.remove(&profile) {
            for install in installs.keys() {
                self.web_extensions.status.remove(install);
            }
        }
    }

    pub(super) fn set_prepared_web_extensions(
        &mut self,
        profile: ProfileId,
        wanted: Vec<WebExtensionLoad>,
        failures: Vec<(ExtensionInstallId, String)>,
        retry: Option<ExtensionInstallId>,
    ) {
        self.set_web_extensions_for(profile, wanted, retry);
        if !self.bootstrapped || self.web_extension_profile_admitted(profile) {
            self.web_extensions
                .preparation_failures
                .insert(profile, failures);
        }
    }

    pub(super) fn set_web_extensions(&mut self, profile: ProfileId, wanted: Vec<WebExtensionLoad>) {
        self.set_web_extensions_for(profile, wanted, None);
    }

    fn set_web_extensions_for(
        &mut self,
        profile: ProfileId,
        wanted: Vec<WebExtensionLoad>,
        retry: Option<ExtensionInstallId>,
    ) {
        if !self.bootstrapped {
            self.web_extensions.deferred.insert(profile, wanted);
            return;
        }
        if !self.web_extension_profile_admitted(profile) {
            return;
        }
        self.web_extensions.preparation_failures.remove(&profile);
        let current = self.web_extensions.loaded.entry(profile).or_default();
        let wanted: std::collections::BTreeMap<_, _> = wanted
            .into_iter()
            .map(|load| (load.install, load))
            .collect();
        let removed: Vec<_> = current
            .keys()
            .filter(|install| !wanted.contains_key(install))
            .copied()
            .collect();
        for install in removed {
            current.remove(&install);
            self.web_extensions.status.remove(&install);
            let _ = self.engine.unload_web_extension(profile, install);
        }
        for (install, load) in wanted {
            if current
                .get(&install)
                .is_some_and(|loaded| unchanged(loaded, &load))
                && retry != Some(install)
            {
                continue;
            }
            #[cfg(target_os = "windows")]
            let replacing = current.contains_key(&install)
                && matches!(
                    self.web_extensions.status.get(&install),
                    Some(WebExtensionStatus::Loading | WebExtensionStatus::Running(_))
                );
            current.insert(install, load.clone());
            #[cfg(target_os = "windows")]
            {
                if replacing {
                    self.web_extensions
                        .status
                        .insert(install, WebExtensionStatus::Loading);
                    if self.engine.load_web_extension(profile, load) != NativeDispatch::Scheduled {
                        self.web_extensions.status.insert(
                            install,
                            WebExtensionStatus::Failed("The browser could not start it.".into()),
                        );
                    }
                } else {
                    self.web_extensions.status.insert(
                        install,
                        WebExtensionStatus::Failed(
                            zephium_core::extensions::WINDOWS_EXTENSION_CAPACITY_MESSAGE.into(),
                        ),
                    );
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                self.web_extensions
                    .status
                    .insert(install, WebExtensionStatus::Loading);
                if self.engine.load_web_extension(profile, load) != NativeDispatch::Scheduled {
                    self.web_extensions.status.insert(
                        install,
                        WebExtensionStatus::Failed("The browser could not start it.".into()),
                    );
                }
            }
        }

        #[cfg(target_os = "windows")]
        self.pump_waiting_windows_extensions();

        let mut active = zephium_core::extensions::ExtensionActiveProfiles::EMPTY;
        for (profile, installs) in &self.web_extensions.loaded {
            if !installs.is_empty() && !active.try_insert(*profile) {
                crate::diagnostic!("extensions: too many profiles run extensions");
            }
        }
        if self
            .extension_browser_surfaces
            .replace_active_profiles(active)
        {
            let sync = self.sync_extension_browser_surfaces();
            if sync.native.rejected {
                crate::diagnostic!("extensions: browser-surface synchronization awaits retry");
            }
        }
        if self.extension_actions.clear_projection(profile) {
            self.project_extension_actions(profile);
        }
        let _ = self.refresh_extension_actions(profile);
    }

    #[cfg(target_os = "windows")]
    fn pump_waiting_windows_extensions(&mut self) {
        // Admission is global on Windows. Queue in stable profile/install order;
        // only a capacity failure is retried automatically, never a bad package.
        let mut waiting: Vec<_> = self
            .web_extensions
            .loaded
            .iter()
            .flat_map(|(profile, installs)| {
                installs.iter().filter_map(|(id, load)| {
                matches!(self.web_extensions.status.get(id), Some(WebExtensionStatus::Failed(error))
                    if error == zephium_core::extensions::WINDOWS_EXTENSION_CAPACITY_MESSAGE)
                    .then_some((*profile, *id, load.clone()))
            })
            })
            .collect();
        waiting.sort_by_key(|(profile, id, _)| (*profile, *id));
        for (profile, id, load) in waiting {
            let active = self
                .web_extensions
                .status
                .values()
                .filter(|status| {
                    matches!(
                        status,
                        WebExtensionStatus::Loading | WebExtensionStatus::Running(_)
                    )
                })
                .count();
            if active >= zephium_core::extensions::WINDOWS_RUNNING_EXTENSIONS {
                break;
            }
            self.web_extensions
                .status
                .insert(id, WebExtensionStatus::Loading);
            if self.engine.load_web_extension(profile, load) != NativeDispatch::Scheduled {
                self.web_extensions.status.insert(
                    id,
                    WebExtensionStatus::Failed("The browser could not start it.".into()),
                );
            }
        }
    }

    pub(super) fn remove_web_extension(&mut self, profile: ProfileId, extension: WebExtensionLoad) {
        if let Some(current) = self.web_extensions.loaded.get_mut(&profile) {
            current.remove(&extension.install);
        }
        if let Some(deferred) = self.web_extensions.deferred.get_mut(&profile) {
            deferred.retain(|load| load.install != extension.install);
        }
        self.web_extensions.status.remove(&extension.install);
        if self.engine.remove_web_extension(profile, extension) != NativeDispatch::Scheduled {
            crate::diagnostic!("extensions: the browser could not erase a removed extension");
        }
        #[cfg(target_os = "windows")]
        self.pump_waiting_windows_extensions();
    }

    pub(super) fn apply_deferred_web_extensions(&mut self) {
        let deferred: std::collections::BTreeMap<_, _> =
            std::mem::take(&mut self.web_extensions.deferred)
                .into_iter()
                .collect();
        for (profile, wanted) in deferred {
            let failures = self.web_extensions.preparation_failures.remove(&profile);
            self.set_web_extensions(profile, wanted);
            if self.web_extension_profile_admitted(profile) {
                if let Some(failures) = failures {
                    self.web_extensions
                        .preparation_failures
                        .insert(profile, failures);
                }
            }
        }
    }

    pub(super) fn on_web_extension_settled(
        &mut self,
        profile: ProfileId,
        install: ExtensionInstallId,
        result: Result<WebExtensionLoaded, String>,
    ) {
        if !self.web_extension_profile_admitted(profile) {
            return;
        }
        let still_wanted = self
            .web_extensions
            .loaded
            .get(&profile)
            .is_some_and(|installs| installs.contains_key(&install));
        if !still_wanted {
            return;
        }
        #[cfg(target_os = "windows")]
        let capacity_failure = result.as_ref().is_err_and(|error| {
            error == zephium_core::extensions::WINDOWS_EXTENSION_CAPACITY_MESSAGE
        });
        let status = match result {
            Ok(loaded) => WebExtensionStatus::Running(loaded),
            Err(error) => {
                crate::diagnostic!("extensions: an extension failed to start: {error}");
                WebExtensionStatus::Failed(error)
            }
        };
        self.web_extensions.status.insert(install, status);
        #[cfg(target_os = "windows")]
        if !capacity_failure {
            self.pump_waiting_windows_extensions();
        }
        let _ = self.refresh_extension_actions(profile);
    }

    #[cfg(target_os = "windows")]
    pub(super) fn extension_document_may_close(&self, id: ItemId) -> bool {
        let Some(profile) = self.profile_of_item(id) else {
            return false;
        };
        let Some(extension) = self
            .items
            .tab(id)
            .and_then(|tab| tab.url.as_ref())
            .and_then(zephium_core::navigation::extension_document_id)
        else {
            return false;
        };
        self.web_extensions
            .loaded
            .get(&profile)
            .is_some_and(|installs| {
                installs.iter().any(|(id, load)| {
                    load.extension_id == extension
                        && matches!(
                            self.web_extensions.status.get(id),
                            Some(WebExtensionStatus::Running(_))
                        )
                })
            })
    }

    pub(super) fn web_extension_status(
        &self,
        profile: ProfileId,
    ) -> Vec<(ExtensionInstallId, WebExtensionStatus)> {
        if !self.web_extension_profile_admitted(profile) {
            return Vec::new();
        }
        let mut result: Vec<_> = self
            .web_extensions
            .loaded
            .get(&profile)
            .map(|installs| {
                installs
                    .keys()
                    .filter_map(|install| {
                        let status = self.web_extensions.status.get(install)?;
                        Some((*install, status.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(failures) = self.web_extensions.preparation_failures.get(&profile) {
            result.extend(
                failures
                    .iter()
                    .map(|(install, error)| (*install, WebExtensionStatus::Failed(error.clone()))),
            );
        }
        result
    }

    pub(super) fn web_extension_target(&self, tab: Option<ItemId>) -> Option<WebExtensionTarget> {
        if !self.bootstrapped {
            return None;
        }
        let window = self.windows.focused()?;
        if !self.web_extension_profile_admitted(window.profile) {
            return None;
        }
        let listing_url = tab
            .filter(|tab| {
                window.active == Some(*tab) && self.extension_tab_in_scope(*tab, window.profile)
            })
            .and_then(|tab| self.items.tab(tab))
            .and_then(|tab| tab.url.as_ref())
            .filter(|url| super::extension_store::store_listing_url(url))
            .map(|url| url.as_str().to_owned());
        Some(WebExtensionTarget {
            profile: window.profile,
            listing_url,
        })
    }
}

impl Shell {
    /// Only the active tab of the focused window, in a profile that can
    /// install extensions, showing that exact listing.
    fn store_listing_in_front(&self, tab: ItemId, extension: &str) -> bool {
        self.web_extension_target(Some(tab))
            .and_then(|target| target.listing_url)
            .and_then(|url| url::Url::parse(&url).ok())
            .is_some_and(|url| super::extension_store::store_listing_id(&url) == Some(extension))
    }

    pub(super) fn show_store_button(
        &mut self,
        tab: ItemId,
        button: zephium_core::ports::engine::StoreButton,
    ) {
        if self.store_listing_in_front(tab, &button.extension) {
            self.engine.show_store_button(tab, button);
        }
    }

    pub(super) fn store_button_clicked(&mut self, tab: ItemId, extension: String) {
        if self.store_listing_in_front(tab, &extension) {
            (self.emit)(Projection::WebExtensionStoreClick(
                zephium_ipc::WebExtensionStoreClickView {
                    tab_id: tab.to_string(),
                    extension_id: extension,
                },
            ));
        }
    }
}

/// `start_background` asks for one background run after a package changed; a
/// later apply that clears it changes nothing that is loaded.
fn unchanged(loaded: &WebExtensionLoad, wanted: &WebExtensionLoad) -> bool {
    loaded.install == wanted.install
        && loaded.extension_id == wanted.extension_id
        && loaded.root == wanted.root
        && loaded.permissions == wanted.permissions
        && loaded.external_native_messaging == wanted.external_native_messaging
        && loaded.native_messaging_grantable == wanted.native_messaging_grantable
        && loaded.match_patterns == wanted.match_patterns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_the_background_run_does_not_reload_an_extension() {
        let loaded = WebExtensionLoad {
            install: ExtensionInstallId::from(7),
            extension_id: "abcdefghijklmnopabcdefghijklmnop".into(),
            root: "/packages/a/1.0-rev".into(),
            permissions: vec!["storage".into()],
            external_native_messaging: false,
            native_messaging_grantable: false,
            match_patterns: vec!["<all_urls>".into()],
            start_background: true,
        };
        let settled = WebExtensionLoad {
            start_background: false,
            ..loaded.clone()
        };
        assert!(unchanged(&loaded, &settled));
        let narrowed = WebExtensionLoad {
            match_patterns: vec!["*://github.com/*".into()],
            ..settled.clone()
        };
        assert!(!unchanged(&loaded, &narrowed));
        let authority_changed = WebExtensionLoad {
            external_native_messaging: true,
            ..settled
        };
        assert!(!unchanged(&loaded, &authority_changed));
    }
}
