//! Each profile's scriptlet program, run in page main worlds through a
//! built-in context of the profile's extension controller. Packages are
//! content-addressed, so profiles with the same lists share one.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use zephium_core::ids::ProfileId;
use zephium_webext_macos::PageScript;

use super::EngineHost;

pub(super) struct Scriptlets {
    root: PathBuf,
    packages: HashMap<ProfileId, (Arc<str>, PathBuf)>,
    swept: bool,
}

impl Scriptlets {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            root,
            packages: HashMap::new(),
            swept: false,
        }
    }

    fn package(&mut self, profile: ProfileId, program: &Arc<str>) -> Option<PathBuf> {
        if let Some((current, dir)) = self.packages.get(&profile) {
            if Arc::ptr_eq(current, program) {
                return Some(dir.clone());
            }
        }
        let digest = Sha256::digest(program.as_bytes());
        let name: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        let dir = self.root.join(name);
        if !dir.is_dir() {
            if let Err(error) = PageScript::write(&dir, program) {
                eprintln!("content blocker: cannot write the scriptlet package: {error}");
                self.packages.remove(&profile);
                return None;
            }
        }
        self.packages
            .insert(profile, (program.clone(), dir.clone()));
        Some(dir)
    }

    /// Removes every package no profile wants or still runs.
    fn sweep(&mut self, running: impl Iterator<Item = PathBuf>) {
        self.swept = true;
        let keep: HashSet<PathBuf> = self
            .packages
            .values()
            .map(|(_, dir)| dir.clone())
            .chain(running)
            .collect();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !keep.contains(&path) {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

impl EngineHost {
    /// Brings the profile's scriptlet context in line with its applied
    /// policy and paused sites. Without site preferences every view counts
    /// as paused, so nothing runs.
    pub(super) fn refresh_profile_scriptlets(&mut self, profile: ProfileId) {
        if self.webext.page_script(profile).is_none() {
            return;
        }
        let sites = self
            .blocker_sites
            .get(&profile)
            .and_then(|slot| slot.borrow().clone());
        let program = sites.as_ref().and_then(|_| {
            self.content_policies
                .get(&profile)?
                .applied
                .as_ref()?
                .cosmetics
                .as_ref()?
                .scriptlet_program()
        });
        let before = self
            .scriptlets
            .packages
            .get(&profile)
            .map(|(_, dir)| dir.clone());
        let dir = match &program {
            Some(program) => self.scriptlets.package(profile, program),
            None => {
                self.scriptlets.packages.remove(&profile);
                None
            }
        };
        let Some(script) = self.webext.page_script(profile) else {
            return;
        };
        script.set_paused(
            sites
                .iter()
                .flat_map(|sites| sites.paused_sites())
                .map(|site| site.as_str().to_owned())
                .collect(),
        );
        script.set(dir.as_deref(), |result| {
            if let Err(error) = result {
                eprintln!("content blocker: the scriptlet program did not load: {error}");
            }
        });
        if before != dir || !self.scriptlets.swept {
            let running: Vec<PathBuf> = self
                .webext
                .page_scripts()
                .filter_map(PageScript::root)
                .collect();
            self.scriptlets.sweep(running.into_iter());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_are_shared_by_content_and_swept_once_unused() {
        let temp = tempfile::tempdir().unwrap();
        let mut scriptlets = Scriptlets::new(temp.path().to_path_buf());
        let (first, second) = (ProfileId::generate(), ProfileId::generate());
        let program: Arc<str> = Arc::from("globalThis.a = 1;");
        let same: Arc<str> = Arc::from("globalThis.a = 1;");
        let a = scriptlets.package(first, &program).unwrap();
        assert_eq!(scriptlets.package(second, &same).unwrap(), a);
        assert_eq!(
            std::fs::read_to_string(a.join("program.js")).unwrap(),
            "globalThis.a = 1;"
        );

        let next: Arc<str> = Arc::from("globalThis.a = 2;");
        let b = scriptlets.package(first, &next).unwrap();
        assert_ne!(a, b);
        std::fs::create_dir(temp.path().join("stale")).unwrap();
        scriptlets.sweep(std::iter::empty());
        assert!(a.is_dir() && b.is_dir());
        assert!(!temp.path().join("stale").exists());

        scriptlets.packages.remove(&second);
        scriptlets.sweep(std::iter::once(a.clone()));
        assert!(a.is_dir(), "a running package stays until it is replaced");
        scriptlets.sweep(std::iter::empty());
        assert!(!a.exists() && b.is_dir());
    }
}
