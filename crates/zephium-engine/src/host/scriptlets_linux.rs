//! Builtin main-world programs owned by the profile, independent of extensions.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use sha2::{Digest, Sha256};
use webkit2gtk::{UserContentManagerExt, WebViewExt};
use wry::WebViewExtUnix;
use zephium_core::blocker::ContentPolicyGeneration;
use zephium_core::ids::ProfileId;

use super::EngineHost;

const MAX_PROGRAM_BYTES: usize = 5 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Scriptlets(HashMap<ProfileId, Rc<Program>>);

struct Program {
    digest: [u8; 32],
    script: webkit2gtk::UserScript,
}

impl Program {
    fn new(source: &str) -> Option<Self> {
        if source.is_empty() || source.len() > MAX_PROGRAM_BYTES || source.contains('\0') {
            return None;
        }
        let digest = Sha256::digest(source.as_bytes()).into();
        let guarded = format!(
            "(() => {{\nif (location.protocol !== 'http:' && location.protocol !== 'https:') return;\n\
             for (const origin of location.ancestorOrigins) {{\n\
             if (!origin.startsWith('http://') && !origin.startsWith('https://')) return;\n\
             }}\n{source}\n}})();"
        );
        #[cfg(feature = "blocker-bench")]
        eprintln!("blocker-qa-program bytes={}", guarded.len());
        Some(Self {
            digest,
            script: webkit2gtk::UserScript::new(
                &guarded,
                webkit2gtk::UserContentInjectedFrames::AllFrames,
                webkit2gtk::UserScriptInjectionTime::Start,
                &["http://*/*", "https://*/*"],
                &[],
            ),
        })
    }
}

#[derive(Default)]
pub(super) struct ViewScriptlet(
    RefCell<Option<Installed>>,
    #[cfg(feature = "blocker-bench")] RefCell<Option<super::blocker_bench::ScriptletMeasurement>>,
);

struct Installed {
    manager: webkit2gtk::UserContentManager,
    program: Rc<Program>,
}

impl Drop for Installed {
    fn drop(&mut self) {
        self.manager.remove_script(&self.program.script);
    }
}

impl ViewScriptlet {
    fn replace(&self, manager: &webkit2gtk::UserContentManager, program: Option<Rc<Program>>) {
        if self
            .0
            .borrow()
            .as_ref()
            .map(|installed| installed.program.digest)
            == program.as_ref().map(|program| program.digest)
        {
            #[cfg(not(feature = "blocker-bench"))]
            return;
            #[cfg(feature = "blocker-bench")]
            if self.1.borrow().is_some() {
                return;
            }
        }
        #[cfg(feature = "blocker-bench")]
        drop(self.1.take());
        drop(self.0.take());
        #[cfg(feature = "blocker-bench")]
        let measurement = super::blocker_bench::ScriptletMeasurement::start(manager);
        if let Some(program) = program {
            manager.add_script(&program.script);
            self.0.replace(Some(Installed {
                manager: manager.clone(),
                program,
            }));
        }
        #[cfg(feature = "blocker-bench")]
        self.1.replace(Some(measurement.finish()));
    }
}

impl Scriptlets {
    pub(super) fn forget(&mut self, profile: ProfileId) {
        self.0.remove(&profile);
    }

    fn prepare(&mut self, profile: ProfileId, source: Option<&str>) {
        let wanted = source.map(|source| <[u8; 32]>::from(Sha256::digest(source.as_bytes())));
        if self.0.get(&profile).map(|program| program.digest) == wanted {
            return;
        }
        self.0.remove(&profile);
        if let Some(source) = source {
            if let Some(program) = self
                .0
                .values()
                .find(|program| Some(program.digest) == wanted)
                .cloned()
            {
                self.0.insert(profile, program);
                return;
            }
            if let Some(program) = Program::new(source) {
                self.0.insert(profile, Rc::new(program));
            } else {
                eprintln!("content blocker: invalid scriptlet program was not registered");
            }
        }
    }
}

impl EngineHost {
    pub(super) fn ensure_view_scriptlets(
        &self,
        profile: ProfileId,
        view: &wry::WebView,
        slot: &ViewScriptlet,
    ) {
        if let Some(manager) = view.webview().user_content_manager() {
            slot.replace(&manager, self.scriptlets.0.get(&profile).cloned());
        }
    }

    pub(super) fn refresh_profile_scriptlets(&mut self, profile: ProfileId) {
        let source = self.scriptlet_source(profile);
        self.scriptlets.prepare(profile, source.as_deref());
        for (id, observed) in &self.views {
            if self
                .partitions
                .get(id)
                .is_some_and(|p| p.profile() == profile)
            {
                self.ensure_view_scriptlets(profile, &observed.view, &observed.scriptlet);
            }
        }
        if let Some(spare) = &self.spare {
            if spare.partition.profile() == profile {
                self.ensure_view_scriptlets(profile, &spare.view.view, &spare.view.scriptlet);
            }
        }
    }

    pub(super) fn wait_for_scriptlets(
        &mut self,
        profile: ProfileId,
        _generation: ContentPolicyGeneration,
    ) -> bool {
        self.refresh_profile_scriptlets(profile);
        false
    }
}

#[cfg(test)]
#[path = "scriptlets_linux_tests.rs"]
mod tests;
