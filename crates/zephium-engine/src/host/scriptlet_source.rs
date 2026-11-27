use zephium_core::blocker::scriptlet_program_with_pauses;
use zephium_core::ids::ProfileId;

use super::EngineHost;

impl EngineHost {
    /// The program a profile's pages run: its policy's scriptlets with its
    /// paused sites in them. None without scriptlets, or without site
    /// preferences, when every view counts as paused.
    pub(super) fn scriptlet_source(&self, profile: ProfileId) -> Option<String> {
        let sites = self.blocker_sites.get(&profile)?.borrow().clone()?;
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
    }
}
