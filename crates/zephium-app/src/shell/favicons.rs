//! Profile-scoped favicon discovery, validation, caching, and persistence.

use super::*;

use crate::store_reads::{PageFavicon, StoredIcon};
use crate::FaviconPageSource;
use zephium_core::icon::IconCandidate;
use zephium_ipc::IconSurface;

pub(super) const TRACKED_ICON_ORIGIN_CAPACITY: usize = 2048;
pub(super) const ICON_CACHE_CAPACITY: usize = 512;
/// A page script can change its address as fast as it likes. A tab looks up
/// at most one page of an origin per interval, and the last address wins.
pub(super) const PAGE_CHECK_GAP: std::time::Duration = std::time::Duration::from_secs(1);
/// The timer attempt that re-examines a tab whose page lookup was deferred.
pub(super) const DEFERRED_PAGE_CHECK: u8 = u8::MAX;
/// The timer attempt that ends the wait for a tab's new icon, so a page that
/// never delivers one stops standing in for the previous page's.
pub(super) const HOLD_EXPIRED: u8 = u8::MAX - 1;
pub(super) const HOLD_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);
const SETTLED_PAGE_CAPACITY: usize = 256;
const SHOWN_ICON_CAPACITY: usize = 1024;
/// Icon addresses that gave nothing are not asked again for an hour, and the
/// oldest give way to new ones, so a long session never stops trying new
/// sites.
pub(super) const FAILED_ICON_CAPACITY: usize = 512;
pub(super) const FAILED_ICON_TTL: std::time::Duration = std::time::Duration::from_secs(3600);
/// Addresses the prober is given for one page, best first.
const FETCHED_ADDRESSES: usize = 3;
/// The most PNG the web engine may hand over for one icon.
const MAX_NATIVE_ICON_BYTES: usize = 256 * 1024;
/// Addresses the page itself is asked to decode, for what the prober cannot
/// reach: vectors, plain http, private hosts and cookie-gated icons.
const RENDERED_ADDRESSES: usize = 3;
pub(super) const STORE_READ_RESULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// What a tab's icon is looked up by: its origin, and its page when the
/// address is short enough to key one. A page whose icon differs from its
/// origin's wears its own; every other page falls back to the origin's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct IconTarget {
    pub(super) profile: ProfileId,
    pub(super) origin: String,
    pub(super) page: Option<String>,
}

/// One tab's search for the icon of its current page.
#[derive(Clone)]
pub(super) struct IconAttempt {
    pub(super) target: IconTarget,
    /// What the page's links name, best first, with its own `/favicon.ico`
    /// last. Empty until the page reports.
    pub(super) ranked: Vec<IconCandidate>,
    /// The page's links name at least one icon.
    pub(super) declared: bool,
    /// The page's watch made its last report.
    pub(super) complete: bool,
    /// The document finished loading.
    pub(super) loaded: bool,
    /// The addresses the prober was last given.
    pub(super) requested: Vec<String>,
    /// The prober's ticket in flight.
    pub(super) fetch: Option<u64>,
    /// The address the page is decoding.
    pub(super) raster: Option<String>,
    /// Addresses the page was asked to decode.
    pub(super) rendered: Vec<String>,
    /// Where the icon on show for this page came from.
    pub(super) found: Option<String>,
    /// The web engine reported its own choice for this page.
    pub(super) native: bool,
}

impl IconAttempt {
    fn new(target: IconTarget) -> Self {
        Self {
            target,
            ranked: Vec::new(),
            declared: false,
            complete: false,
            loaded: false,
            requested: Vec::new(),
            fetch: None,
            raster: None,
            rendered: Vec::new(),
            found: None,
            native: false,
        }
    }
}

enum IconWork {
    /// The page already wears an icon from the first address it names.
    Adopt(String),
    Fetch(Vec<String>),
    Render(String),
    Idle,
}

/// Icon addresses that gave nothing, each for a while. Bounded in both ways:
/// an entry older than the limit is as good as absent, and when the table is
/// full the oldest entry goes, never a new site's chance.
pub(super) struct FailedIcons {
    capacity: usize,
    ttl: std::time::Duration,
    at: std::collections::HashMap<(ProfileId, String), std::time::Instant>,
    /// Oldest first; an entry asked about again moves to the back.
    order: std::collections::VecDeque<(ProfileId, String)>,
}

impl Default for FailedIcons {
    fn default() -> Self {
        Self::with_limits(FAILED_ICON_CAPACITY, FAILED_ICON_TTL)
    }
}

impl FailedIcons {
    pub(super) fn with_limits(capacity: usize, ttl: std::time::Duration) -> Self {
        Self {
            capacity,
            ttl,
            at: Default::default(),
            order: Default::default(),
        }
    }

    pub(super) fn contains(&self, key: &(ProfileId, String), now: std::time::Instant) -> bool {
        self.at
            .get(key)
            .is_some_and(|at| now.saturating_duration_since(*at) < self.ttl)
    }

    pub(super) fn insert(&mut self, key: (ProfileId, String), now: std::time::Instant) {
        if self.at.insert(key.clone(), now).is_some() {
            self.order.retain(|held| held != &key);
        }
        self.order.push_back(key);
        while let Some(oldest) = self.order.front() {
            let expired = self
                .at
                .get(oldest)
                .is_none_or(|at| now.saturating_duration_since(*at) >= self.ttl);
            if !expired && self.at.len() <= self.capacity {
                break;
            }
            if let Some(oldest) = self.order.pop_front() {
                self.at.remove(&oldest);
            }
        }
    }

    pub(super) fn forget_profile(&mut self, profile: ProfileId) {
        self.at.retain(|(owner, _), _| *owner != profile);
        self.order.retain(|(owner, _)| *owner != profile);
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.at.len()
    }
}

/// The icon a tab last drew, kept while its next page's is looked for.
#[derive(Clone)]
struct ShownIcon {
    profile: ProfileId,
    key: String,
    site: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct PendingFaviconStoreRead {
    pub(super) generation: u64,
    pub(super) target: IconTarget,
}

pub(super) struct PendingFaviconBatch {
    pub(super) generation: u64,
    pub(super) profile: ProfileId,
    pub(super) space: SpaceId,
    pub(super) requested: std::collections::HashSet<String>,
}

pub(super) type IconKey = (IconSurface, ProfileId, String);

/// One cached raster: the pixels chrome needs, plus the tag it caches them by.
#[derive(Clone)]
pub(super) struct IconRecord {
    pub(super) revision: String,
    pub(super) encoded: String,
}

#[derive(Default)]
pub(super) struct FaviconState {
    /// Origins whose own raster is settled for this session. A raster that
    /// leaves the bounded cache takes its entry with it.
    pub(super) icons_checked: std::collections::HashSet<(ProfileId, String)>,
    pub(super) icon_values: std::collections::HashMap<(ProfileId, String), IconRecord>,
    pub(super) icon_cache_order: std::collections::VecDeque<(ProfileId, String)>,
    /// The address each held raster came from, so a page naming the same
    /// address as its origin needs no fetch and no row of its own.
    pub(super) icon_sources: std::collections::HashMap<(ProfileId, String), String>,
    /// Revision each privileged surface currently holds for an origin.
    /// Interior mutability because the paths that reference icons take `&self`.
    pub(super) delivered: std::cell::RefCell<std::collections::HashMap<IconKey, String>>,
    surface_profiles: std::cell::RefCell<std::collections::HashMap<IconSurface, ProfileId>>,
    /// References made since the last publish whose pixels the surface lacks.
    pub(super) undelivered: std::cell::RefCell<Vec<IconKey>>,
    /// Profile each surface last heard an icon event for; its address lookups
    /// follow that profile.
    pub(super) announced: std::cell::RefCell<std::collections::HashMap<IconSurface, ProfileId>>,
    pub(super) icon_attempts: std::collections::HashMap<ItemId, IconAttempt>,
    /// Icon addresses the page could not decode.
    pub(super) failed: FailedIcons,
    /// Icon addresses the anonymous prober could not use; the page may still
    /// be able to.
    pub(super) unfetched: FailedIcons,
    /// The page each tab's icon is settled for, so one page is looked up once
    /// however often its address is reported.
    pub(super) settled: std::collections::HashMap<ItemId, IconTarget>,
    /// Tabs whose wait for a new icon ran out, with the page it ran out for.
    pub(super) hold_over: std::collections::HashMap<ItemId, IconTarget>,
    /// What each tab last drew.
    shown: std::cell::RefCell<std::collections::HashMap<ItemId, ShownIcon>>,
    /// When each tab last began looking up a page of an origin. Only recent
    /// entries are kept.
    pub(super) page_checks: std::collections::HashMap<ItemId, (IconTarget, std::time::Instant)>,
    pub(super) store_reads: std::collections::HashMap<ItemId, PendingFaviconStoreRead>,
    pub(super) store_generation: u64,
    pub(super) pending_batch: Option<PendingFaviconBatch>,
    pub(super) batch_generation: u64,
    pub(super) probe: super::favicon_probe::FaviconProbeState,
}

impl Shell {
    fn item_target(&self, id: ItemId) -> Option<IconTarget> {
        let profile = self.profile_of_item(id)?;
        let url = self.items.tab(id).and_then(|t| t.url.as_ref())?;
        Some(IconTarget {
            profile,
            origin: origin_of(url)?,
            page: zephium_core::icon::page_key(url),
        })
    }

    /// Every origin the sidebar is about to draw for this window, then the
    /// pages among them: Essentials, Pinned and Today, folder contents
    /// included.
    fn sidebar_origins(&self, profile: ProfileId, space: SpaceId) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut origins = Vec::new();
        let mut pages = Vec::new();
        let mut frontier: Vec<ItemId> = [
            Placement::Favorites { space },
            Placement::Space {
                space,
                section: SpaceSection::Pinned,
            },
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
        ]
        .into_iter()
        .flat_map(|placement| self.items.roots(placement).iter().copied())
        .collect();
        let mut visited = std::collections::HashSet::new();
        while let Some(id) = frontier.pop() {
            if !visited.insert(id) || origins.len() == MAX_FAVICON_BATCH_ORIGINS {
                continue;
            }
            frontier.extend(self.items.children(id).iter().copied());
            if let Some(url) = self.items.tab(id).and_then(|tab| tab.url.as_ref()) {
                note_sites(url, &mut seen, &mut origins, &mut pages);
            }
        }
        // The switcher stacks the first tabs' icons of the other spaces too.
        for other in self
            .spaces
            .of(profile)
            .into_iter()
            .filter(|other| *other != space)
        {
            for id in self.space_first_tabs(other) {
                if origins.len() == MAX_FAVICON_BATCH_ORIGINS {
                    break;
                }
                if let Some(url) = self.items.tab(id).and_then(|tab| tab.url.as_ref()) {
                    note_sites(url, &mut seen, &mut origins, &mut pages);
                }
            }
        }
        // Origins come first: they are every row's fallback.
        origins.truncate(MAX_FAVICON_BATCH_ORIGINS);
        pages.truncate(MAX_FAVICON_BATCH_ORIGINS - origins.len());
        origins.extend(pages);
        origins
    }

    pub(super) fn hydrate_favicon_cache(&mut self, profile: ProfileId, space: SpaceId) {
        let origins = self.sidebar_origins(profile, space);
        if origins.is_empty() {
            return;
        }
        let requested: std::collections::HashSet<String> = origins.iter().cloned().collect();
        self.favicons.batch_generation = self.favicons.batch_generation.wrapping_add(1);
        if self.favicons.batch_generation == 0 {
            self.favicons.batch_generation = 1;
        }
        let generation = self.favicons.batch_generation;
        self.favicons.pending_batch = Some(PendingFaviconBatch {
            generation,
            profile,
            space,
            requested,
        });
        if let Some(reads) = &self.store_reads {
            if !reads.request_favicon_batch(generation, profile, space, origins) {
                self.favicons.pending_batch = None;
            }
        } else {
            #[cfg(test)]
            {
                let rasters = self.store.favicon_rasters(profile, &origins);
                self.on_store_read(StoreReadResult::FaviconBatch {
                    generation,
                    profile,
                    space,
                    origins,
                    rasters,
                });
            }
            #[cfg(not(test))]
            {
                self.favicons.pending_batch = None;
            }
        }
    }

    pub(super) fn on_favicon_read(
        &mut self,
        generation: u64,
        id: ItemId,
        profile: ProfileId,
        origin: String,
        icon: StoredIcon,
        page: Option<Box<PageFavicon>>,
    ) {
        let target = match self.favicons.store_reads.get(&id) {
            Some(pending)
                if pending.generation == generation
                    && pending.target.profile == profile
                    && pending.target.origin == origin
                    && pending.target.page.as_deref()
                        == page.as_ref().map(|page| page.key.as_str()) =>
            {
                pending.target.clone()
            }
            _ => return,
        };
        self.favicons.store_reads.remove(&id);
        if let Some(queue) = &self.self_queue {
            queue.cancel_favicon(id);
        }
        if self.item_target(id).as_ref() != Some(&target) {
            return;
        }
        let key = (profile, origin);
        let origin_known = self.favicons.icons_checked.contains(&key);
        // A stored raster is drawn whatever its age; age only decides whether
        // the renderer is asked for a newer one.
        let mut shown = false;
        let mut origin_fresh = false;
        if let Some(bytes) = icon.rgba.as_deref().filter(|_| !origin_known) {
            shown = self.cache_icon(key.clone(), bytes);
            origin_fresh = shown && !icon.stale;
        }
        if let Some(page) = &page {
            if let Some(bytes) = page.icon.rgba.as_deref() {
                shown |= self.cache_icon((profile, page.key.clone()), bytes);
            }
        }
        if shown {
            self.project_tab(id);
        }
        if origin_fresh {
            self.favicons.icons_checked.insert(key);
        }
        // A page is judged by its own icon's age; its origin's is refreshed by
        // whichever page matches it next.
        let refresh = match &page {
            Some(page) => page.icon.stale,
            None => icon.stale && !origin_known,
        };
        if refresh {
            self.start_favicon_discovery(id, target);
        } else {
            self.settle_page(id, target);
        }
    }

    pub(super) fn on_favicon_batch_read(
        &mut self,
        generation: u64,
        profile: ProfileId,
        space: SpaceId,
        origins: Vec<String>,
        rasters: Vec<(String, Vec<u8>)>,
    ) {
        let exact = self.favicons.pending_batch.as_ref().is_some_and(|pending| {
            pending.generation == generation && pending.profile == profile && pending.space == space
        });
        if !exact {
            return;
        }
        let Some(pending) = self.favicons.pending_batch.take() else {
            return;
        };
        let origin_set: std::collections::HashSet<_> = origins.iter().cloned().collect();
        if origins.len() > MAX_FAVICON_BATCH_ORIGINS
            || origin_set.len() != origins.len()
            || origin_set != pending.requested
            || self
                .spaces
                .get(space)
                .is_none_or(|candidate| candidate.profile != profile)
        {
            return;
        }

        let mut accepted = std::collections::HashSet::new();
        let mut changed = false;
        for (origin, rgba) in rasters.into_iter().take(MAX_FAVICON_BATCH_ORIGINS) {
            if origin_set.contains(&origin) && accepted.insert(origin.clone()) {
                let key = (profile, origin);
                if self.cache_icon(key, &rgba) {
                    // Batch hydration intentionally does not mark freshness:
                    // it restores an immediate sidebar image, while the next
                    // live navigation still performs the one-query age check
                    // and refreshes an old raster through the renderer.
                    changed = true;
                }
            }
        }
        if changed {
            self.project_items();
        }
        // An Essential is always on screen but may never have been opened, so
        // nothing would ever bring its icon; ask for the ones the store lacks.
        let unmarked: Vec<String> = self
            .items
            .roots(Placement::Favorites { space })
            .iter()
            .filter_map(|id| self.items.tab(*id)?.url.as_ref().map(url::Url::to_string))
            .filter(|url| {
                url::Url::parse(url)
                    .ok()
                    .and_then(|parsed| origin_of(&parsed))
                    .is_some_and(|origin| {
                        !self.favicons.icon_values.contains_key(&(profile, origin))
                    })
            })
            .collect();
        self.want_icons(
            zephium_ipc::IconSurface::Chrome,
            profile,
            unmarked.iter().map(String::as_str),
        );
    }

    pub(super) fn maybe_discover_favicon(&mut self, id: ItemId) {
        let Some(target) = self.item_target(id) else {
            return;
        };
        let searching = self
            .favicons
            .icon_attempts
            .get(&id)
            .is_some_and(|attempt| attempt.target == target);
        if searching || self.icon_work_done(id, &target) || self.defer_page_check(id, &target) {
            return;
        }

        // Persistent profiles can hydrate the already-decoded fixed raster.
        // Private profiles deliberately bypass SQLite but still use the same
        // renderer-side decoder and a bounded in-memory cache.
        if self.incognito_profile(target.profile) {
            self.start_favicon_discovery(id, target);
            return;
        }

        let exact_pending = self
            .favicons
            .store_reads
            .get(&id)
            .is_some_and(|pending| pending.target == target);
        if exact_pending {
            return;
        }
        self.cancel_favicon_attempt(id);
        self.favicons.store_generation = self.favicons.store_generation.wrapping_add(1);
        if self.favicons.store_generation == 0 {
            self.favicons.store_generation = 1;
        }
        let generation = self.favicons.store_generation;
        self.favicons.store_reads.insert(
            id,
            PendingFaviconStoreRead {
                generation,
                target: target.clone(),
            },
        );
        if let Some(reads) = &self.store_reads {
            if reads.request_favicon(
                generation,
                id,
                target.profile,
                target.origin.clone(),
                target.page.clone(),
            ) {
                if let Some(queue) = &self.self_queue {
                    queue.schedule_favicon(
                        id,
                        0,
                        std::time::Instant::now() + STORE_READ_RESULT_TIMEOUT,
                    );
                }
                return;
            }
            self.favicons.store_reads.remove(&id);
        } else {
            #[cfg(test)]
            {
                let read = crate::store_reads::read_favicon(
                    &*self.store,
                    generation,
                    id,
                    target.profile,
                    target.origin,
                    target.page,
                );
                self.on_store_read(read);
                return;
            }
            #[cfg(not(test))]
            {
                self.favicons.store_reads.remove(&id);
            }
        }
        // Store-read pressure must not make favicons permanently disappear;
        // fall back to the already-bounded renderer discovery pipeline.
        if let Some(current) = self.item_target(id) {
            self.start_favicon_discovery(id, current);
        }
    }

    /// A page script can change its address as fast as it likes, and each
    /// address would cost a store read and a renderer pass. Within an origin a
    /// tab starts at most one page lookup per interval; the lookup that was
    /// held back runs when the interval ends, for whichever page the tab shows
    /// by then.
    fn defer_page_check(&mut self, id: ItemId, target: &IconTarget) -> bool {
        if target.page.is_none() {
            return false;
        }
        let now = std::time::Instant::now();
        let checks = &mut self.favicons.page_checks;
        let last = match checks.get(&id) {
            Some((last, _)) if last == target => return false,
            Some((last, at)) if last.origin == target.origin => Some(*at),
            _ => None,
        };
        let Some(at) = last.filter(|at| now.duration_since(*at) < PAGE_CHECK_GAP) else {
            checks.retain(|_, (_, at)| now.duration_since(*at) < PAGE_CHECK_GAP);
            checks.insert(id, (target.clone(), now));
            return false;
        };
        self.cancel_favicon_attempt(id);
        if let Some(queue) = &self.self_queue {
            queue.schedule_favicon(id, DEFERRED_PAGE_CHECK, at + PAGE_CHECK_GAP);
        }
        true
    }

    /// Whether the tab's icon needs no further looking for: its page is
    /// settled, or it has no page of its own and the origin's is.
    fn icon_work_done(&self, id: ItemId, target: &IconTarget) -> bool {
        self.favicons.settled.get(&id) == Some(target)
            || (target.page.is_none()
                && self
                    .favicons
                    .icons_checked
                    .contains(&(target.profile, target.origin.clone())))
    }

    /// Whether the tab is still waiting for the icon of the page it shows, so
    /// the one it drew for the page before stays on until it arrives.
    fn icon_pending(&self, id: ItemId, target: &IconTarget) -> bool {
        !self.icon_work_done(id, target) && self.favicons.hold_over.get(&id) != Some(target)
    }

    fn start_favicon_discovery(&mut self, id: ItemId, target: IconTarget) {
        if self
            .favicons
            .icon_attempts
            .get(&id)
            .is_some_and(|attempt| attempt.target == target)
        {
            return;
        }
        self.cancel_favicon_attempt(id);
        self.favicons
            .icon_attempts
            .insert(id, IconAttempt::new(target));
        let _ = self.engine.discover_favicon(id);
        if let Some(queue) = &self.self_queue {
            queue.schedule_favicon(id, HOLD_EXPIRED, std::time::Instant::now() + HOLD_LIMIT);
        }
    }

    pub(super) fn cancel_favicon_attempt(&mut self, id: ItemId) {
        self.favicons.icon_attempts.remove(&id);
        self.favicons.store_reads.remove(&id);
        self.favicons.settled.remove(&id);
        self.drop_page_job(id);
        if let Some(reads) = &self.store_reads {
            reads.cancel_favicon(id);
        }
        if let Some(queue) = &self.self_queue {
            queue.cancel_favicon(id);
        }
    }

    /// Notes that the tab's icon is final for its page, so repeated URL and
    /// load callbacks for the same page start nothing.
    fn settle_page(&mut self, id: ItemId, target: IconTarget) {
        // Tabs leave without every path telling these maps.
        if self.favicons.settled.len() >= SETTLED_PAGE_CAPACITY {
            let items = &self.items;
            self.favicons
                .settled
                .retain(|id, _| items.tab(*id).is_some());
        }
        self.favicons.hold_over.remove(&id);
        self.favicons.settled.insert(id, target);
    }

    pub(super) fn favicon_load_completed(&mut self, id: ItemId) {
        let current = self.item_target(id);
        match self.favicons.icon_attempts.get_mut(&id) {
            Some(attempt) if Some(&attempt.target) == current.as_ref() => {
                attempt.loaded = true;
                self.drive_icon_attempt(id);
            }
            _ => self.maybe_discover_favicon(id),
        }
    }

    pub(super) fn poll_favicon(&mut self, id: ItemId, attempt: u8) {
        match attempt {
            DEFERRED_PAGE_CHECK => self.maybe_discover_favicon(id),
            HOLD_EXPIRED => self.expire_icon_hold(id),
            0 => {
                let Some(pending) = self.favicons.store_reads.remove(&id) else {
                    return;
                };
                if let Some(reads) = &self.store_reads {
                    reads.cancel_favicon(id);
                }
                if self.item_target(id).as_ref() == Some(&pending.target) {
                    self.start_favicon_discovery(id, pending.target);
                }
            }
            _ => {}
        }
    }

    /// A page that has not delivered an icon for a long while stops standing
    /// in for the previous page's; what it finds later is still drawn.
    fn expire_icon_hold(&mut self, id: ItemId) {
        let Some(target) = self.item_target(id) else {
            return;
        };
        if !self.icon_pending(id, &target) {
            return;
        }
        if self.favicons.hold_over.len() >= SETTLED_PAGE_CAPACITY {
            let items = &self.items;
            self.favicons
                .hold_over
                .retain(|id, _| items.tab(*id).is_some());
        }
        self.favicons.hold_over.insert(id, target);
        self.project_tab(id);
    }

    /// What the page named, once the link report arrives. Reports are whole
    /// snapshots; the newest replaces the last.
    pub(super) fn favicon_candidates(
        &mut self,
        id: ItemId,
        page_url: &str,
        complete: bool,
        candidates: Vec<IconCandidate>,
    ) {
        let Some(page) = self.attempt_page(id, page_url) else {
            return;
        };
        if let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) {
            attempt.declared = !candidates.is_empty();
            attempt.ranked = zephium_core::icon::rank(&page, &candidates);
            attempt.complete |= complete;
        }
        self.drive_icon_attempt(id);
    }

    /// The page behind `page_url` when it is the page this tab's current
    /// attempt is for.
    fn attempt_page(&self, id: ItemId, page_url: &str) -> Option<url::Url> {
        let page = url::Url::parse(page_url).ok()?;
        let target = self.item_target(id)?;
        let attempt = self.favicons.icon_attempts.get(&id)?;
        (attempt.target == target
            && origin_of(&page).as_deref() == Some(target.origin.as_str())
            && zephium_core::icon::page_key(&page) == target.page)
            .then_some(page)
    }

    /// Decides the next step of a tab's search, after anything that could
    /// change it. At most one fetch and one page decode are outstanding.
    fn drive_icon_attempt(&mut self, id: ItemId) {
        let Some(attempt) = self.favicons.icon_attempts.get(&id).cloned() else {
            return;
        };
        if self.item_target(id).as_ref() != Some(&attempt.target) {
            self.cancel_favicon_attempt(id);
            return;
        }
        if attempt.fetch.is_some() || attempt.raster.is_some() {
            return;
        }
        // Where the web engine names the page's icon itself, the declared
        // addresses only back it up: they are used once the page has loaded
        // without it.
        if self.engine.native_favicons() && !attempt.native && !attempt.loaded && !attempt.complete
        {
            return;
        }
        match self.next_icon_work(&attempt) {
            IconWork::Adopt(source) => self.adopt_icon(id, source),
            IconWork::Fetch(addresses) => self.fetch_icon(id, addresses),
            IconWork::Render(address) => self.render_icon(id, address),
            IconWork::Idle if attempt.complete => self.finish_icon_attempt(id),
            IconWork::Idle => {}
        }
    }

    fn next_icon_work(&self, attempt: &IconAttempt) -> IconWork {
        if attempt.native && attempt.found.is_some() {
            return IconWork::Idle;
        }
        let now = std::time::Instant::now();
        let profile = attempt.target.profile;
        let usable: Vec<&IconCandidate> = attempt
            .ranked
            .iter()
            .filter(|candidate| {
                candidate.kind != zephium_core::icon::IconKind::Mask
                    && !self
                        .favicons
                        .failed
                        .contains(&(profile, candidate.url.clone()), now)
            })
            .collect();
        if attempt.found.is_none() {
            if let Some(top) = usable
                .first()
                .filter(|top| self.holds_source(&attempt.target, &top.url))
            {
                return IconWork::Adopt(top.url.clone());
            }
        }
        // The prober takes the best addresses it can read, in ranking order,
        // up to the first one it cannot: a vector, a plain-http or private
        // address, or one that gave it nothing.
        let fetchable: Vec<String> = usable
            .iter()
            .map_while(|candidate| {
                (candidate.bitmap()
                    && candidate.url.starts_with("https://")
                    && !self
                        .favicons
                        .unfetched
                        .contains(&(profile, candidate.url.clone()), now))
                .then(|| candidate.url.clone())
            })
            .take(FETCHED_ADDRESSES)
            .collect();
        if !fetchable.is_empty() && !self.incognito_profile(profile) && self.page_prober_attached()
        {
            // A later report ranking another address first replaces an icon
            // already found; anything else only adds addresses to try.
            let changed = match attempt.found {
                Some(_) => attempt.declared && fetchable.first() != attempt.requested.first(),
                None => fetchable != attempt.requested,
            };
            if changed {
                return IconWork::Fetch(fetchable);
            }
        }
        if attempt.found.is_some() || attempt.rendered.len() >= RENDERED_ADDRESSES {
            return IconWork::Idle;
        }
        // What the prober cannot read, the page decodes: vectors, plain http,
        // private hosts and icons that want the page's cookies. A canvas can
        // read only same-origin pixels, so those go first.
        let mut options: Vec<&&IconCandidate> = usable
            .iter()
            .filter(|candidate| !attempt.rendered.contains(&candidate.url))
            .collect();
        options.sort_by_key(|candidate| {
            url::Url::parse(&candidate.url)
                .ok()
                .and_then(|url| origin_of(&url))
                .as_deref()
                != Some(attempt.target.origin.as_str())
        });
        options.first().map_or(IconWork::Idle, |candidate| {
            IconWork::Render(candidate.url.clone())
        })
    }

    /// Whether the icon held for this page, or for its origin, came from
    /// `address`.
    fn holds_source(&self, target: &IconTarget, address: &str) -> bool {
        let held = |key: &str| {
            let key = (target.profile, key.to_owned());
            self.favicons.icon_values.contains_key(&key)
                && self.favicons.icon_sources.get(&key).map(String::as_str) == Some(address)
        };
        target.page.as_deref().is_some_and(held)
            || (held(&target.origin)
                && self
                    .favicons
                    .icons_checked
                    .contains(&(target.profile, target.origin.clone())))
    }

    /// The first address the page names is already the one its icon came
    /// from: nothing to fetch, and a row of its own would only go stale.
    fn adopt_icon(&mut self, id: ItemId, source: String) {
        let Some(target) = self.favicons.icon_attempts.get_mut(&id).map(|attempt| {
            attempt.found = Some(source.clone());
            attempt.requested = vec![source.clone()];
            attempt.target.clone()
        }) else {
            return;
        };
        let page_held = target.page.as_deref().is_some_and(|page| {
            let key = (target.profile, page.to_owned());
            self.favicons.icon_values.contains_key(&key)
                && self.favicons.icon_sources.get(&key) == Some(&source)
        });
        if let (false, Some(page)) = (page_held, target.page.as_deref()) {
            self.forget_page_icon(target.profile, page);
        }
        self.settle_page(id, target);
        self.project_items();
        self.drive_icon_attempt(id);
    }

    fn fetch_icon(&mut self, id: ItemId, addresses: Vec<String>) {
        let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) else {
            return;
        };
        attempt.requested = addresses.clone();
        let target = attempt.target.clone();
        let ticket = self.queue_page_job(id, target, FaviconPageSource::Addresses(addresses));
        if let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) {
            attempt.fetch = Some(ticket);
        }
    }

    fn render_icon(&mut self, id: ItemId, address: String) {
        let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) else {
            return;
        };
        attempt.rendered.push(address.clone());
        attempt.raster = Some(address.clone());
        let profile = attempt.target.profile;
        if self.engine.rasterize_favicon(id, &address) == NativeDispatch::Scheduled {
            return;
        }
        self.favicons
            .failed
            .insert((profile, address), std::time::Instant::now());
        if let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) {
            attempt.raster = None;
        }
        self.drive_icon_attempt(id);
    }

    /// Nothing more to ask of this page: its icon is whatever it found, and
    /// what it still wears from before does not hold it up.
    fn finish_icon_attempt(&mut self, id: ItemId) {
        let Some(attempt) = self.favicons.icon_attempts.remove(&id) else {
            return;
        };
        if let Some(queue) = &self.self_queue {
            queue.cancel_favicon(id);
        }
        self.drop_page_job(id);
        self.settle_page(id, attempt.target);
        self.project_tab(id);
    }

    /// The answer to a page's fetch.
    pub(super) fn on_page_icon_fetched(
        &mut self,
        id: ItemId,
        target: IconTarget,
        rgba: Option<Vec<u8>>,
        source: Option<String>,
        failed: Vec<String>,
    ) {
        let now = std::time::Instant::now();
        for address in failed {
            self.favicons
                .unfetched
                .insert((target.profile, address), now);
        }
        if self.item_target(id).as_ref() != Some(&target) {
            return;
        }
        if let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) {
            if attempt.target == target {
                attempt.fetch = None;
            }
        }
        if let Some(rgba) = rgba {
            if !self.accept_icon(id, &target, &rgba, source.clone()) {
                if let Some(address) = source {
                    self.favicons
                        .unfetched
                        .insert((target.profile, address), now);
                }
            }
        }
        self.drive_icon_attempt(id);
    }

    /// The page decoded one address into the fixed raster. Where no attempt
    /// asked for it, the raster still belongs to the page it names.
    pub(super) fn favicon_pixels(&mut self, id: ItemId, page_url: &str, rgba: Vec<u8>) {
        let Some(target) = self.item_target(id) else {
            return;
        };
        let Some(source) = url::Url::parse(page_url).ok() else {
            return;
        };
        if origin_of(&source).as_deref() != Some(target.origin.as_str())
            || zephium_core::icon::page_key(&source) != target.page
        {
            return;
        }
        let address = self
            .favicons
            .icon_attempts
            .get_mut(&id)
            .filter(|attempt| attempt.target == target)
            .and_then(|attempt| attempt.raster.take());
        if !self.accept_icon(id, &target, &rgba, address.clone()) {
            if let Some(address) = address {
                self.favicons
                    .failed
                    .insert((target.profile, address), std::time::Instant::now());
            }
        }
        self.drive_icon_attempt(id);
    }

    /// The page could not decode the address it was asked to.
    pub(super) fn favicon_raster_failed(&mut self, id: ItemId, page_url: &str, address: String) {
        let Some(target) = self.item_target(id) else {
            return;
        };
        if self.attempt_page(id, page_url).is_none() {
            return;
        }
        self.favicons
            .failed
            .insert((target.profile, address.clone()), std::time::Instant::now());
        if let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) {
            if attempt.raster.as_deref() == Some(address.as_str()) {
                attempt.raster = None;
            }
        }
        self.drive_icon_attempt(id);
    }

    /// The web engine's own choice for the page, as a PNG. It is decoded like
    /// any fetched icon and, where it works, the declared addresses are not
    /// needed.
    pub(super) fn favicon_image(&mut self, id: ItemId, page_url: &str, png: Vec<u8>) {
        let Some(target) = self.item_target(id) else {
            return;
        };
        let Some(page) = url::Url::parse(page_url).ok() else {
            return;
        };
        if png.is_empty()
            || png.len() > MAX_NATIVE_ICON_BYTES
            || origin_of(&page).as_deref() != Some(target.origin.as_str())
            || zephium_core::icon::page_key(&page) != target.page
            || !self.page_prober_attached()
        {
            return;
        }
        let ticket = self.queue_page_job(id, target.clone(), FaviconPageSource::Encoded(png));
        if let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) {
            if attempt.target == target {
                attempt.native = true;
                attempt.fetch = Some(ticket);
            }
        }
    }

    /// Files the icon a tab's page now wears. False when it cannot be kept.
    fn accept_icon(
        &mut self,
        id: ItemId,
        target: &IconTarget,
        rgba: &[u8],
        source: Option<String>,
    ) -> bool {
        if zephium_core::icon::validated_rgba32(rgba).is_none()
            || self.item_target(id).as_ref() != Some(target)
            || !self.file_page_icon(target, rgba)
        {
            return false;
        }
        if let Some(source) = &source {
            self.note_icon_source(target, rgba, source);
        }
        if let Some(attempt) = self.favicons.icon_attempts.get_mut(&id) {
            if attempt.target == *target {
                attempt.found = Some(source.unwrap_or_default());
            }
        }
        self.settle_page(id, target.clone());
        self.project_items();
        true
    }

    /// Remembers the address whichever key now holds this raster came from.
    fn note_icon_source(&mut self, target: &IconTarget, rgba: &[u8], source: &str) {
        let Some(revision) = zephium_core::icon::revision(rgba) else {
            return;
        };
        for key in target.page.iter().chain(std::iter::once(&target.origin)) {
            let key = (target.profile, key.clone());
            if self
                .favicons
                .icon_values
                .get(&key)
                .is_some_and(|held| held.revision == revision)
            {
                self.favicons.icon_sources.insert(key, source.to_owned());
            }
        }
    }

    /// Files the raster a page declared. It becomes the page's own icon when
    /// it differs from its origin's, which stays the fallback for the pages
    /// that declare nothing; otherwise the origin's icon is the page's.
    fn file_page_icon(&mut self, target: &IconTarget, rgba: &[u8]) -> bool {
        let profile = target.profile;
        let origin_key = (profile, target.origin.clone());
        let Some(revision) = zephium_core::icon::revision(rgba) else {
            return false;
        };
        let origin_icon = self.favicons.icon_values.get(&origin_key);
        let origin_held =
            origin_icon.is_some() && self.favicons.icons_checked.contains(&origin_key);
        let matches_origin = origin_icon.is_some_and(|held| held.revision == revision);
        let page = target.page.as_deref();
        if origin_held && matches_origin {
            // Nothing to write: the origin's row was settled this session.
            if let Some(page) = page {
                self.forget_page_icon(profile, page);
            }
            return true;
        }
        match page {
            Some(page) if origin_held => {
                if !self.cache_icon((profile, page.to_owned()), rgba) {
                    return false;
                }
                self.persist_icon(profile, page, rgba);
                true
            }
            page => {
                if let Some(page) = page {
                    self.forget_page_icon(profile, page);
                }
                self.set_origin_icon(profile, &target.origin, rgba)
            }
        }
    }

    /// Makes `rgba` the icon an origin falls back to. Open pages settled on
    /// the icon it replaces keep that one as their own.
    fn set_origin_icon(&mut self, profile: ProfileId, origin: &str, rgba: &[u8]) -> bool {
        let key = (profile, origin.to_owned());
        let Some(revision) = zephium_core::icon::revision(rgba) else {
            return false;
        };
        if let Some(previous) = self
            .favicons
            .icon_values
            .get(&key)
            .filter(|held| held.revision != revision)
            .cloned()
        {
            self.keep_replaced_icon(profile, origin, &previous);
        }
        if !self.cache_icon(key.clone(), rgba) {
            return false;
        }
        self.favicons.icons_checked.insert(key);
        self.persist_icon(profile, origin, rgba);
        true
    }

    fn keep_replaced_icon(&mut self, profile: ProfileId, origin: &str, previous: &IconRecord) {
        let Some(bytes) = zephium_core::icon::decode_rgba32(&previous.encoded) else {
            return;
        };
        let pages: std::collections::HashSet<String> = self
            .favicons
            .settled
            .iter()
            .filter(|(id, target)| {
                target.profile == profile
                    && target.origin == origin
                    && self.item_target(**id).as_ref() == Some(*target)
            })
            .filter_map(|(_, target)| target.page.clone())
            .filter(|page| {
                !self
                    .favicons
                    .icon_values
                    .contains_key(&(profile, page.clone()))
            })
            .collect();
        for page in pages {
            if self.cache_icon((profile, page.clone()), &bytes) {
                self.persist_icon(profile, &page, &bytes);
            }
        }
    }

    fn forget_page_icon(&mut self, profile: ProfileId, page: &str) {
        let key = (profile, page.to_owned());
        if !self.favicons.icon_values.contains_key(&key) {
            return;
        }
        self.favicons.icon_cache_order.retain(|held| held != &key);
        self.uncache_icon(&key);
        if !self.incognito_profile(profile) {
            self.store.forget_page_favicon(profile, page.to_owned());
        }
    }

    fn persist_icon(&self, profile: ProfileId, key: &str, rgba: &[u8]) {
        if !self.incognito_profile(profile) {
            self.store.save_favicon(
                profile,
                key.to_owned(),
                Some(zephium_core::icon::RGBA32_MIME.to_owned()),
                rgba.to_vec(),
            );
        }
    }

    /// A Work page the agent read discovered its icon: cache it by origin
    /// exactly as a tab's would be, and hand it to chrome.
    pub(super) fn work_page_favicon(&mut self, profile: ProfileId, page_url: &str, rgba: Vec<u8>) {
        let Some(origin) = url::Url::parse(page_url)
            .ok()
            .and_then(|url| origin_of(&url))
        else {
            return;
        };
        if self.profiles.get(profile).is_none() {
            return;
        }
        self.admit_origin_icon(profile, &origin, &rgba);
    }

    /// Caches, stores and delivers one fresh raster for an origin no item
    /// names. The same capacity and age rules apply as for a tab's icon.
    pub(super) fn admit_origin_icon(
        &mut self,
        profile: ProfileId,
        origin: &str,
        rgba: &[u8],
    ) -> bool {
        if zephium_core::icon::validated_rgba32(rgba).is_none()
            || !self.set_origin_icon(profile, origin, rgba)
        {
            return false;
        }
        let _ = self.icon_ref_for(IconSurface::Chrome, profile, origin);
        self.project_items();
        true
    }

    pub(super) fn cache_icon(&mut self, key: (ProfileId, String), rgba: &[u8]) -> bool {
        let (Some(revision), Some(encoded)) = (
            zephium_core::icon::revision(rgba),
            zephium_core::icon::encode_rgba32(rgba),
        ) else {
            return false;
        };
        self.favicons
            .icon_cache_order
            .retain(|candidate| candidate != &key);
        while self.favicons.icon_values.len() >= ICON_CACHE_CAPACITY
            && !self.favicons.icon_values.contains_key(&key)
        {
            let Some(evicted) = self.favicons.icon_cache_order.pop_front() else {
                break;
            };
            self.uncache_icon(&evicted);
        }
        self.favicons
            .icon_values
            .insert(key.clone(), IconRecord { revision, encoded });
        self.favicons.icon_cache_order.push_back(key);
        true
    }

    /// Drops one cached raster and what was known to depend on it.
    fn uncache_icon(&mut self, evicted: &(ProfileId, String)) {
        self.favicons.icon_values.remove(evicted);
        self.favicons
            .delivered
            .borrow_mut()
            .retain(|(_, profile, origin), _| {
                (*profile, origin.as_str()) != (evicted.0, evicted.1.as_str())
            });
        // A raster that leaves the bounded cache loses its settled marker so
        // a later visit may hydrate it from SQLite (or rediscover it for a
        // private profile). A page that wore the raster is looked up again
        // the same way.
        self.favicons.icons_checked.remove(evicted);
        self.favicons.icon_sources.remove(evicted);
        if zephium_core::icon::names_page(&evicted.1) {
            self.favicons.settled.retain(|_, target| {
                target.profile != evicted.0 || target.page.as_deref() != Some(evicted.1.as_str())
            });
        }
    }

    /// Sends each surface the pixels behind every icon reference it does not
    /// already hold at the current revision. Projections name icons rather than
    /// carrying them, so this is the one path rasters travel.
    pub(super) fn publish_icons(&self) {
        let pending = std::mem::take(&mut *self.favicons.undelivered.borrow_mut());
        if pending.is_empty() {
            return;
        }
        let mut delivered = self.favicons.delivered.borrow_mut();
        let mut grouped: std::collections::HashMap<
            (IconSurface, ProfileId),
            Vec<zephium_ipc::FaviconEntry>,
        > = std::collections::HashMap::new();
        for key in pending {
            if self
                .windows
                .focused()
                .is_none_or(|window| window.profile != key.1)
            {
                continue;
            }
            let Some(record) = self.favicons.icon_values.get(&(key.1, key.2.clone())) else {
                continue;
            };
            if delivered.get(&key) == Some(&record.revision) {
                continue;
            }
            delivered.insert(key.clone(), record.revision.clone());
            grouped
                .entry((key.0, key.1))
                .or_default()
                .push(zephium_ipc::FaviconEntry {
                    origin: key.2,
                    revision: record.revision.clone(),
                    rgba: record.encoded.clone(),
                });
        }
        drop(delivered);
        for ((surface, profile), entries) in grouped {
            self.favicons
                .announced
                .borrow_mut()
                .insert(surface, profile);
            (self.emit)(Projection::Favicons(zephium_ipc::FaviconsView {
                surface,
                profile_id: profile.to_string(),
                entries,
            }));
        }
        self.announce_icon_owner();
    }

    /// The chrome surface follows the focused window's profile even when no
    /// raster is due, so a returning profile reads its own icons.
    pub(super) fn announce_icon_owner(&self) {
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return;
        };
        let mut announced = self.favicons.announced.borrow_mut();
        if announced.get(&IconSurface::Chrome) == Some(&profile) {
            return;
        }
        announced.insert(IconSurface::Chrome, profile);
        drop(announced);
        (self.emit)(Projection::Favicons(zephium_ipc::FaviconsView {
            surface: IconSurface::Chrome,
            profile_id: profile.to_string(),
            entries: Vec::new(),
        }));
    }

    /// Names the cached icon for an origin and queues its pixels when the
    /// surface does not already hold that exact revision.
    pub(super) fn icon_ref_for(
        &self,
        surface: IconSurface,
        profile: ProfileId,
        origin: &str,
    ) -> Option<zephium_ipc::IconRef> {
        if self
            .windows
            .focused()
            .is_none_or(|window| window.profile != profile)
        {
            return None;
        }
        self.select_icon_profile(surface, profile);
        let record = self
            .favicons
            .icon_values
            .get(&(profile, origin.to_owned()))?;
        let key = (surface, profile, origin.to_owned());
        if self.favicons.delivered.borrow().get(&key) != Some(&record.revision) {
            let mut undelivered = self.favicons.undelivered.borrow_mut();
            if !undelivered.contains(&key) {
                undelivered.push(key);
            }
        }
        Some(zephium_ipc::IconRef {
            origin: origin.to_owned(),
            revision: record.revision.clone(),
        })
    }

    pub(super) fn select_icon_profile(&self, surface: IconSurface, profile: ProfileId) {
        if self
            .favicons
            .surface_profiles
            .borrow_mut()
            .insert(surface, profile)
            == Some(profile)
        {
            return;
        }
        // Each webview holds one profile's rasters. Returning to a profile must
        // resend its pixels, even though their revisions have not changed.
        self.forget_delivered_icons(surface);
        self.favicons
            .undelivered
            .borrow_mut()
            .retain(|(owner, candidate, _)| *owner != surface || *candidate == profile);
        (self.emit)(Projection::Favicons(zephium_ipc::FaviconsView {
            surface,
            profile_id: profile.to_string(),
            entries: Vec::new(),
        }));
    }

    /// A surface reattached with an empty raster cache, so nothing it
    /// previously received can be assumed present.
    pub(super) fn forget_delivered_icons(&self, surface: IconSurface) {
        self.favicons
            .delivered
            .borrow_mut()
            .retain(|(owner, _, _), _| *owner != surface);
        self.favicons.announced.borrow_mut().remove(&surface);
    }

    /// The icon a tab draws, given the one its current page names, if any.
    /// With none, a tab still waiting on a page of the site it last drew keeps
    /// that icon; anything else shows none.
    pub(super) fn keep_or_hold_icon(
        &self,
        id: ItemId,
        tab: &TabState,
        profile: ProfileId,
        icon: Option<zephium_ipc::IconRef>,
    ) -> Option<zephium_ipc::IconRef> {
        let site = tab
            .url
            .as_ref()
            .and_then(origin_of)
            .and_then(|origin| site_of_origin(&origin));
        let mut shown = self.favicons.shown.borrow_mut();
        if let Some(icon) = icon {
            if let Some(site) = site {
                if shown.len() >= SHOWN_ICON_CAPACITY {
                    shown.retain(|id, _| self.items.tab(*id).is_some());
                }
                shown.insert(
                    id,
                    ShownIcon {
                        profile,
                        key: icon.origin.clone(),
                        site,
                    },
                );
            }
            return Some(icon);
        }
        let kept = site.and_then(|site| {
            let held = shown.get(&id)?;
            let target = self.item_target(id)?;
            (held.profile == profile && held.site == site && self.icon_pending(id, &target))
                .then(|| held.key.clone())
        });
        drop(shown);
        let icon = kept.and_then(|key| self.icon_ref_for(IconSurface::Chrome, profile, &key));
        if icon.is_none() {
            self.favicons.shown.borrow_mut().remove(&id);
        }
        icon
    }

    pub(super) fn incognito_profile(&self, profile: ProfileId) -> bool {
        self.profiles
            .get(profile)
            .is_some_and(|profile| profile.kind == ProfileKind::Incognito)
    }
}

fn note_sites(
    url: &url::Url,
    seen: &mut std::collections::HashSet<String>,
    origins: &mut Vec<String>,
    pages: &mut Vec<String>,
) {
    let Some(origin) = origin_of(url) else {
        return;
    };
    if seen.insert(origin.clone()) {
        origins.push(origin);
    }
    if let Some(page) = zephium_core::icon::page_key(url) {
        if seen.insert(page.clone()) {
            pages.push(page);
        }
    }
}

/// The registrable domain of an origin, so `app.slack.com` and `slack.com`
/// are one site. Hosts without a known suffix (`localhost`, addresses) match
/// only themselves.
fn site_of_origin(origin: &str) -> Option<String> {
    let url = url::Url::parse(origin).ok()?;
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return Some(host);
    }
    Some(
        psl::domain(host.as_bytes())
            .filter(|domain| domain.suffix().is_known())
            .and_then(|domain| std::str::from_utf8(domain.as_bytes()).ok())
            .map_or_else(|| host.clone(), str::to_owned),
    )
}

/// The other spelling of the same host: `www.example.com` for `example.com`
/// and the reverse, under the same scheme and port. These two are one site by
/// every convention, so one's icon stands in for the other's until it has its
/// own. No other subdomain does: `mail.google.com` and `gemini.google.com` are
/// different products.
pub(super) fn apex_pair(origin: &str) -> Option<String> {
    let mut url = url::Url::parse(origin).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return None;
    }
    let other = match host.strip_prefix("www.") {
        Some(bare) if bare.contains('.') => bare.to_owned(),
        Some(_) => return None,
        None if host.contains('.') => format!("www.{host}"),
        None => return None,
    };
    url.set_host(Some(&other)).ok()?;
    Some(url.origin().ascii_serialization())
}

pub(super) fn origin_of(url: &url::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    match url.origin() {
        url::Origin::Tuple(..) => Some(url.origin().ascii_serialization()),
        url::Origin::Opaque(_) => None,
    }
}

#[cfg(test)]
mod pair_tests {
    use super::*;

    #[test]
    fn only_the_apex_and_its_www_are_one_host() {
        assert_eq!(
            apex_pair("https://example.com").as_deref(),
            Some("https://www.example.com")
        );
        assert_eq!(
            apex_pair("https://www.example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            apex_pair("http://www.example.co.uk:8080").as_deref(),
            Some("http://example.co.uk:8080")
        );
        // Every other subdomain is a different product.
        assert_eq!(
            apex_pair("https://mail.google.com").as_deref(),
            Some("https://www.mail.google.com")
        );
        assert_eq!(
            apex_pair("https://www.www.example.com").as_deref(),
            Some("https://www.example.com")
        );
        // Hosts that are not names.
        assert_eq!(apex_pair("https://localhost"), None);
        assert_eq!(apex_pair("http://127.0.0.1:3000"), None);
        assert_eq!(apex_pair("http://[::1]:3000"), None);
        assert_eq!(apex_pair("https://www.localhost"), None);
        assert_eq!(apex_pair("not an origin"), None);
    }
}
