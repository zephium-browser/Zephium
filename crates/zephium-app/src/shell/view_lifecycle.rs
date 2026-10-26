//! Bounded native-view residency, discard, crash, and process recovery.

use super::recent_tabs::RecentTabs;
use super::*;
use zephium_core::ports::engine::MemoryPressure;

// The few most recently used hidden pages stay warm for instant switching;
// older hidden pages sleep after the chosen idle grace, whatever the tab
// count. Above the pressure watermark, under critical OS pressure, or while a
// foreground page waits for a slot, the least recently used page goes first
// without waiting. The absolute ceiling mirrors the engine's native tab
// budget; it is a backstop for kept-awake and protected pages, while OS
// memory pressure is the real budget. Pages are never force-discarded: the
// engine probe still vetoes anything a reload would lose.
pub(super) const WARM_VIEW_LIMIT: usize = 4;
pub(super) const LIVE_VIEW_PRESSURE_LIMIT: usize = 24;
pub(super) const LIVE_VIEW_ABSOLUTE_LIMIT: usize = 64;
const _: () = assert!(LIVE_VIEW_ABSOLUTE_LIMIT >= LIVE_VIEW_PRESSURE_LIMIT + MAX_VISIBLE_PANES);
pub(super) const MAX_CONCURRENT_DISCARD_PROBES: usize = 4;
pub(super) const DISCARD_IDLE_GRACE: std::time::Duration = std::time::Duration::from_secs(15 * 60);
// Suspension keeps page state, so it can start well before a discard.
const DORMANT_GRACE: std::time::Duration = std::time::Duration::from_secs(5 * 60);
// A system memory warning can last for hours on small machines; a short
// grace keeps tabs the user is actively switching between from reloading.
const WARNING_IDLE_GRACE: std::time::Duration = std::time::Duration::from_secs(2 * 60);
// A few pages hold most of a session's memory. One whose renderer exceeds the
// heavy threshold sleeps after a short grace, even inside the warm set; only
// the most recent hidden page stays for instant back-and-forth.
const HEAVY_PAGE: (u64, std::time::Duration) = (256 << 20, std::time::Duration::from_secs(3 * 60));
const MEMORY_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
pub(super) const DISCARD_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub(super) const DISCARD_PROTECTED_RETRY: std::time::Duration =
    std::time::Duration::from_secs(5 * 60);
const CAPACITY_WAIT: std::time::Duration = std::time::Duration::from_secs(4);

pub(super) struct CapacityRequest {
    id: ItemId,
    profile: ProfileId,
    window: WindowId,
    space: SpaceId,
    url: String,
    pub(super) deadline: std::time::Instant,
}

#[derive(Clone)]
pub(super) enum PendingDiscardProbe {
    Probing {
        probe: DiscardProbeId,
        committed_url: String,
        deadline: std::time::Instant,
    },
    Closing {
        probe: DiscardProbeId,
        recreate: bool,
        deferred_navigation: Option<String>,
        reload_on_refusal: bool,
    },
}

pub(super) struct ResidencyState {
    sleeping: bool,
    exceptions: Vec<String>,
    memory_pressure: MemoryPressure,
    preferred_limits: (usize, usize),
    pub(super) heavy_page: (u64, std::time::Duration),
    pub(super) page_bytes: std::collections::HashMap<ItemId, u64>,
    memory_sampled: Option<std::time::Instant>,
    pub(super) capacity_requests: std::collections::VecDeque<CapacityRequest>,
    capacity_blocked: std::collections::HashMap<ItemId, String>,
    pub(super) recent: RecentTabs,
    pub(super) last_focus: std::collections::HashMap<ItemId, std::time::Instant>,
    pub(super) resident_since: std::collections::HashMap<ItemId, std::time::Instant>,
    pub(super) inactive_since: std::collections::HashMap<ItemId, std::time::Instant>,
    last_protected: std::collections::HashSet<ItemId>,
    last_shown: std::collections::HashSet<ItemId>,
    pub(super) dormant_min: std::time::Duration,
    pub(super) dormant_sent: Vec<ItemId>,
    pub(super) discard_idle_min: std::time::Duration,
    pub(super) discard_probe_timeout: std::time::Duration,
    pub(super) discard_protected_retry: std::time::Duration,
    pub(super) warm_view_limit: usize,
    pub(super) live_view_pressure_limit: usize,
    pub(super) next_discard_probe: u64,
    pub(super) discard_probes: std::collections::HashMap<ItemId, PendingDiscardProbe>,
    pub(super) discard_protected_until: std::collections::HashMap<ItemId, std::time::Instant>,
}

impl Default for ResidencyState {
    fn default() -> Self {
        Self {
            sleeping: true,
            exceptions: Vec::new(),
            memory_pressure: MemoryPressure::Normal,
            preferred_limits: (WARM_VIEW_LIMIT, LIVE_VIEW_PRESSURE_LIMIT),
            heavy_page: HEAVY_PAGE,
            page_bytes: std::collections::HashMap::new(),
            memory_sampled: None,
            capacity_requests: std::collections::VecDeque::new(),
            capacity_blocked: std::collections::HashMap::new(),
            recent: RecentTabs::default(),
            last_focus: std::collections::HashMap::new(),
            resident_since: std::collections::HashMap::new(),
            inactive_since: std::collections::HashMap::new(),
            last_protected: std::collections::HashSet::new(),
            last_shown: std::collections::HashSet::new(),
            dormant_min: DORMANT_GRACE,
            dormant_sent: Vec::new(),
            discard_idle_min: DISCARD_IDLE_GRACE,
            discard_probe_timeout: DISCARD_PROBE_TIMEOUT,
            discard_protected_retry: DISCARD_PROTECTED_RETRY,
            warm_view_limit: WARM_VIEW_LIMIT,
            live_view_pressure_limit: LIVE_VIEW_PRESSURE_LIMIT,
            next_discard_probe: 0,
            discard_probes: std::collections::HashMap::new(),
            discard_protected_until: std::collections::HashMap::new(),
        }
    }
}

impl ResidencyState {
    pub(super) fn load(store: &dyn zephium_core::ports::store::Store) -> Self {
        let mut state = Self::default();
        for key in [
            "performance.sleep",
            "performance.after",
            "performance.memory",
            "performance.exceptions",
        ] {
            if let Some(value) = store.app_setting(key) {
                state.apply_setting(key, &value);
            }
        }
        state
    }

    fn apply_setting(&mut self, key: &str, value: &str) {
        if !zephium_core::preferences::value_allowed(key, value) {
            return;
        }
        match key {
            "performance.sleep" => self.sleeping = value == "true",
            "performance.after" => {
                let minutes = value.parse::<u64>().unwrap_or(15);
                self.discard_idle_min = std::time::Duration::from_secs(minutes * 60);
                self.dormant_min = self.discard_idle_min.min(DORMANT_GRACE);
            }
            "performance.memory" => {
                self.preferred_limits = match value {
                    "save-memory" => (2, 12),
                    "keep-ready" => (10, 28),
                    _ => (WARM_VIEW_LIMIT, LIVE_VIEW_PRESSURE_LIMIT),
                };
                self.heavy_page = match value {
                    "save-memory" => (160 << 20, std::time::Duration::from_secs(60)),
                    "keep-ready" => (512 << 20, std::time::Duration::from_secs(10 * 60)),
                    _ => HEAVY_PAGE,
                };
                self.update_limits();
            }
            "performance.exceptions" => {
                self.exceptions = value.lines().map(str::to_owned).collect()
            }
            _ => {}
        }
    }

    fn update_limits(&mut self) {
        let (warm, pressure) = self.preferred_limits;
        (self.warm_view_limit, self.live_view_pressure_limit) = match self.memory_pressure {
            MemoryPressure::Normal => (warm, pressure),
            MemoryPressure::Warning => (warm.min(2), pressure.min(12)),
            MemoryPressure::Critical => (0, pressure.min(4)),
        };
    }

    fn heavy_grace(&self) -> std::time::Duration {
        match self.memory_pressure {
            MemoryPressure::Normal => self.heavy_page.1,
            MemoryPressure::Warning | MemoryPressure::Critical => {
                self.heavy_page.1.min(WARNING_IDLE_GRACE)
            }
        }
    }

    fn idle_grace(&self) -> std::time::Duration {
        match self.memory_pressure {
            MemoryPressure::Normal => self.discard_idle_min,
            MemoryPressure::Warning | MemoryPressure::Critical => {
                self.discard_idle_min.min(WARNING_IDLE_GRACE)
            }
        }
    }
}

pub(super) struct Warmth {
    warm: std::collections::HashSet<ItemId>,
    // The most recent hidden page, kept even when heavy.
    instant: Option<ItemId>,
}

#[derive(Default)]
pub(super) struct CrashState {
    pub(super) crashes: std::collections::HashMap<ItemId, std::time::Instant>,
    pub(super) presentations: std::collections::HashSet<ItemId>,
}

impl Shell {
    pub(super) fn apply_performance_setting(&mut self, key: &str, value: &str) {
        if key.starts_with("performance.") {
            self.residency.apply_setting(key, value);
            self.maintain_views();
        }
    }

    pub(super) fn on_page_memory(&mut self, id: ItemId, profile: ProfileId, bytes: u64) {
        if self.profile_of_item(id) == Some(profile)
            && self.items.tab(id).is_some_and(TabState::has_view)
        {
            self.residency.page_bytes.insert(id, bytes);
        }
    }

    pub(super) fn on_memory_pressure(&mut self, pressure: MemoryPressure) {
        if self.residency.memory_pressure != pressure {
            self.residency.memory_pressure = pressure;
            self.engine.set_memory_pressure(pressure);
            self.residency.update_limits();
            self.maintain_views();
        }
    }

    fn site_kept_awake(&self, id: ItemId) -> bool {
        self.items
            .tab(id)
            .and_then(|tab| tab.url.as_ref())
            .and_then(|url| url.host_str())
            .is_some_and(|host| {
                self.residency
                    .exceptions
                    .iter()
                    .any(|site| zephium_core::time::site_covers(site, host.trim_end_matches('.')))
            })
    }

    /// Only a foreground intent may wait for an acknowledged safe close. The
    /// queue is bounded by visible panes and never makes an unsafe page eligible.
    pub(super) fn wait_for_view_capacity(&mut self, id: ItemId, url: String) -> bool {
        if !self.discard_protected_leaves().contains(&id) {
            return false;
        }
        let Some(window) = self.windows.focused() else {
            return false;
        };
        let Some(profile) = self.profile_of_item(id) else {
            return false;
        };
        let (window, space) = (window.id, window.space);
        if let Some(request) = self
            .residency
            .capacity_requests
            .iter_mut()
            .find(|request| request.id == id)
        {
            request.url = url;
            request.profile = profile;
            request.window = window;
            request.space = space;
            request.deadline = std::time::Instant::now() + CAPACITY_WAIT;
        } else {
            if self.residency.capacity_requests.len() >= MAX_VISIBLE_PANES {
                return false;
            }
            self.residency.capacity_requests.push_back(CapacityRequest {
                id,
                profile,
                window,
                space,
                url,
                deadline: std::time::Instant::now() + CAPACITY_WAIT,
            });
        }
        self.rollback_capacity_create(id);
        self.residency.capacity_blocked.remove(&id);
        if let Some(queue) = &self.self_queue {
            queue.schedule_view_capacity(id, std::time::Instant::now() + CAPACITY_WAIT);
        }
        true
    }

    pub(super) fn rollback_capacity_create(&mut self, id: ItemId) {
        self.record_view_retirement(id);
        let title = self.items.tab(id).map(|tab| tab.title.clone());
        self.items.view_creation_failed(id);
        if let Some(title) = title {
            self.items.set_title(id, title);
        }
    }

    pub(super) fn mark_capacity_blocked(&mut self, id: ItemId, url: String) {
        self.rollback_capacity_create(id);
        self.residency.capacity_blocked.insert(id, url);
    }

    pub(super) fn capacity_presentation(&self, id: ItemId) -> Option<zephium_ipc::TabAvailability> {
        if let Some(request) = self
            .residency
            .capacity_requests
            .iter()
            .find(|request| request.id == id)
        {
            Some(zephium_ipc::TabAvailability::WaitingForCapacity {
                url: request.url.clone(),
            })
        } else {
            self.residency
                .capacity_blocked
                .get(&id)
                .map(|url| zephium_ipc::TabAvailability::BlockedByCapacity { url: url.clone() })
        }
    }

    pub(super) fn clear_capacity_status(&mut self, id: ItemId) {
        self.residency.capacity_blocked.remove(&id);
        self.residency
            .capacity_requests
            .retain(|request| request.id != id);
        if let Some(queue) = &self.self_queue {
            queue.cancel_view_capacity(id);
        }
    }

    pub(super) fn on_view_capacity_retry(&mut self, _id: ItemId) {
        self.maintain_views();
    }

    fn retry_capacity_requests(&mut self) {
        let protected = self.discard_protected_leaves();
        let now = std::time::Instant::now();
        let mut pending = std::mem::take(&mut self.residency.capacity_requests);
        while let Some(request) = pending.pop_front() {
            if let Some(queue) = &self.self_queue {
                queue.cancel_view_capacity(request.id);
            }
            if self.profile_of_item(request.id) != Some(request.profile)
                || self.items.tab(request.id).is_none_or(TabState::has_view)
            {
                continue;
            }
            let same_scope = self.windows.focused().is_some_and(|window| {
                window.id == request.window
                    && window.space == request.space
                    && window.profile == request.profile
            });
            if !same_scope || !protected.contains(&request.id) || now >= request.deadline {
                self.residency
                    .capacity_blocked
                    .insert(request.id, request.url);
                self.project_tab(request.id);
            } else if self.items.view_ids().len() < LIVE_VIEW_ABSOLUTE_LIMIT {
                let effects = self.items.navigate(request.id, &request.url);
                self.apply(effects);
                let _ = self.relayout();
                self.project_tab(request.id);
            } else {
                if let Some(queue) = &self.self_queue {
                    queue.schedule_view_capacity(request.id, request.deadline);
                }
                self.residency.capacity_requests.push_back(request);
            }
        }
    }
    pub(super) fn on_view_creation_failed(&mut self, id: ItemId) {
        self.record_view_retirement(id);
        crate::diagnostic!("view-create: native failure or presentation retirement");
        self.zoom.pending.remove(&id);
        self.cancel_pending_presentation(id);
        self.clear_discard_after_retirement(id);
        self.items.view_creation_failed(id);
        let split_collapsed = self.collapse_failed_split_leaf(id);
        if split_collapsed {
            self.schedule_persist();
        }
        let _ = self.relayout();
        if split_collapsed {
            self.project_items();
        } else {
            self.project_tab(id);
        }
    }

    // Sleeping-tabs model: hidden views carry a low-memory hint, then a
    // bounded set of exact-generation/epoch probes may authorize full native
    // discard. Positive renderer results are only advisory until this actor
    // rechecks URL, loading, visibility, idle age, and the current budget.
    pub(super) fn maintain_views(&mut self) -> bool {
        self.crash
            .crashes
            .retain(|id, _| self.items.tab(*id).is_some());
        self.crash
            .presentations
            .retain(|id| self.items.tab(*id).is_some());
        self.zoom
            .pending
            .retain(|id, _| self.items.tab(*id).is_some_and(TabState::has_view));
        if self.windows.focused().is_none() {
            return false;
        }
        self.prune_work_pane();
        self.residency
            .capacity_blocked
            .retain(|id, _| self.items.tab(*id).is_some_and(|tab| !tab.has_view()));
        self.retry_capacity_requests();
        let shown: std::collections::HashSet<ItemId> = if self.window_visible {
            self.visible_tree()
                .map(|t| t.tabs().into_iter().collect())
                .unwrap_or_default()
        } else {
            std::collections::HashSet::new()
        };
        self.residency
            .last_focus
            .retain(|id, _| self.items.tab(*id).is_some());
        self.residency
            .resident_since
            .retain(|id, _| self.items.tab(*id).is_some_and(TabState::has_view));
        self.residency
            .inactive_since
            .retain(|id, _| self.items.tab(*id).is_some_and(TabState::has_view));
        self.residency
            .page_bytes
            .retain(|id, _| self.items.tab(*id).is_some_and(TabState::has_view));
        self.last_visits
            .retain(|id, _| self.items.tab(*id).is_some());
        self.residency.discard_protected_until.retain(|id, until| {
            self.items.tab(*id).is_some() && *until > std::time::Instant::now()
        });

        // A timeout command is ordinary coalescible work and may be displaced
        // during a full lifecycle burst. The maintenance pass independently
        // expires the same fixed set so overload cannot strand all probe slots.
        let expired: Vec<ItemId> = self
            .residency
            .discard_probes
            .iter()
            .filter_map(|(id, state)| match state {
                PendingDiscardProbe::Probing { deadline, .. }
                    if *deadline <= std::time::Instant::now() =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        for id in expired {
            self.residency.discard_probes.remove(&id);
            if let Some(queue) = &self.self_queue {
                queue.cancel_discard_probe(id);
            }
            self.protect_discard_candidate(id);
        }

        // Conceptual visible leaves remain protected even while the OS window
        // is minimized. Dormancy may hide them, but discard must not destroy a
        // split the user expects to reappear atomically.
        let protected = self.discard_protected_leaves();
        self.record_inactive_leaves(&protected, &shown);
        let live_count = self.items.view_ids().len();
        let warm = self.warm_views(&protected);
        self.sample_hidden_page_memory(&shown);
        let invalid_probes: Vec<ItemId> = self
            .residency
            .discard_probes
            .iter()
            .filter_map(|(id, state)| match state {
                PendingDiscardProbe::Probing { committed_url, .. } => {
                    let valid = self.wants_discard(*id, live_count, &warm)
                        && !protected.contains(id)
                        && self.items.tab(*id).is_some_and(|tab| {
                            tab.has_view()
                                && !tab.loading
                                && tab
                                    .url
                                    .as_ref()
                                    .is_some_and(|url| url.as_str() == committed_url)
                        });
                    (!valid).then_some(*id)
                }
                PendingDiscardProbe::Closing { .. } => self.items.tab(*id).is_none().then_some(*id),
            })
            .collect();
        for id in invalid_probes {
            if self.items.tab(id).is_none() {
                self.residency.discard_probes.remove(&id);
                if let Some(queue) = &self.self_queue {
                    queue.cancel_discard_probe(id);
                }
            } else {
                self.cancel_discard_probe(id);
            }
        }
        // Preference/visibility changes can still veto a native close before
        // its final retirement point. Preserve the exact terminal obligation;
        // a cancellation is not a physical-close acknowledgment.
        let cancelled_closes: Vec<(ItemId, DiscardProbeId)> = self
            .residency
            .discard_probes
            .iter()
            .filter_map(|(id, state)| match state {
                PendingDiscardProbe::Closing { probe, .. }
                    if !self.wants_discard(*id, live_count, &warm)
                        || protected.contains(id)
                        || self.items.tab(*id).is_some_and(|tab| tab.loading) =>
                {
                    Some((*id, *probe))
                }
                _ => None,
            })
            .collect();
        for (id, probe) in cancelled_closes {
            self.engine.cancel_discard(id, probe);
        }

        let mut dormant: Vec<ItemId> = self
            .items
            .view_ids()
            .into_iter()
            .filter(|id| {
                self.residency.sleeping
                    && !self.site_kept_awake(*id)
                    && !shown.contains(id)
                    && !self.residency.discard_probes.contains_key(id)
                    && self
                        .items
                        .tab(*id)
                        .is_some_and(|tab| !tab.loading && tab.url.is_some())
                    && self.idle_for(*id, self.residency.dormant_min)
            })
            .collect();
        dormant.sort();
        if dormant != self.residency.dormant_sent {
            self.residency.dormant_sent = dormant.clone();
            self.engine.set_dormant(dormant);
        }

        let urgent = self.discard_is_urgent(live_count);
        let capacity = !self.residency.capacity_requests.is_empty();
        let suspended_hidden = self.engine.suspends_hidden_views();
        let mut candidates: Vec<ItemId> = self
            .items
            .view_ids()
            .into_iter()
            .filter(|id| {
                !protected.contains(id)
                    && !self.residency.discard_probes.contains_key(id)
                    // A waiting foreground page may find a page safe now.
                    && (capacity
                        || self
                            .residency
                            .discard_protected_until
                            .get(id)
                            .is_none_or(|until| *until <= std::time::Instant::now()))
                    // A suspended page already costs no CPU. Waking it only to
                    // ask is worthwhile when memory is actually needed.
                    && (urgent
                        || !suspended_hidden
                        || !self.residency.dormant_sent.contains(id))
                    && self.items.tab(*id).is_some_and(|tab| {
                        tab.has_view() && !tab.loading && tab.url.is_some()
                    })
                    && self.wants_discard(*id, live_count, &warm)
            })
            .collect();
        if urgent {
            // Memory is needed now: free the most with the fewest reloads.
            candidates.sort_by_key(|id| {
                (
                    std::cmp::Reverse(self.residency.page_bytes.get(id).copied()),
                    self.last_view_activity(*id).copied(),
                    *id,
                )
            });
        } else {
            candidates.sort_by_key(|id| (self.last_view_activity(*id).copied(), *id));
        }

        let available =
            MAX_CONCURRENT_DISCARD_PROBES.saturating_sub(self.residency.discard_probes.len());
        for id in candidates.into_iter().take(available) {
            let Some(next) = self.residency.next_discard_probe.checked_add(1) else {
                // Reusing a process-local correlation id could accept a very
                // late callback. Saturation permanently disables new probes.
                break;
            };
            self.residency.next_discard_probe = next;
            let probe = DiscardProbeId(next);
            let deadline = std::time::Instant::now() + self.residency.discard_probe_timeout;
            let Some(committed_url) = self
                .items
                .tab(id)
                .and_then(|tab| tab.url.as_ref())
                .map(ToString::to_string)
            else {
                continue;
            };
            self.residency.discard_probes.insert(
                id,
                PendingDiscardProbe::Probing {
                    probe,
                    committed_url,
                    deadline,
                },
            );
            // A suspended WebView2 cannot reliably execute the DOM query.
            // Replace the desired dormant set first; main-thread FIFO then
            // guarantees resume is requested before the probe evaluation.
            if self.residency.dormant_sent.contains(&id) {
                self.residency.dormant_sent.retain(|dormant| *dormant != id);
                self.engine.set_dormant(self.residency.dormant_sent.clone());
            }
            if !self.engine.probe_discard_safety(id, probe) {
                self.residency.discard_probes.remove(&id);
                self.protect_discard_candidate(id);
                continue;
            }
            if let Some(queue) = &self.self_queue {
                queue.schedule_discard_probe(id, probe, deadline);
            }
        }
        false
    }

    pub(super) fn discard_protected_leaves(&self) -> std::collections::HashSet<ItemId> {
        let mut protected = std::collections::HashSet::new();
        let Some(window) = self.windows.focused() else {
            return protected;
        };
        if self.active_browser_page().is_some() {
            if let Some(id) = self.work_pane_tab() {
                if self.item_in_scope(id, window.profile, window.space) {
                    protected.insert(id);
                }
            }
        } else if let Some(active) = window.active {
            if self.item_in_scope(active, window.profile, window.space) {
                protected.insert(active);
                if let Some(tree) = &window.splits {
                    if tree.contains(active)
                        && self.pane_in_scope(tree, window.profile, window.space)
                    {
                        protected.extend(tree.tabs());
                    }
                }
            }
        }
        protected
    }

    fn candidate_is_still_discardable(&self, id: ItemId, committed_url: &str) -> bool {
        let live_count = self.items.view_ids().len();
        let protected = self.discard_protected_leaves();
        !protected.contains(&id)
            && self.items.tab(id).is_some_and(|tab| {
                tab.has_view()
                    && !tab.loading
                    && tab
                        .url
                        .as_ref()
                        .is_some_and(|url| url.as_str() == committed_url)
            })
            && self.wants_discard(id, live_count, &self.warm_views(&protected))
    }

    /// Memory is needed now, so idle grace and the warm set no longer apply.
    fn discard_is_urgent(&self, live_count: usize) -> bool {
        self.residency.memory_pressure == MemoryPressure::Critical
            || (self.residency.sleeping
                && (!self.residency.capacity_requests.is_empty()
                    || live_count > self.residency.live_view_pressure_limit))
    }

    /// Policy intent only; the engine probe still vetoes unsafe pages. The
    /// user's choices are honored: kept-awake sites never sleep, and with
    /// sleeping off only critical OS pressure, where the alternative is the
    /// OS killing renderers outright, may still discard.
    fn wants_discard(&self, id: ItemId, live_count: usize, warm: &Warmth) -> bool {
        if self.site_kept_awake(id) {
            return false;
        }
        if self.discard_is_urgent(live_count) {
            return true;
        }
        let heavy = self
            .residency
            .page_bytes
            .get(&id)
            .is_some_and(|bytes| *bytes >= self.residency.heavy_page.0);
        self.residency.sleeping
            && ((!warm.warm.contains(&id) && self.idle_for(id, self.residency.idle_grace()))
                || (heavy
                    && warm.instant != Some(id)
                    && self.idle_for(id, self.residency.heavy_grace())))
    }

    /// The most recently active hidden pages, kept resident for fast switching.
    fn warm_views(&self, protected: &std::collections::HashSet<ItemId>) -> Warmth {
        let mut hidden: Vec<ItemId> = self
            .items
            .view_ids()
            .into_iter()
            .filter(|id| !protected.contains(id))
            .collect();
        hidden.sort_by_key(|id| std::cmp::Reverse((self.last_view_activity(*id).copied(), *id)));
        Warmth {
            instant: hidden.first().copied(),
            warm: hidden
                .into_iter()
                .take(self.residency.warm_view_limit)
                .collect(),
        }
    }

    /// Heavy pages are found by their renderer's footprint; asked for only
    /// while something can act on the answer, at most every half minute.
    fn sample_hidden_page_memory(&mut self, shown: &std::collections::HashSet<ItemId>) {
        if !self.residency.sleeping && self.residency.memory_pressure == MemoryPressure::Normal {
            return;
        }
        let now = std::time::Instant::now();
        if self
            .residency
            .memory_sampled
            .is_some_and(|sampled| now.duration_since(sampled) < MEMORY_SAMPLE_INTERVAL)
        {
            return;
        }
        let hidden: Vec<ItemId> = self
            .items
            .view_ids()
            .into_iter()
            .filter(|id| !shown.contains(id))
            .collect();
        if !hidden.is_empty() {
            self.residency.memory_sampled = Some(now);
            self.engine.sample_page_memory(hidden);
        }
    }

    fn protect_discard_candidate(&mut self, id: ItemId) {
        self.residency.discard_protected_until.insert(
            id,
            std::time::Instant::now() + self.residency.discard_protected_retry,
        );
    }

    pub(super) fn cancel_discard_probe(&mut self, id: ItemId) {
        match self.residency.discard_probes.get_mut(&id) {
            Some(PendingDiscardProbe::Closing {
                probe, recreate, ..
            }) => {
                // Cancel before native retirement if possible. Its exact
                // Refused/Discarded result still owns the terminal obligation.
                *recreate = true;
                self.engine.cancel_discard(id, *probe);
            }
            Some(PendingDiscardProbe::Probing { .. }) => {
                self.residency.discard_probes.remove(&id);
                if let Some(queue) = &self.self_queue {
                    queue.cancel_discard_probe(id);
                }
            }
            None => {}
        }
    }

    pub(super) fn recreate_after_inflight_discard(&mut self, id: ItemId) -> bool {
        if let Some(PendingDiscardProbe::Closing {
            probe, recreate, ..
        }) = self.residency.discard_probes.get_mut(&id)
        {
            *recreate = true;
            self.engine.cancel_discard(id, *probe);
            true
        } else {
            false
        }
    }

    pub(super) fn on_discard_probe_timeout(&mut self, id: ItemId, probe: DiscardProbeId) {
        let exact = matches!(
            self.residency.discard_probes.get(&id),
            Some(PendingDiscardProbe::Probing { probe: pending, .. }) if *pending == probe
        );
        if exact {
            self.residency.discard_probes.remove(&id);
            self.protect_discard_candidate(id);
            self.maintain_views();
        }
    }

    pub(super) fn on_discard_safety(
        &mut self,
        id: ItemId,
        probe: DiscardProbeId,
        can_discard: bool,
    ) {
        let Some(PendingDiscardProbe::Probing {
            probe: pending,
            committed_url,
            deadline,
        }) = self.residency.discard_probes.get(&id).cloned()
        else {
            return;
        };
        if pending != probe {
            return;
        }
        if let Some(queue) = &self.self_queue {
            queue.cancel_discard_probe(id);
        }
        if !can_discard
            || deadline <= std::time::Instant::now()
            || !self.candidate_is_still_discardable(id, &committed_url)
        {
            self.residency.discard_probes.remove(&id);
            if !can_discard || deadline <= std::time::Instant::now() {
                self.protect_discard_candidate(id);
            }
            self.maintain_views();
            return;
        }

        self.residency.discard_probes.insert(
            id,
            PendingDiscardProbe::Closing {
                probe,
                recreate: false,
                deferred_navigation: None,
                reload_on_refusal: false,
            },
        );
        // Native repeats safety/restoration checks before its physical
        // retirement point. A refused admission leaves this view alive.
        if !self.engine.discard_view(id, probe) {
            if let Some(profile) = self.profile_of_item(id) {
                self.on_view_discard_refused(id, profile, probe);
            } else {
                self.residency.discard_probes.remove(&id);
                self.protect_discard_candidate(id);
            }
        }
    }

    pub(super) fn on_view_discard_refused(
        &mut self,
        id: ItemId,
        profile: ProfileId,
        probe: DiscardProbeId,
    ) {
        let Some(PendingDiscardProbe::Closing {
            probe: pending,
            deferred_navigation,
            recreate,
            reload_on_refusal,
        }) = self.residency.discard_probes.get(&id).cloned()
        else {
            return;
        };
        if pending != probe || self.profile_of_item(id) != Some(profile) {
            return;
        }
        self.residency.discard_probes.remove(&id);
        self.protect_discard_candidate(id);
        let effects = if let Some(input) = deferred_navigation {
            self.items.navigate(id, &input)
        } else if recreate || self.discard_protected_leaves().contains(&id) {
            self.items.ensure_view(id)
        } else {
            Vec::new()
        };
        self.apply(effects);
        if reload_on_refusal && self.items.tab(id).is_some_and(TabState::has_view) {
            let _ = self.engine.reload(id);
        }
        let _ = self.relayout();
        self.maintain_views();
        self.project_tab(id);
    }

    pub(super) fn on_view_discarded(
        &mut self,
        id: ItemId,
        profile: ProfileId,
        probe: DiscardProbeId,
    ) {
        let Some(PendingDiscardProbe::Closing {
            probe: pending,
            recreate,
            deferred_navigation,
            ..
        }) = self.residency.discard_probes.get(&id).cloned()
        else {
            return;
        };
        if pending != probe || self.profile_of_item(id) != Some(profile) {
            return;
        }
        self.zoom.pending.remove(&id);
        self.cancel_pending_presentation(id);
        self.residency.discard_probes.remove(&id);
        self.record_view_retirement(id);
        if !self.items.mark_view_discarded(id) {
            return;
        }

        let mut effects = if let Some(input) = deferred_navigation {
            self.items.navigate(id, &input)
        } else if recreate || self.discard_protected_leaves().contains(&id) {
            self.items.ensure_view(id)
        } else {
            Vec::new()
        };
        // If a split/focus change happened between native acknowledgement and
        // this actor turn, the final visibility check wins.
        if effects.is_empty() && self.discard_protected_leaves().contains(&id) {
            effects.extend(self.items.ensure_view(id));
        }
        self.apply(effects);
        let _ = self.relayout();
        self.maintain_views();
        self.project_tab(id);
    }

    fn idle_for(&self, id: ItemId, min: std::time::Duration) -> bool {
        // A fresh background page has never been focused. Its actual view
        // residency starts the grace, without making it recently selected.
        // Recreating a view also starts a fresh grace for that document.
        self.last_view_activity(id)
            .is_some_and(|time| time.elapsed() >= min)
    }

    fn last_view_activity(&self, id: ItemId) -> Option<&std::time::Instant> {
        self.residency
            .last_focus
            .get(&id)
            .into_iter()
            .chain(self.residency.resident_since.get(&id))
            .chain(self.residency.inactive_since.get(&id))
            .max()
    }

    pub(super) fn record_view_creation(&mut self, id: ItemId) {
        self.residency.inactive_since.remove(&id);
        self.residency
            .resident_since
            .insert(id, std::time::Instant::now());
    }

    pub(super) fn record_view_retirement(&mut self, id: ItemId) {
        self.residency.resident_since.remove(&id);
        self.residency.inactive_since.remove(&id);
    }

    fn record_inactive_leaves(
        &mut self,
        protected: &std::collections::HashSet<ItemId>,
        shown: &std::collections::HashSet<ItemId>,
    ) {
        let now = std::time::Instant::now();
        // A visible document can be read for hours. Its background grace starts
        // when it leaves a split/Work/foreground surface, not at activation.
        // Minimization starts dormancy age while keeping those leaves protected
        // from full discard; later conceptual focus changes also start a grace.
        for id in self
            .residency
            .last_protected
            .difference(protected)
            .chain(self.residency.last_shown.difference(shown))
        {
            if self.items.tab(*id).is_some_and(TabState::has_view) {
                self.residency.inactive_since.insert(*id, now);
            }
        }
        self.residency.last_protected = protected.clone();
        self.residency.last_shown = shown.clone();
    }

    fn retire_crashed_view(&mut self, id: ItemId) {
        // Renderer death is physical retirement proof and supersedes an
        // unfinished discard. Its old terminal callback may never arrive.
        self.clear_discard_after_retirement(id);
        self.record_view_retirement(id);
        self.zoom.pending.remove(&id);
        let title = self.items.tab(id).map(|tab| tab.title.clone());
        self.items.view_creation_failed(id);
        if let Some(title) = title {
            // `view_creation_failed` owns the generic create-failure label.
            // A renderer crash is different: its label is transient chrome
            // state and must not replace the last committed document title.
            self.items.set_title(id, title);
        }
        self.crash.presentations.insert(id);
    }

    fn clear_discard_after_retirement(&mut self, id: ItemId) {
        self.residency.discard_probes.remove(&id);
        if let Some(queue) = &self.self_queue {
            queue.cancel_discard_probe(id);
        }
    }

    // One automatic relaunch per crash burst: a second death inside the
    // window means the page kills its web process deterministically, and a
    // reload loop would peg the machine.
    pub(super) fn on_crashed(&mut self, id: ItemId) {
        const RETRY_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);
        if self.items.tab(id).is_none() {
            return;
        }
        self.cancel_pending_presentation(id);
        self.cancel_discard_probe(id);
        // `Crashed` is emitted only after the engine has physically removed
        // and revoked the exact native-view generation. Sending a second,
        // id-only close here could be reordered behind recovery and destroy
        // the replacement generation.
        self.retire_crashed_view(id);
        self.items.set_loading(id, false);
        let recent = self
            .crash
            .crashes
            .insert(id, std::time::Instant::now())
            .is_some_and(|t| t.elapsed() < RETRY_WINDOW);
        let active = self.windows.focused().and_then(|window| window.active);
        let visible = active == Some(id) || self.work_pane_shows(id);
        let effects = if !recent && visible {
            self.items.ensure_view(id)
        } else {
            Vec::new()
        };
        self.apply(effects);
        if visible {
            let _ = self.relayout();
            self.maintain_views();
        }
        // Crash status is runtime truth, not useful session state. Avoid a
        // full O(items) snapshot/relayout for every hidden WebKit view when a
        // shared renderer process reports a burst of per-view terminations.
        self.project_tab(id);
    }

    pub(super) fn on_profile_process_exit(&mut self, profile: ProfileId, ids: Vec<ItemId>) {
        const RETRY_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);
        let visible_order: Vec<ItemId> = self
            .visible_tree()
            .map(|tree| tree.tabs())
            .unwrap_or_default();
        let visible: std::collections::HashSet<ItemId> = visible_order.iter().copied().collect();
        let mut suppressed_visible = std::collections::HashSet::new();
        let mut effects = Vec::new();
        for id in ids {
            if self.profile_of_item(id) != Some(profile) {
                continue;
            }
            self.cancel_pending_presentation(id);
            self.cancel_discard_probe(id);
            let repeated = self
                .crash
                .crashes
                .insert(id, std::time::Instant::now())
                .is_some_and(|time| time.elapsed() < RETRY_WINDOW);
            self.retire_crashed_view(id);
            // A browser-process loss can report hundreds of tabs at once.
            // Recreate every currently visible split leaf, because native
            // layout admission requires a live token for every leaf. Hidden
            // tabs still recover lazily, avoiding a process/controller storm.
            if visible.contains(&id) && !repeated {
                effects.extend(self.items.ensure_view(id));
            } else if visible.contains(&id) {
                suppressed_visible.insert(id);
            }
        }
        if self
            .windows
            .focused()
            .and_then(|window| window.active)
            .is_some_and(|active| suppressed_visible.contains(&active))
        {
            // A repeatedly crashing active leaf has no native token. Focus a
            // successfully recreated sibling so layout collapses to that live
            // leaf until the protected tab is explicitly activated again.
            if let Some(replacement) = visible_order.into_iter().find(|id| {
                !suppressed_visible.contains(id)
                    && self.items.tab(*id).is_some_and(TabState::has_view)
            }) {
                if let Some(window) = self.windows.focused_mut() {
                    window.active = Some(replacement);
                }
                self.items.set_lifecycle(replacement, Lifecycle::Active);
                self.note_activation(replacement);
            }
        }
        self.commit(effects);
    }

    /// A tab was activated: it goes on top of `residency.recent` and its
    /// idle clock restarts. The only push onto `recent`; sites that show a
    /// tab without activating it (split partners, the work pane, a hidden
    /// window) call `touch` alone.
    pub(super) fn note_activation(&mut self, id: ItemId) {
        if self.items.tab(id).is_some() {
            self.residency.recent.push(id);
        }
        self.touch(id);
    }

    /// Removes an item and its descendants from the tree. Every shell
    /// removal goes through here or `remove_profile_items`, so
    /// `residency.recent` never names a closed tab.
    pub(super) fn remove_item(&mut self, id: ItemId) -> Vec<Effect> {
        let effects = self.items.remove(id);
        self.residency.recent.retain_live(&self.items);
        effects
    }

    pub(super) fn remove_profile_items(
        &mut self,
        spaces: &zephium_core::spaces::RemovedProfileSpaces,
    ) -> Vec<Effect> {
        let effects = self.items.remove_for_profile(spaces);
        self.residency.recent.retain_live(&self.items);
        effects
    }

    pub(super) fn touch(&mut self, id: ItemId) {
        self.cancel_discard_probe(id);
        self.residency
            .last_focus
            .insert(id, std::time::Instant::now());
    }
}
