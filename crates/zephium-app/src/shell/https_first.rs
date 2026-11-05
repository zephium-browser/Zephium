//! Pages the browser opens over plain http are tried over https first where
//! the engine does not do it itself, as Chrome's HTTPS upgrades do. One that
//! fails over https loads over http, and its host is not tried again until
//! Zephium quits.

use super::*;

#[derive(Default)]
pub(super) struct HttpsFirst {
    /// Each tab's https try and the address it was asked for.
    pending: std::collections::HashMap<ItemId, url::Url>,
    /// Hosts that failed over https, per profile, for this run.
    fallback: std::collections::HashSet<(ProfileId, String)>,
    /// Each tab's http load after a failed try, until it commits or fails.
    fallen_back: std::collections::HashMap<ItemId, url::Url>,
}

impl Shell {
    /// The address to load in place of `url`, when it is tried over https.
    pub(super) fn https_first(&mut self, id: ItemId, url: &str) -> Option<String> {
        self.https_first.pending.remove(&id);
        self.https_first.fallen_back.remove(&id);
        if self.engine.upgrades_https_natively() {
            return None;
        }
        let asked = url::Url::parse(url).ok()?;
        let upgraded = zephium_core::navigation::https_upgrade(&asked)?;
        let profile = self.profile_of_item(id)?;
        let host = asked.host_str()?.to_owned();
        if self.https_first.fallback.contains(&(profile, host)) {
            return None;
        }
        self.https_first.pending.insert(id, asked);
        Some(upgraded.to_string())
    }

    /// The tab's load failed. When it was an https try, the page loads over
    /// http as asked, its host is not tried again, and the failure is not
    /// shown: true then.
    pub(super) fn https_first_failed(&mut self, id: ItemId) -> bool {
        self.https_first.fallen_back.remove(&id);
        let Some(asked) = self.https_first.pending.remove(&id) else {
            return false;
        };
        if let (Some(profile), Some(host)) = (self.profile_of_item(id), asked.host_str()) {
            self.https_first.fallback.insert((profile, host.to_owned()));
        }
        let effects = self.items.navigate(id, asked.as_str());
        let _ = self.commit(effects);
        self.https_first.fallen_back.insert(id, asked);
        true
    }

    /// The tab's view closed: a new view's first load that failed is closed
    /// with it. When that load was an https try, the http load goes to a new
    /// view instead of the failure showing: true then.
    pub(super) fn https_first_reopen(&mut self, id: ItemId) -> bool {
        let Some(asked) = self.https_first.fallen_back.remove(&id) else {
            return false;
        };
        let effects = self.items.navigate(id, asked.as_str());
        if effects.is_empty() {
            return false;
        }
        let _ = self.commit(effects);
        true
    }

    /// The tab committed `committed`: an https try of that host is over.
    pub(super) fn https_first_settled(&mut self, id: ItemId, committed: &url::Url) {
        self.https_first.fallen_back.remove(&id);
        if self.https_first.pending.get(&id).is_some_and(|asked| {
            committed.scheme() == "https" && committed.host_str() == asked.host_str()
        }) {
            self.https_first.pending.remove(&id);
        }
    }

    /// The tab is gone.
    pub(super) fn https_first_forget(&mut self, id: ItemId) {
        self.https_first.pending.remove(&id);
        self.https_first.fallen_back.remove(&id);
    }
}
