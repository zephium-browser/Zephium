//! Bounded user-owned site preferences. Private profiles use the same model in
//! memory only; the application must never dispatch their mutations to Store.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub const MAX_PAUSED_BLOCKER_SITES: usize = 256;
pub const MAX_PERSONAL_HIDES: usize = 256;
pub const MAX_PERSONAL_RULE_BYTES: usize = 512 * 1024;

/// Immutable, derived native input. It deliberately has no serde encoding:
/// personal/private policy must never enter the public subscription cache.
pub struct PreparedBlockerSites {
    revision: u64,
    entries: BTreeMap<BlockerSite, PreparedBlockerSite>,
}

pub struct PreparedBlockerSite {
    pub paused: bool,
    pub css: Arc<str>,
    pub fingerprint: super::ContentRuleDigest,
}

impl std::fmt::Debug for PreparedBlockerSites {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedBlockerSites")
            .field("revision", &self.revision)
            .field("site_count", &self.entries.len())
            .finish()
    }
}

impl PreparedBlockerSites {
    /// Selectors must have passed grammar validation before constructing this
    /// bounded transport. Fingerprints identify content, never browsing events.
    pub fn new(
        revision: u64,
        entries: impl IntoIterator<Item = (BlockerSite, bool, Arc<str>)>,
    ) -> Option<Arc<Self>> {
        if revision == 0 || revision > i64::MAX as u64 {
            return None;
        }
        let mut output = BTreeMap::new();
        let mut bytes = 0usize;
        for (site, paused, css) in entries {
            bytes = bytes.checked_add(site.0.len() + css.len())?;
            if bytes > 1024 * 1024 || output.len() >= MAX_PAUSED_BLOCKER_SITES + MAX_PERSONAL_HIDES
            {
                return None;
            }
            let mut digest = Sha256::new();
            digest.update(b"zephium-personal-site-policy-v1");
            digest.update(css.as_bytes());
            let fingerprint = super::ContentRuleDigest::from_bytes(digest.finalize().into());
            if output
                .insert(
                    site,
                    PreparedBlockerSite {
                        paused,
                        css,
                        fingerprint,
                    },
                )
                .is_some()
            {
                return None;
            }
        }
        Some(Arc::new(Self {
            revision,
            entries: output,
        }))
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn get(&self, site: &BlockerSite) -> Option<&PreparedBlockerSite> {
        self.entries.get(site)
    }
    pub fn paused_sites(&self) -> impl Iterator<Item = &BlockerSite> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.paused)
            .map(|(site, _)| site)
    }
    pub fn same_content(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self.entries.iter().all(|(site, value)| {
                other.entries.get(site).is_some_and(|other| {
                    value.paused == other.paused && value.fingerprint == other.fingerprint
                })
            })
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BlockerSite(String);

impl BlockerSite {
    /// Site controls default to one exact canonical hostname, across its
    /// schemes and ports. They do not implicitly include sibling/subdomains.
    pub fn from_url(value: &str) -> Option<Self> {
        if value.len() > super::MAX_NETWORK_REQUEST_URL_BYTES {
            return None;
        }
        let url = url::Url::parse(value).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = url.host_str()?.trim_end_matches('.');
        if host.is_empty() || host.len() > 253 {
            return None;
        }
        Some(Self(host.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BlockerSite {
    type Error = SitePreferenceError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let site =
            Self::from_url(&format!("https://{value}/")).ok_or(SitePreferenceError::InvalidSite)?;
        if site.0 != value {
            return Err(SitePreferenceError::InvalidSite);
        }
        Ok(site)
    }
}

impl From<BlockerSite> for String {
    fn from(site: BlockerSite) -> String {
        site.0
    }
}

impl std::fmt::Debug for BlockerSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BlockerSite(<private>)")
    }
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PersonalHide {
    pub id: u64,
    pub site: BlockerSite,
    pub selector: String,
    pub label: String,
    pub enabled: bool,
}

impl std::fmt::Debug for PersonalHide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonalHide")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct SitePreferencesWire {
    version: u32,
    revision: u64,
    next_hide_id: u64,
    paused: BTreeSet<BlockerSite>,
    hides: Vec<PersonalHide>,
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(try_from = "SitePreferencesWire", into = "SitePreferencesWire")]
pub struct BlockerSitePreferences(SitePreferencesWire);

impl Default for BlockerSitePreferences {
    fn default() -> Self {
        Self(SitePreferencesWire {
            version: 1,
            revision: 1,
            next_hide_id: 1,
            paused: BTreeSet::new(),
            hides: Vec::new(),
        })
    }
}

impl std::fmt::Debug for BlockerSitePreferences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockerSitePreferences")
            .field("revision", &self.0.revision)
            .field("paused_count", &self.0.paused.len())
            .field("hide_count", &self.0.hides.len())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum SitePreferenceChange {
    Pause { site: BlockerSite, paused: bool },
    AddHide(PersonalHide),
    SetHideEnabled { id: u64, enabled: bool },
    RemoveHide { id: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SitePreferenceError {
    InvalidSite,
    InvalidRule,
    UnknownRule,
    ResourceLimit,
    RevisionExhausted,
}

impl std::fmt::Display for SitePreferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "site preference rejected: {self:?}")
    }
}
impl std::error::Error for SitePreferenceError {}

impl BlockerSitePreferences {
    pub fn revision(&self) -> u64 {
        self.0.revision
    }
    pub fn paused(&self, site: &BlockerSite) -> bool {
        self.0.paused.contains(site)
    }
    pub fn hides(&self) -> &[PersonalHide] {
        &self.0.hides
    }

    pub fn paused_sites(&self) -> impl Iterator<Item = &BlockerSite> {
        self.0.paused.iter()
    }

    /// The application validates selector grammar through the blocker before
    /// admission, and again on restore. This model enforces persistence bounds
    /// and identities, not platform-specific CSS syntax.
    pub fn changed(&self, change: SitePreferenceChange) -> Result<Self, SitePreferenceError> {
        let mut next = self.clone();
        match change {
            SitePreferenceChange::Pause { site, paused } => {
                if self.paused(&site) == paused {
                    return Ok(self.clone());
                }
                if paused {
                    next.0.paused.insert(site);
                } else {
                    next.0.paused.remove(&site);
                }
            }
            SitePreferenceChange::AddHide(mut hide) => {
                if let Some(existing) = self.0.hides.iter().find(|existing| {
                    existing.site == hide.site && existing.selector == hide.selector
                }) {
                    return self.changed(SitePreferenceChange::SetHideEnabled {
                        id: existing.id,
                        enabled: true,
                    });
                }
                hide.id = next.0.next_hide_id;
                next.0.next_hide_id = next
                    .0
                    .next_hide_id
                    .checked_add(1)
                    .ok_or(SitePreferenceError::RevisionExhausted)?;
                next.0.hides.push(hide);
            }
            SitePreferenceChange::SetHideEnabled { id, enabled } => {
                if self
                    .0
                    .hides
                    .iter()
                    .any(|hide| hide.id == id && hide.enabled == enabled)
                {
                    return Ok(self.clone());
                }
                next.0
                    .hides
                    .iter_mut()
                    .find(|h| h.id == id)
                    .ok_or(SitePreferenceError::UnknownRule)?
                    .enabled = enabled;
            }
            SitePreferenceChange::RemoveHide { id } => {
                let index = next
                    .0
                    .hides
                    .iter()
                    .position(|h| h.id == id)
                    .ok_or(SitePreferenceError::UnknownRule)?;
                next.0.hides.remove(index);
            }
        }
        next.0.revision = next
            .0
            .revision
            .checked_add(1)
            .filter(|r| *r <= i64::MAX as u64)
            .ok_or(SitePreferenceError::RevisionExhausted)?;
        Self::try_from(next.0)
    }
}

impl TryFrom<SitePreferencesWire> for BlockerSitePreferences {
    type Error = SitePreferenceError;
    fn try_from(wire: SitePreferencesWire) -> Result<Self, Self::Error> {
        if wire.version != 1
            || wire.revision == 0
            || wire.revision > i64::MAX as u64
            || wire.next_hide_id == 0
        {
            return Err(SitePreferenceError::InvalidRule);
        }
        if wire.paused.len() > MAX_PAUSED_BLOCKER_SITES || wire.hides.len() > MAX_PERSONAL_HIDES {
            return Err(SitePreferenceError::ResourceLimit);
        }
        let mut ids = BTreeSet::new();
        let mut bytes: usize = wire.paused.iter().map(|s| s.0.len()).sum();
        for hide in &wire.hides {
            if hide.id == 0
                || hide.id >= wire.next_hide_id
                || !ids.insert(hide.id)
                || hide.selector.is_empty()
                || hide.selector.len() > 4096
                || hide.selector.chars().any(char::is_control)
                || hide.label.trim().is_empty()
                || hide.label.len() > 256
                || hide.label.chars().any(char::is_control)
            {
                return Err(SitePreferenceError::InvalidRule);
            }
            bytes += hide.site.0.len() + hide.selector.len() + hide.label.len();
        }
        if bytes > MAX_PERSONAL_RULE_BYTES {
            return Err(SitePreferenceError::ResourceLimit);
        }
        Ok(Self(wire))
    }
}

impl From<BlockerSitePreferences> for SitePreferencesWire {
    fn from(value: BlockerSitePreferences) -> Self {
        value.0
    }
}

/// `program` for a profile with paused sites. A frame whose top-level
/// document is on a paused site runs nothing, embeds included, as the paused
/// site's network rules are off for the whole view. Hosts match as
/// [`BlockerSite::from_url`] reads them.
pub fn scriptlet_program_with_pauses<'a>(
    program: &str,
    paused: impl IntoIterator<Item = &'a BlockerSite>,
) -> String {
    let paused: Vec<&str> = paused.into_iter().map(BlockerSite::as_str).collect();
    if paused.is_empty() {
        return program.to_owned();
    }
    let hosts = serde_json::to_string(&paused).unwrap_or_else(|_| "[]".into());
    format!(
        "(() => {{\nconst origins = location.ancestorOrigins;\nlet top = location.hostname;\nif (origins && origins.length > 0) {{ try {{ top = new URL(origins[origins.length - 1]).hostname; }} catch (e) {{ return; }} }}\nif (new Set({hosts}).has(top.replace(/\\.$/, ''))) {{ return; }}\n{program}}})();\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_without_pauses_is_unchanged_and_pauses_are_data() {
        assert_eq!(scriptlet_program_with_pauses("run();", []), "run();");
        let sites = [
            BlockerSite::try_from("b.example".to_owned()).unwrap(),
            BlockerSite::try_from("[::1]".to_owned()).unwrap(),
        ];
        let wrapped = scriptlet_program_with_pauses("run();", &sites);
        assert!(wrapped.contains(r#"new Set(["b.example","[::1]"])"#));
        assert!(wrapped.contains("location.ancestorOrigins"));
        assert!(wrapped.ends_with("run();})();\n"));
    }

    #[test]
    fn site_scope_is_exact_canonical_host_not_domain_suffix() {
        let site = BlockerSite::from_url("https://WWW.Example.com.:8443/path").unwrap();
        assert_eq!(site.as_str(), "www.example.com");
        let prefs = BlockerSitePreferences::default()
            .changed(SitePreferenceChange::Pause {
                site: site.clone(),
                paused: true,
            })
            .unwrap();
        assert!(prefs.paused(&BlockerSite::from_url("http://www.example.com/other").unwrap()));
        assert!(!prefs.paused(&BlockerSite::from_url("https://example.com/").unwrap()));
        assert!(!prefs.paused(&BlockerSite::from_url("https://notwww.example.com/").unwrap()));
        assert!(BlockerSite::try_from("example.com:8443".to_owned()).is_err());
        assert!(BlockerSite::from_url("file:///example.com/").is_none());
    }

    #[test]
    fn removed_hide_ids_are_never_reused_and_pause_preserves_personal_hides() {
        let hide = PersonalHide {
            id: 999,
            site: BlockerSite::from_url("https://example.com/").unwrap(),
            selector: ".banner".into(),
            label: "Banner".into(),
            enabled: true,
        };
        let first = BlockerSitePreferences::default()
            .changed(SitePreferenceChange::AddHide(hide.clone()))
            .unwrap();
        assert_eq!(first.hides()[0].id, 1);
        let paused = first
            .changed(SitePreferenceChange::Pause {
                site: hide.site.clone(),
                paused: true,
            })
            .unwrap();
        assert_eq!(paused.hides(), first.hides());
        let removed = paused
            .changed(SitePreferenceChange::RemoveHide { id: 1 })
            .unwrap();
        let second = removed
            .changed(SitePreferenceChange::AddHide(hide))
            .unwrap();
        assert_eq!(second.hides()[0].id, 2);
        assert!(second.revision() > removed.revision());
    }
}
