//! Icons for origins no tab has shown, such as the sources of a Work run.
//! Held and stored rasters are delivered at once; the rest go, a few at a
//! time, to the anonymous origin prober the desktop attaches. An origin that
//! gave nothing is not asked again for an hour.

use super::*;

use crate::store_reads::FAVICON_CACHE_MAX_AGE_SECONDS;
use favicons::TRACKED_ICON_ORIGIN_CAPACITY;
use zephium_ipc::IconSurface;

pub(super) const FAVICON_PROBE_CONCURRENCY: usize = 4;
pub(super) const FAVICON_PROBE_FAILURE_TTL: std::time::Duration =
    std::time::Duration::from_secs(3600);
const FAVICON_PROBE_PENDING_LOOKUPS: usize = 8;

type ProbeKey = (ProfileId, String);

#[derive(Default)]
pub(super) struct FaviconProbeState {
    prober: Option<crate::FaviconProber>,
    queued: std::collections::VecDeque<ProbeKey>,
    in_flight: std::collections::HashSet<ProbeKey>,
    failed: std::collections::HashMap<ProbeKey, std::time::Instant>,
    lookups: std::collections::BTreeMap<u64, ProfileId>,
    lookup_generation: u64,
    /// Surfaces other than chrome that listed an origin before its icon was
    /// held. Chrome is always sent arriving icons; these are sent them once.
    wanted: std::collections::HashMap<ProbeKey, Vec<IconSurface>>,
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
                || probe
                    .failed
                    .get(&key)
                    .is_some_and(|at| now.duration_since(*at) < FAVICON_PROBE_FAILURE_TTL)
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
        while probe.in_flight.len() < FAVICON_PROBE_CONCURRENCY {
            let Some(key) = probe.queued.pop_front() else {
                break;
            };
            probe.in_flight.insert(key.clone());
            prober(key.0, key.1);
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
        let now = std::time::Instant::now();
        let failed = &mut self.favicons.probe.failed;
        failed.retain(|_, at| now.duration_since(*at) < FAVICON_PROBE_FAILURE_TTL);
        if failed.len() >= TRACKED_ICON_ORIGIN_CAPACITY {
            if let Some(oldest) = failed
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(key, _)| key.clone())
            {
                failed.remove(&oldest);
            }
        }
        failed.insert(key, now);
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
