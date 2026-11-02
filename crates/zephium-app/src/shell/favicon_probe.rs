//! Icons for origins no tab has shown, such as the sources of a Work run, and
//! the icon addresses a tab's page declared. Held and stored rasters are
//! delivered at once; the rest go, a few at a time, to the anonymous prober
//! the desktop attaches. An origin that gave nothing is not asked again for an
//! hour.

use super::*;

use crate::store_reads::FAVICON_CACHE_MAX_AGE_SECONDS;
use crate::{FaviconPageSource, FaviconProbe};
use favicons::{FailedIcons, IconTarget, TRACKED_ICON_ORIGIN_CAPACITY};
use zephium_ipc::IconSurface;

pub(super) const FAVICON_PROBE_CONCURRENCY: usize = 4;
pub(super) const FAVICON_PROBE_FAILURE_TTL: std::time::Duration =
    std::time::Duration::from_secs(3600);
const FAVICON_PROBE_PENDING_LOOKUPS: usize = 8;

type ProbeKey = (ProfileId, String);

/// A tab's page icon waiting for, or held by, the prober.
pub(super) struct PageJob {
    pub(super) id: ItemId,
    pub(super) target: IconTarget,
    source: FaviconPageSource,
}

pub(super) struct FaviconProbeState {
    pub(super) prober: Option<crate::FaviconProber>,
    queued: std::collections::VecDeque<ProbeKey>,
    in_flight: std::collections::HashSet<ProbeKey>,
    failed: FailedIcons,
    lookups: std::collections::BTreeMap<u64, ProfileId>,
    lookup_generation: u64,
    /// Surfaces other than chrome that listed an origin before its icon was
    /// held. Chrome is always sent arriving icons; these are sent them once.
    wanted: std::collections::HashMap<ProbeKey, Vec<IconSurface>>,
    /// Page icons by ticket, waiting or in flight; at most one per tab.
    jobs: std::collections::HashMap<u64, PageJob>,
    job_of: std::collections::HashMap<ItemId, u64>,
    queued_jobs: std::collections::VecDeque<u64>,
    in_flight_jobs: std::collections::HashSet<u64>,
    next_ticket: u64,
}

impl Default for FaviconProbeState {
    fn default() -> Self {
        Self {
            prober: None,
            queued: Default::default(),
            in_flight: Default::default(),
            failed: FailedIcons::with_limits(
                TRACKED_ICON_ORIGIN_CAPACITY,
                FAVICON_PROBE_FAILURE_TTL,
            ),
            lookups: Default::default(),
            lookup_generation: 0,
            wanted: Default::default(),
            jobs: Default::default(),
            job_of: Default::default(),
            queued_jobs: Default::default(),
            in_flight_jobs: Default::default(),
            next_ticket: 0,
        }
    }
}

/// The probe fetches over HTTPS only, so only an HTTPS origin can be asked.
fn probe_origin(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    (url.scheme() == "https").then(|| origin_of(&url)).flatten()
}

impl Shell {
    /// Icons for addresses a surface lists without a tab of their own, such as
    /// bookmarks, history and launcher hits. A held raster is already in the
    /// listing; the rest come from the store and, failing that, the network.
    pub(super) fn want_icons<'a>(
        &mut self,
        surface: IconSurface,
        profile: ProfileId,
        urls: impl IntoIterator<Item = &'a str>,
    ) {
        let mut seen = std::collections::HashSet::new();
        let origins: Vec<String> = urls
            .into_iter()
            .filter_map(probe_origin)
            .filter(|origin| {
                !self
                    .favicons
                    .icon_values
                    .contains_key(&(profile, origin.clone()))
            })
            .filter(|origin| seen.insert(origin.clone()))
            .take(MAX_FAVICON_BATCH_ORIGINS)
            .collect();
        if origins.is_empty() {
            return;
        }
        if surface != IconSurface::Chrome {
            let wanted = &mut self.favicons.probe.wanted;
            // Origins skipped by the queue or a recent failure never answer;
            // starting over at the bound keeps those from piling up. A surface
            // that loses its entry is served on its next listing.
            if wanted.len() + origins.len() > TRACKED_ICON_ORIGIN_CAPACITY {
                wanted.clear();
            }
            for origin in &origins {
                let key = (profile, origin.clone());
                let surfaces = wanted.entry(key).or_default();
                if !surfaces.contains(&surface) {
                    surfaces.push(surface);
                }
            }
        }
        self.probe_favicons(profile, origins);
    }

    /// Marks a newly held icon for the surfaces that were waiting on it.
    fn deliver_wanted(&mut self, key: &ProbeKey) {
        for surface in self.favicons.probe.wanted.remove(key).unwrap_or_default() {
            let _ = self.icon_ref_for(surface, key.0, &key.1);
        }
    }

    pub(super) fn attach_favicon_prober(&mut self, prober: crate::FaviconProber) {
        self.favicons.probe.prober.get_or_insert(prober);
        self.pump_favicon_probes();
    }

    pub(super) fn probe_favicons(&mut self, profile: ProfileId, origins: Vec<String>) {
        if self.profiles.get(profile).is_none() {
            return;
        }
        let now = std::time::Instant::now();
        let mut seen = std::collections::HashSet::new();
        let mut missing = Vec::new();
        let mut delivered = false;
        for origin in origins
            .iter()
            .take(MAX_FAVICON_BATCH_ORIGINS)
            .filter_map(|value| probe_origin(value))
        {
            if !seen.insert(origin.clone()) {
                continue;
            }
            if self
                .icon_ref_for(IconSurface::Chrome, profile, &origin)
                .is_some()
            {
                delivered = true;
                continue;
            }
            let key = (profile, origin);
            let probe = &self.favicons.probe;
            if probe.in_flight.contains(&key)
                || probe.queued.contains(&key)
                || probe.failed.contains(&key, now)
            {
                continue;
            }
            missing.push(key.1);
        }
        if delivered {
            self.publish_icons();
        }
        if missing.is_empty() {
            return;
        }
        // A private page's icon comes only from the page itself: a probe is
        // a request from outside its private partition.
        if self.incognito_profile(profile) {
            return;
        }
        self.lookup_stored_favicons(profile, missing);
    }

    fn lookup_stored_favicons(&mut self, profile: ProfileId, origins: Vec<String>) {
        let probe = &mut self.favicons.probe;
        probe.lookup_generation = probe.lookup_generation.wrapping_add(1).max(1);
        let generation = probe.lookup_generation;
        if let Some(reads) = &self.store_reads {
            if probe.lookups.len() >= FAVICON_PROBE_PENDING_LOOKUPS {
                probe.lookups.pop_first();
            }
            probe.lookups.insert(generation, profile);
            if !reads.request_favicon_probe(generation, profile, origins.clone()) {
                self.favicons.probe.lookups.remove(&generation);
                self.queue_favicon_probes(profile, origins);
            }
            return;
        }
        #[cfg(test)]
        {
            probe.lookups.insert(generation, profile);
            let rasters = origins
                .iter()
                .filter_map(|origin| {
                    let (bytes, age) = self.store.favicon_raster_with_age(profile, origin)?;
                    Some((origin.clone(), bytes, age))
                })
                .collect();
            self.on_favicon_probe_read(generation, profile, origins, rasters);
        }
        #[cfg(not(test))]
        self.queue_favicon_probes(profile, origins);
    }

    pub(super) fn on_favicon_probe_read(
        &mut self,
        generation: u64,
        profile: ProfileId,
        origins: Vec<String>,
        rasters: Vec<(String, Vec<u8>, i64)>,
    ) {
        if self.favicons.probe.lookups.remove(&generation) != Some(profile)
            || self.profiles.get(profile).is_none()
        {
            return;
        }
        let requested: std::collections::HashSet<&str> =
            origins.iter().map(String::as_str).collect();
        let mut fresh = std::collections::HashSet::new();
        let mut delivered = false;
        for (origin, rgba, age) in rasters.into_iter().take(MAX_FAVICON_BATCH_ORIGINS) {
            if !requested.contains(origin.as_str())
                || !self.cache_icon((profile, origin.clone()), &rgba)
            {
                continue;
            }
            delivered |= self
                .icon_ref_for(IconSurface::Chrome, profile, &origin)
                .is_some();
            self.deliver_wanted(&(profile, origin.clone()));
            // A stored raster is drawn whatever its age; age only decides
            // whether the origin is asked for a newer one.
            if age <= FAVICON_CACHE_MAX_AGE_SECONDS {
                fresh.insert(origin);
            }
        }
        if delivered {
            self.project_items();
        }
        let stale = origins
            .into_iter()
            .filter(|origin| !fresh.contains(origin))
            .collect();
        self.queue_favicon_probes(profile, stale);
    }

    fn queue_favicon_probes(&mut self, profile: ProfileId, origins: Vec<String>) {
        let probe = &mut self.favicons.probe;
        for origin in origins {
            let key = (profile, origin);
            if probe.queued.len() >= MAX_FAVICON_BATCH_ORIGINS
                || probe.in_flight.contains(&key)
                || probe.queued.contains(&key)
            {
                continue;
            }
            probe.queued.push_back(key);
        }
        self.pump_favicon_probes();
    }

    fn pump_favicon_probes(&mut self) {
        let probe = &mut self.favicons.probe;
        let Some(prober) = probe.prober.clone() else {
            return;
        };
        while probe.in_flight.len() + probe.in_flight_jobs.len() < FAVICON_PROBE_CONCURRENCY {
            // A tab waiting on its page's icon goes before a list's.
            if let Some(ticket) = probe.queued_jobs.pop_front() {
                let Some(job) = probe.jobs.get(&ticket) else {
                    continue;
                };
                probe.in_flight_jobs.insert(ticket);
                prober(FaviconProbe::Page {
                    ticket,
                    profile: job.target.profile,
                    origin: job.target.origin.clone(),
                    source: job.source.clone(),
                });
                continue;
            }
            let Some(key) = probe.queued.pop_front() else {
                break;
            };
            probe.in_flight.insert(key.clone());
            prober(FaviconProbe::Origin {
                profile: key.0,
                origin: key.1,
            });
        }
    }

    /// Whether page icons can be fetched at all.
    pub(super) fn page_prober_attached(&self) -> bool {
        self.favicons.probe.prober.is_some()
    }

    /// Queues the prober's work for one tab's page, replacing any earlier job
    /// of that tab, and returns its ticket.
    pub(super) fn queue_page_job(
        &mut self,
        id: ItemId,
        target: IconTarget,
        source: FaviconPageSource,
    ) -> u64 {
        self.drop_page_job(id);
        let probe = &mut self.favicons.probe;
        probe.next_ticket = probe.next_ticket.wrapping_add(1).max(1);
        let ticket = probe.next_ticket;
        probe.jobs.insert(ticket, PageJob { id, target, source });
        probe.job_of.insert(id, ticket);
        probe.queued_jobs.push_back(ticket);
        self.pump_favicon_probes();
        ticket
    }

    /// Forgets a tab's job. One already with the prober still answers, and
    /// that answer is dropped.
    pub(super) fn drop_page_job(&mut self, id: ItemId) {
        let probe = &mut self.favicons.probe;
        if let Some(ticket) = probe.job_of.remove(&id) {
            probe.jobs.remove(&ticket);
            probe.queued_jobs.retain(|queued| *queued != ticket);
        }
    }

    pub(super) fn favicon_fetched(
        &mut self,
        ticket: u64,
        rgba: Option<Vec<u8>>,
        source: Option<String>,
        failed: Vec<String>,
    ) {
        let probe = &mut self.favicons.probe;
        probe.in_flight_jobs.remove(&ticket);
        let job = probe.jobs.remove(&ticket);
        if let Some(job) = &job {
            if probe.job_of.get(&job.id) == Some(&ticket) {
                probe.job_of.remove(&job.id);
            }
        }
        self.pump_favicon_probes();
        if let Some(job) = job {
            self.on_page_icon_fetched(job.id, job.target, rgba, source, failed);
        }
    }

    pub(super) fn favicon_probed(
        &mut self,
        profile: ProfileId,
        origin: String,
        rgba: Option<Vec<u8>>,
    ) {
        let key = (profile, origin);
        if !self.favicons.probe.in_flight.remove(&key) {
            return;
        }
        let admitted = self.profiles.get(profile).is_some()
            && rgba
                .as_deref()
                .is_some_and(|rgba| self.admit_origin_icon(profile, &key.1, rgba));
        if admitted {
            self.deliver_wanted(&key);
            self.publish_icons();
        } else {
            self.favicons.probe.wanted.remove(&key);
            self.remember_failed_probe(key);
        }
        self.pump_favicon_probes();
    }

    fn remember_failed_probe(&mut self, key: ProbeKey) {
        self.favicons
            .probe
            .failed
            .insert(key, std::time::Instant::now());
    }

    pub(super) fn clear_pending_favicon_probe_lookups(&mut self) {
        let probe = &mut self.favicons.probe;
        probe.lookups.clear();
        probe.wanted.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_origins_are_probed() {
        assert_eq!(
            probe_origin("https://Example.com/path?q=1").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            probe_origin("https://example.com:8443/").as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(probe_origin("http://example.com/"), None);
        assert_eq!(probe_origin("file:///etc/hosts"), None);
        assert_eq!(probe_origin("not a url"), None);
    }
}
