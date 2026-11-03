//! Each profile's scriptlet program, run in page main worlds through a
//! built-in context of the profile's extension controller. Private profiles,
//! which run no extensions, get a nonpersistent controller holding only this
//! context.
//!
//! A package carries the profile's paused sites, so it is content-addressed
//! and shared only among normal profiles. A private profile's packages live
//! in its own directory under a root that is emptied at every start and as
//! its session ends; they never reach the shared store.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::MainThreadOnly;
use objc2_web_kit::{
    WKWebExtensionController, WKWebExtensionControllerConfiguration, WKWebViewConfiguration,
    WKWebsiteDataStore,
};
use sha2::{Digest, Sha256};
use zephium_core::blocker::{scriptlet_program_with_pauses, ContentPolicyGeneration};
use zephium_core::ids::ProfileId;
use zephium_core::ports::engine::Partition;
use zephium_webext_macos::PageScript;

use super::dispatch::with_scriptlets_ready;
use super::EngineHost;

/// How long a policy's settlement waits for WebKit to read a new program.
/// The shell's startup hold is shorter and gives up first.
const READY_WITHIN: Duration = Duration::from_secs(2);

pub(super) struct Scriptlets {
    root: PathBuf,
    private_root: PathBuf,
    packages: HashMap<ProfileId, PathBuf>,
    private: HashMap<ProfileId, Private>,
    waits: HashMap<ProfileId, Wait>,
    swept: bool,
}

struct Private {
    // Dropped after the script, which unloads its context from it.
    script: PageScript,
    controller: Retained<WKWebExtensionController>,
}

struct Wait {
    generation: ContentPolicyGeneration,
    _timeout: Option<crate::platform::imp::ContentPolicyTimeout>,
}

impl Scriptlets {
    pub(super) fn new(root: PathBuf, private_root: PathBuf) -> Self {
        // Private packages hold paused sites; a crash may have left some.
        empty(&private_root);
        Self {
            root,
            private_root,
            packages: HashMap::new(),
            private: HashMap::new(),
            waits: HashMap::new(),
            swept: false,
        }
    }

    fn prepare_private(&mut self, profile: ProfileId) -> Option<&Private> {
        if !PageScript::supported() {
            return None;
        }
        let mtm = objc2::MainThreadMarker::new()?;
        Some(self.private.entry(profile).or_insert_with(|| {
            let settings =
                unsafe { WKWebExtensionControllerConfiguration::nonPersistentConfiguration(mtm) };
            let store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
            unsafe { settings.setDefaultWebsiteDataStore(Some(&store)) };
            let controller = unsafe {
                WKWebExtensionController::initWithConfiguration(
                    WKWebExtensionController::alloc(mtm),
                    &settings,
                )
            };
            Private {
                script: PageScript::new(&controller, true),
                controller,
            }
        }))
    }

    /// Gives a private view's configuration the profile's controller.
    pub(super) fn attach_private(
        &mut self,
        profile: ProfileId,
        configuration: &WKWebViewConfiguration,
    ) {
        if let Some(private) = self.prepare_private(profile) {
            unsafe { configuration.setWebExtensionController(Some(&private.controller)) };
        }
    }

    /// Lets go of a profile's wait as its policy retires, and of a private
    /// profile's context and everything it wrote.
    pub(super) fn forget(&mut self, profile: ProfileId) {
        self.waits.remove(&profile);
        if self.private.remove(&profile).is_some() {
            self.packages.remove(&profile);
        }
        let _ = std::fs::remove_dir_all(self.private_dir(profile));
    }

    fn private_dir(&self, profile: ProfileId) -> PathBuf {
        self.private_root.join(hex(&profile.bytes()))
    }

    fn package(&mut self, profile: ProfileId, private: bool, program: &str) -> Option<PathBuf> {
        let name = hex(&Sha256::digest(program.as_bytes())[..16]);
        let dir = if private {
            self.private_dir(profile).join(name)
        } else {
            self.root.join(name)
        };
        if !dir.is_dir() {
            let written = dir
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| PageScript::write(&dir, program));
            if let Err(error) = written {
                eprintln!("content blocker: cannot write the scriptlet package: {error}");
                self.packages.remove(&profile);
                return None;
            }
        }
        self.packages.insert(profile, dir.clone());
        Some(dir)
    }

    /// Removes every package no profile wants or still runs.
    fn sweep(&mut self, running: impl Iterator<Item = PathBuf>) {
        self.swept = true;
        let keep: HashSet<PathBuf> = self.packages.values().cloned().chain(running).collect();
        prune(&self.root, &keep);
        for entry in std::fs::read_dir(&self.private_root)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            prune(&path, &keep);
            let _ = std::fs::remove_dir(&path);
        }
    }
}

fn prune(dir: &Path, keep: &HashSet<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if !keep.contains(&path) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

fn empty(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl EngineHost {
    /// Makes the profile's scriptlet context before its first view, so the
    /// startup hold, which waits for the policy's settlement, covers it.
    pub(crate) fn prepare_profile(&mut self, partition: Partition) {
        let profile = partition.profile();
        if self.erasure_tombstones.contains(&profile)
            || !super::profiles::bind_profile_persistence_class(
                &mut self.profile_persistence_classes,
                partition,
            )
        {
            return;
        }
        match partition {
            Partition::Ephemeral(_) => {
                self.scriptlets.prepare_private(profile);
            }
            Partition::Default(_) | Partition::Persistent(_) => {
                self.webext.prepare(profile, &self.sink);
            }
        }
    }

    /// Reports the policy applied once the profile's program runs from it
    /// too. True when that waits on WebKit; the settlement then follows from
    /// [`Self::scriptlets_ready`].
    pub(super) fn wait_for_scriptlets(
        &mut self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
    ) -> bool {
        if !self.refresh_profile_scriptlets(profile) {
            return false;
        }
        let timeout =
            crate::platform::imp::schedule_content_policy_timeout(READY_WITHIN, move || {
                with_scriptlets_ready(profile, move |host| host.scriptlets_ready(profile));
            });
        self.scriptlets.waits.insert(
            profile,
            Wait {
                generation,
                _timeout: timeout,
            },
        );
        true
    }

    fn scriptlets_ready(&mut self, profile: ProfileId) {
        if let Some(wait) = self.scriptlets.waits.remove(&profile) {
            self.settle_applied_content_policy(profile, wait.generation);
        }
    }

    /// Brings the profile's scriptlet context in line with its applied
    /// policy and paused sites; true while WebKit reads a new package.
    /// Without site preferences every view counts as paused, so nothing runs.
    pub(super) fn refresh_profile_scriptlets(&mut self, profile: ProfileId) -> bool {
        if !self.scriptlets.private.contains_key(&profile)
            && self.webext.page_script(profile).is_none()
        {
            return false;
        }
        let sites = self
            .blocker_sites
            .get(&profile)
            .and_then(|slot| slot.borrow().clone());
        let program = sites.as_ref().and_then(|sites| {
            let program = self
                .content_policies
                .get(&profile)?
                .applied
                .as_ref()?
                .cosmetics
                .as_ref()?
                .scriptlet_program()?;
            Some(scriptlet_program_with_pauses(
                &program,
                sites.paused_sites(),
            ))
        });
        let before = self.scriptlets.packages.get(&profile).cloned();
        let private = self.scriptlets.private.contains_key(&profile);
        let dir = match &program {
            Some(program) => self.scriptlets.package(profile, private, program),
            None => {
                self.scriptlets.packages.remove(&profile);
                None
            }
        };
        let Some(script) = self
            .scriptlets
            .private
            .get(&profile)
            .map(|private| &private.script)
            .or_else(|| self.webext.page_script(profile))
        else {
            return false;
        };
        let waiting = Rc::new(std::cell::Cell::new(false));
        let notify = waiting.clone();
        let pending = script.set(dir.as_deref(), move |result| {
            if let Err(error) = result {
                eprintln!("content blocker: the scriptlet program did not load: {error}");
            }
            if notify.get() {
                with_scriptlets_ready(profile, move |host| host.scriptlets_ready(profile));
            }
        });
        waiting.set(pending);
        if before != dir || !self.scriptlets.swept {
            let running: Vec<PathBuf> = self
                .webext
                .page_scripts()
                .chain(
                    self.scriptlets
                        .private
                        .values()
                        .map(|private| &private.script),
                )
                .filter_map(PageScript::root)
                .collect();
            self.scriptlets.sweep(running.into_iter());
        }
        pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scriptlets() -> (tempfile::TempDir, Scriptlets) {
        let temp = tempfile::tempdir().unwrap();
        let (root, private) = (temp.path().join("shared"), temp.path().join("private"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&private).unwrap();
        (temp, Scriptlets::new(root, private))
    }

    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files
    }

    #[test]
    fn packages_are_shared_by_content_and_swept_once_unused() {
        let (temp, mut scriptlets) = scriptlets();
        let (first, second) = (ProfileId::generate(), ProfileId::generate());
        let a = scriptlets
            .package(first, false, "globalThis.a = 1;")
            .unwrap();
        assert_eq!(
            scriptlets
                .package(second, false, "globalThis.a = 1;")
                .unwrap(),
            a
        );
        assert_eq!(
            std::fs::read_to_string(a.join("program.js")).unwrap(),
            "globalThis.a = 1;"
        );

        let b = scriptlets
            .package(first, false, "globalThis.a = 2;")
            .unwrap();
        assert_ne!(a, b);
        let stale = temp.path().join("shared/stale");
        std::fs::create_dir(&stale).unwrap();
        scriptlets.sweep(std::iter::empty());
        assert!(a.is_dir() && b.is_dir());
        assert!(!stale.exists());

        scriptlets.packages.remove(&second);
        scriptlets.sweep(std::iter::once(a.clone()));
        assert!(a.is_dir(), "a running package stays until it is replaced");
        scriptlets.sweep(std::iter::empty());
        assert!(!a.exists() && b.is_dir());
    }

    #[test]
    fn a_private_pause_never_reaches_the_shared_store() {
        let (temp, mut scriptlets) = scriptlets();
        let private = ProfileId::generate();
        let site =
            zephium_core::blocker::BlockerSite::try_from("paused.example".to_owned()).unwrap();
        let program = scriptlet_program_with_pauses("globalThis.a = 1;", [&site]);
        let dir = scriptlets.package(private, true, &program).unwrap();
        assert!(dir.starts_with(temp.path().join("private")));
        let shared = scriptlets
            .package(ProfileId::generate(), false, "globalThis.a = 1;")
            .unwrap();
        assert!(shared.starts_with(temp.path().join("shared")));
        scriptlets.sweep(std::iter::empty());
        let leaked = files(&temp.path().join("shared")).into_iter().any(|file| {
            std::fs::read_to_string(file)
                .unwrap_or_default()
                .contains("paused.example")
        });
        assert!(!leaked, "a private pause reached the shared store");
        assert!(dir.is_dir());
        scriptlets.forget(private);
        assert!(files(&temp.path().join("private")).is_empty());
    }

    #[test]
    fn leftover_private_packages_go_at_start() {
        let temp = tempfile::tempdir().unwrap();
        let (root, private) = (temp.path().join("shared"), temp.path().join("private"));
        std::fs::create_dir_all(private.join("profile/package")).unwrap();
        std::fs::write(private.join("profile/package/program.js"), "x").unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let _scriptlets = Scriptlets::new(root, private.clone());
        assert!(files(&private).is_empty());
    }
}
