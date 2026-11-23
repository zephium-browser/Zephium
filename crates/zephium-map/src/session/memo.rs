use super::*;
use crate::results::{MAX_SOCIAL, TITLE_BYTES};
use crate::site::{MAX_CHILDREN, MAX_SECTIONS};
use serde::{Deserialize, Serialize};

pub const MEMO_VERSION: u16 = 3;
pub const MAX_MEMO_BYTES: usize = 256 * 1024;
pub const MEMO_TTL: u64 = 24 * 60 * 60;
#[derive(Clone, Serialize, Deserialize)]
pub struct SiteMemo {
    pub version: u16,
    pub read_at: u64,
    pub payload: SitePayload,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SitePayload {
    pub place: Place,
    pub robots: BTreeMap<String, Robots>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum MemoError {
    Limit,
    Version,
    Invalid,
}
impl SiteMemo {
    pub fn decode(raw: &[u8], site: &PlaceKey) -> Result<Self, MemoError> {
        if raw.len() > MAX_MEMO_BYTES {
            return Err(MemoError::Limit);
        }
        #[derive(Deserialize)]
        struct Header {
            version: u16,
        }
        let header: Header = serde_json::from_slice(raw).map_err(|_| MemoError::Invalid)?;
        if header.version != MEMO_VERSION {
            return Err(MemoError::Version);
        }
        let memo: Self = serde_json::from_slice(raw).map_err(|_| MemoError::Invalid)?;
        memo.validate(site)?;
        Ok(memo)
    }
    fn validate(&self, site: &PlaceKey) -> Result<(), MemoError> {
        let p = &self.payload.place;
        if &p.key != site
            || PlaceKey::from_url(p.entry.as_url()).as_ref() != Some(site)
            || site.root(p.entry.as_url()).ok().as_ref() != Some(&p.entry)
            || p.check_ids().is_err()
            || SiteKey::from_url(p.entry.as_url()).as_ref() != Some(&p.site)
            || p.title.len() > TITLE_BYTES
            || p.focus.is_some()
            || p.notes.shell
            || p.notes.wall
            || p.sections
                .iter()
                .filter(|e| e.tier == site::Tier::Primary)
                .count()
                > site::MAX_PRIMARY_SECTIONS
            || p.sections
                .iter()
                .filter(|e| e.tier == site::Tier::Secondary)
                .count()
                > site::MAX_SECONDARY_SECTIONS
            || p.sections
                .windows(2)
                .any(|pair| pair[0].tier > pair[1].tier)
            || p.sections.iter().map(|e| e.children.len()).sum::<usize>() > site::MAX_PLACE_CHILDREN
            || p.sections.len() > MAX_SECTIONS
            || p.social.len() > MAX_SOCIAL
            || !p.social.iter().all(|v| {
                v.basis != crate::results::ProfileBasis::Results
                    && crate::results::site_profile(&v.url) == Some(v.network)
                    && PageKey::from_url(&v.url).is_ok()
            })
            || !p.sections.iter().all(|e| {
                valid_entry(e, p, None) && e.children.iter().all(|c| valid_entry(c, p, Some(e)))
            })
            || self.payload.robots.len() > MAX_FETCHES
            || !self.payload.robots.iter().all(|(origin, r)| {
                Url::parse(origin).is_ok_and(|u| {
                    u.origin().ascii_serialization() == *origin
                        && SiteKey::from_url(&u).as_ref() == Some(&p.site)
                }) && r.valid()
            })
        {
            return Err(MemoError::Invalid);
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, MemoError> {
        if self.version != MEMO_VERSION {
            return Err(MemoError::Version);
        }
        self.validate(&self.payload.place.key)?;
        let mut bytes = Vec::new();
        struct Limited<'a>(&'a mut Vec<u8>);
        impl std::io::Write for Limited<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if b.len() > MAX_MEMO_BYTES - self.0.len() {
                    return Err(std::io::ErrorKind::FileTooLarge.into());
                }
                self.0.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(Limited(&mut bytes), self).map_err(|_| MemoError::Limit)?;
        Ok(bytes)
    }
}
fn valid_entry(e: &Entry, place: &Place, parent: Option<&Entry>) -> bool {
    let Some(site) = SiteKey::from_url(e.key.as_url()) else {
        return false;
    };
    let delegated =
        parent.is_some_and(|p| SiteKey::from_url(p.key.as_url()).as_ref() == Some(&site));
    let site_nav = e
        .sources
        .iter()
        .any(|s| matches!(s, Source::PrimaryNav | Source::MenuGroup | Source::Footer));
    if site != place.site && !delegated && !(site_nav && site::external_role(e.role)) {
        return false;
    }
    if e.off_site != (site != place.site) {
        return false;
    }

    e.read.is_none()
        && !e.label.is_empty()
        && e.label.len() <= TITLE_BYTES
        && e.group.as_ref().is_none_or(|s| s.len() <= TITLE_BYTES)
        && !e.sources.is_empty()
        && !e
            .sources
            .iter()
            .any(|s| matches!(s, Source::EngineResult | Source::EngineSitelink))
        && e.sources.iter().collect::<BTreeSet<_>>().len() == e.sources.len()
        && e.children.len() <= if parent.is_none() { MAX_CHILDREN } else { 0 }
}
