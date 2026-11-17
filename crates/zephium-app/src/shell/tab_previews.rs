use super::*;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const ENTRIES: usize = 16;
const CACHE_BYTES: usize = 1024 * 1024 - 4096;
const ENTRY_BYTES: usize = 80 * 1024;
const FRESH: Duration = Duration::from_secs(30);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_NATIVE_CAPTURES: usize = ENTRIES;
/// WebView2 captures only a view on screen, and a switch hides the tab it
/// leaves before its capture runs. There a tab is captured in front: when it
/// finishes loading, once it is switched to, and when the switcher opens.
pub(super) const CAPTURES_ONLY_ON_SCREEN: bool = cfg!(target_os = "windows");

struct Preview {
    tab: ItemId,
    url: String,
    revision: u64,
    captured: Instant,
    bytes: Box<[u8]>,
}

impl Preview {
    fn retained_bytes(&self) -> usize {
        self.bytes.len() + self.url.capacity() + std::mem::size_of::<Self>()
    }
}
#[derive(Default)]
struct Cache(VecDeque<Preview>);
impl Cache {
    fn fresh(&self, tab: ItemId, url: &str, now: Instant, only_missing: bool) -> bool {
        self.0.iter().any(|p| {
            p.tab == tab
                && p.url == url
                && (only_missing || now.saturating_duration_since(p.captured) < FRESH)
        })
    }
    fn insert(&mut self, preview: Preview) {
        if preview.bytes.is_empty()
            || preview.bytes.len() > ENTRY_BYTES
            || preview.retained_bytes() > CACHE_BYTES
        {
            return;
        }
        self.0.retain(|held| held.tab != preview.tab);
        while self.0.len() >= ENTRIES
            || self.0.iter().map(Preview::retained_bytes).sum::<usize>() + preview.retained_bytes()
                > CACHE_BYTES
        {
            self.0.pop_front();
        }
        self.0.push_back(preview);
    }
    fn get(&mut self, tab: ItemId, revision: u64) -> Option<Vec<u8>> {
        let index = self
            .0
            .iter()
            .position(|p| p.tab == tab && p.revision == revision)?;
        let preview = self.0.remove(index)?;
        let bytes = preview.bytes.to_vec();
        self.0.push_back(preview);
        Some(bytes)
    }
}

struct PendingPreview {
    request: u64,
    window: WindowId,
    tab: ItemId,
    url: String,
    deadline: Instant,
    completed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
pub(super) struct TabPreviews {
    windows: HashMap<WindowId, Cache>,
    queue: VecDeque<(WindowId, ItemId)>,
    pending: Option<PendingPreview>,
    // A timed-out callback still owns native work. Keep its reservation until
    // the callback drains, and never retry that tab around an unknown view.
    abandoned: HashMap<u64, PendingPreview>,
    revision: u64,
}

impl TabPreviews {
    pub(super) fn forget(&mut self, tab: ItemId) {
        for cache in self.windows.values_mut() {
            cache.0.retain(|p| p.tab != tab);
        }
        self.queue.retain(|(_, id)| *id != tab);
        self.invalidate_capture(tab);
    }
    pub(super) fn invalidate_capture(&mut self, tab: ItemId) {
        for capture in self.pending.iter_mut().chain(self.abandoned.values_mut()) {
            if capture.tab == tab {
                capture.url.clear();
            }
        }
    }

    fn may_capture(&self, tab: ItemId) -> bool {
        self.pending.is_none()
            && self.abandoned.len() < MAX_NATIVE_CAPTURES
            && !self.abandoned.values().any(|capture| capture.tab == tab)
    }

    fn expire(&mut self, now: Instant) -> bool {
        let before = self.abandoned.len();
        self.abandoned
            .retain(|_, capture| !capture.completed.load(std::sync::atomic::Ordering::Acquire));
        let released = before != self.abandoned.len();
        if self
            .pending
            .as_ref()
            .is_none_or(|capture| capture.deadline > now)
        {
            return released;
        }
        let Some(capture) = self.pending.take() else {
            return false;
        };
        self.abandoned.insert(capture.request, capture);
        true
    }

    fn settle(&mut self, request: u64) -> Option<PendingPreview> {
        if self
            .pending
            .as_ref()
            .is_some_and(|capture| capture.request == request)
        {
            self.pending.take()
        } else {
            self.abandoned.remove(&request);
            None
        }
    }

    pub(super) fn retain_tabs(&mut self, keep: impl Fn(ItemId) -> bool) {
        for cache in self.windows.values_mut() {
            cache.0.retain(|p| keep(p.tab));
        }
        self.queue.retain(|(_, id)| keep(*id));
        for capture in self.pending.iter_mut().chain(self.abandoned.values_mut()) {
            if !keep(capture.tab) {
                capture.url.clear();
            }
        }
    }
    pub(super) fn retain_windows(&mut self, keep: impl Fn(WindowId) -> bool) {
        self.windows.retain(|id, _| keep(*id));
        self.queue.retain(|(id, _)| keep(*id));
        for capture in self.pending.iter_mut().chain(self.abandoned.values_mut()) {
            if !keep(capture.window) {
                capture.url.clear();
            }
        }
    }
    pub(super) fn url(&self, window: WindowId, tab: ItemId, url: Option<&str>) -> Option<String> {
        let preview = self
            .windows
            .get(&window)?
            .0
            .iter()
            .find(|p| p.tab == tab && Some(p.url.as_str()) == url)?;
        let prefix = if cfg!(target_os = "windows") {
            "http://zephium-preview.localhost"
        } else {
            "zephium-preview://localhost"
        };
        Some(format!("{prefix}/{window}/{tab}?v={}", preview.revision))
    }
}

impl Shell {
    pub(super) fn queue_tab_preview(&mut self, window: WindowId, tab: ItemId, only_missing: bool) {
        let Some(state) = self.items.tab(tab).filter(|tab| tab.has_view()) else {
            return;
        };
        let Some(url) = state.url.as_ref() else {
            return;
        };
        if self
            .tab_previews
            .windows
            .get(&window)
            .is_some_and(|cache| cache.fresh(tab, url.as_str(), Instant::now(), only_missing))
            || self.tab_previews.queue.contains(&(window, tab))
            || self.tab_previews.pending.as_ref().is_some_and(|capture| {
                capture.window == window && capture.tab == tab && capture.url == url.as_str()
            })
            || self
                .tab_previews
                .abandoned
                .values()
                .any(|capture| capture.tab == tab)
        {
            return;
        }
        if self.tab_previews.queue.len() == ENTRIES {
            self.tab_previews.queue.pop_front();
        }
        self.tab_previews.queue.push_back((window, tab));
        if self.tab_previews.pending.is_none() {
            if let Some(queue) = &self.self_queue {
                let _ = queue.try_push(Command::CaptureTabPreview);
            }
        }
    }

    /// A tab that finished loading in front of the focused window is on
    /// screen, the one moment a view that only captures there can be taken.
    pub(super) fn capture_loaded_tab_preview(&mut self, tab: ItemId) {
        if !CAPTURES_ONLY_ON_SCREEN {
            return;
        }
        let Some(window) = self
            .windows
            .focused()
            .filter(|window| window.active == Some(tab))
            .map(|window| window.id)
        else {
            return;
        };
        self.queue_tab_preview(window, tab, false);
    }

    pub(super) fn capture_tab_preview(&mut self) {
        self.tab_previews.expire(Instant::now());
        if self.tab_previews.pending.is_some()
            || self.tab_previews.abandoned.len() >= MAX_NATIVE_CAPTURES
        {
            return;
        }
        let Some(queue) = self.self_queue.clone() else {
            return;
        };
        while let Some((window, tab)) = self.tab_previews.queue.pop_front() {
            if self.windows.get(window).is_none() || !self.tab_previews.may_capture(tab) {
                continue;
            }
            let Some(url) = self
                .items
                .tab(tab)
                .filter(|t| t.has_view())
                .and_then(|t| t.url.as_ref())
                .map(ToString::to_string)
            else {
                continue;
            };
            let Some(request) = self.tab_previews.revision.checked_add(1) else {
                return;
            };
            self.tab_previews.revision = request;
            let deadline = Instant::now() + CAPTURE_TIMEOUT;
            let completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            self.tab_previews.pending = Some(PendingPreview {
                completed: completed.clone(),
                request,
                window,
                tab,
                url: url.clone(),
                deadline,
            });
            queue.schedule_dormancy(deadline);
            self.engine.capture_tab_preview(
                tab,
                url,
                Box::new(move |bytes| {
                    let _ = queue.try_push(Command::TabPreviewReady { request, bytes });
                    // Even a coalesced/dropped actor reply releases the native
                    // reservation once maintenance sees this receipt.
                    completed.store(true, std::sync::atomic::Ordering::Release);
                }),
            );
            return;
        }
    }

    pub(super) fn tab_preview_ready(&mut self, request: u64, bytes: Option<Vec<u8>>) {
        self.tab_previews.expire(Instant::now());
        let Some(PendingPreview {
            window, tab, url, ..
        }) = self.tab_previews.settle(request)
        else {
            self.capture_tab_preview();
            return;
        };
        if self.windows.get(window).is_some()
            && self.items.tab(tab).is_some_and(|t| {
                t.has_view() && t.url.as_ref().map(|u| u.as_str()) == Some(url.as_str())
            })
        {
            if let Some(bytes) = bytes {
                self.tab_previews
                    .windows
                    .entry(window)
                    .or_default()
                    .insert(Preview {
                        tab,
                        url,
                        revision: request,
                        captured: Instant::now(),
                        bytes: bytes.into_boxed_slice(),
                    });
                self.project_tab_switcher();
            }
        }
        self.capture_tab_preview();
    }

    pub(super) fn expire_tab_preview(&mut self, now: Instant) {
        if self.tab_previews.expire(now) {
            self.capture_tab_preview();
        }
        // The shared earliest maintenance wake clears its deadline. An early
        // wake for another owner must not lose this still-pending capture.
        if let (Some(queue), Some(capture)) = (&self.self_queue, &self.tab_previews.pending) {
            queue.schedule_dormancy(capture.deadline);
        }
    }

    pub(super) fn read_tab_preview(
        &mut self,
        window: WindowId,
        tab: ItemId,
        revision: u64,
    ) -> Option<Vec<u8>> {
        let focused = self.windows.focused()?;
        if focused.id != window || !self.item_in_focused_scope(tab) {
            return None;
        }
        let url = self.items.tab(tab)?.url.as_ref()?.as_str();
        let cache = self.tab_previews.windows.get_mut(&window)?;
        if !cache
            .0
            .iter()
            .any(|p| p.tab == tab && p.url == url && p.revision == revision)
        {
            return None;
        }
        cache.get(tab, revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn preview(id: u128, size: usize) -> Preview {
        Preview {
            tab: id.into(),
            url: "https://example.test/".into(),
            revision: id as u64,
            captured: Instant::now(),
            bytes: vec![1; size].into_boxed_slice(),
        }
    }
    fn pending(request: u64, tab: u128, now: Instant) -> PendingPreview {
        PendingPreview {
            request,
            window: 1,
            tab: tab.into(),
            url: "https://example.test/".into(),
            deadline: now + CAPTURE_TIMEOUT,
            completed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    #[test]
    fn timeout_frees_scheduling_but_reserves_hung_tab_and_late_callback_cannot_clear_successor() {
        let now = Instant::now();
        let mut previews = TabPreviews {
            pending: Some(pending(1, 1, now)),
            ..Default::default()
        };
        assert!(!previews.expire(now + CAPTURE_TIMEOUT - Duration::from_millis(1)));
        assert!(previews.expire(now + CAPTURE_TIMEOUT));
        assert!(!previews.may_capture(1.into()));
        assert!(previews.may_capture(2.into()));
        previews.pending = Some(pending(2, 2, now + CAPTURE_TIMEOUT));
        assert!(previews.settle(1).is_none());
        assert_eq!(previews.pending.as_ref().unwrap().request, 2);
        assert_eq!(previews.settle(2).unwrap().tab, 2.into());
        assert!(previews.pending.is_none() && previews.abandoned.is_empty());
    }

    #[test]
    fn repeated_timeouts_and_retirement_cannot_grow_native_capture_reservations() {
        let now = Instant::now();
        let mut previews = TabPreviews::default();
        for request in 1..=MAX_NATIVE_CAPTURES as u64 {
            assert!(previews.may_capture((request as u128).into()));
            previews.pending = Some(pending(request, request as u128, now));
            assert!(previews.expire(now + CAPTURE_TIMEOUT));
            previews.forget((request as u128).into());
            assert_eq!(previews.abandoned.len(), request as usize);
        }
        for _ in 0..100 {
            assert!(!previews.may_capture(999.into()));
            assert!(!previews.expire(now + CAPTURE_TIMEOUT));
        }
        // Native completion receipt releases ownership even if the actor's
        // bytes-bearing completion message was coalesced away.
        previews.abandoned[&1]
            .completed
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(previews.expire(now + CAPTURE_TIMEOUT));
        assert!(previews.may_capture(999.into()));
        assert_eq!(previews.abandoned.len(), MAX_NATIVE_CAPTURES - 1);
    }

    #[test]
    fn byte_and_entry_limits_and_lru_access_are_enforced() {
        let mut cache = Cache::default();
        for id in 1..=12 {
            cache.insert(preview(id, ENTRY_BYTES));
        }
        assert_eq!(cache.0.len(), 12);
        assert_eq!(cache.0.iter().map(|p| p.bytes.len()).sum::<usize>(), 983040);
        let retained = cache.0.iter().map(Preview::retained_bytes).sum::<usize>();
        assert!(retained + 4096 <= 1024 * 1024);
        println!("12 previews: encoded=983040 retained={retained} overhead_reserve=4096");
        assert!(cache.get(1.into(), 1).is_some());
        cache.insert(preview(13, ENTRY_BYTES));
        assert!(cache.get(2.into(), 2).is_none());
        assert!(cache.get(1.into(), 2).is_none());
        assert!(cache.get(1.into(), 1).is_some());
        cache.insert(preview(99, ENTRY_BYTES + 1));
        assert!(cache.get(99.into(), 99).is_none());
        let mut cache = Cache::default();
        for id in 1..=20 {
            cache.insert(preview(id, 100));
        }
        assert_eq!(cache.0.len(), ENTRIES);
        assert!(cache.get(4.into(), 4).is_none());
    }
    #[test]
    fn freshness_is_url_bound_and_expires_at_thirty_seconds() {
        let mut cache = Cache::default();
        let preview = preview(1, 100);
        let captured = preview.captured;
        cache.insert(preview);
        assert!(cache.fresh(
            1.into(),
            "https://example.test/",
            captured + Duration::from_secs(29),
            false
        ));
        assert!(!cache.fresh(1.into(), "https://example.test/", captured + FRESH, false));
        assert!(!cache.fresh(1.into(), "https://other.test/", captured, false));
        assert!(cache.fresh(1.into(), "https://example.test/", captured + FRESH, true));
    }
    #[test]
    fn close_and_window_retirement_remove_private_bytes() {
        let mut previews = TabPreviews::default();
        previews
            .windows
            .entry(1)
            .or_default()
            .insert(preview(1, 100));
        previews.forget(1.into());
        assert!(previews.windows[&1].0.is_empty());
        previews.retain_windows(|_| false);
        assert!(previews.windows.is_empty());
    }
}
