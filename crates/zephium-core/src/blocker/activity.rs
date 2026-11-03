use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
static SHARED: LazyLock<Arc<PageActivity>> = LazyLock::new(|| Arc::new(PageActivity::new()));

/// Main-frame loads in flight across every page, background tabs included, so
/// list-update work can wait until browsing is quiet.
#[derive(Debug)]
pub struct PageActivity {
    loading: AtomicUsize,
    last_change_ms: AtomicU64,
}

/// One main-frame load in flight; dropping it ends the load, so a page closed
/// mid-load never leaves the count behind.
#[derive(Debug)]
pub struct ActivePageLoad(Arc<PageActivity>);

impl PageActivity {
    /// An activity record that has seen no loads.
    pub fn new() -> Self {
        LazyLock::force(&EPOCH);
        Self {
            loading: AtomicUsize::new(0),
            last_change_ms: AtomicU64::new(0),
        }
    }

    /// The one record the engine writes and the blocker reads.
    pub fn shared() -> Arc<Self> {
        SHARED.clone()
    }

    /// Marks a main-frame load as started until the returned guard drops.
    pub fn begin_load(self: &Arc<Self>) -> ActivePageLoad {
        self.loading.fetch_add(1, Ordering::AcqRel);
        self.touch();
        ActivePageLoad(self.clone())
    }

    /// How long no main-frame load has been in flight, or `None` while one is.
    /// Before any load it has been quiet for as long as can be said.
    pub fn quiet_for(&self, now: Instant) -> Option<Duration> {
        if self.loading.load(Ordering::Acquire) > 0 {
            return None;
        }
        // Stored one past the elapsed milliseconds, so zero means no load yet.
        match self.last_change_ms.load(Ordering::Acquire) {
            0 => Some(Duration::MAX),
            stored => {
                Some(now.saturating_duration_since(*EPOCH + Duration::from_millis(stored - 1)))
            }
        }
    }

    fn touch(&self) {
        let elapsed = u64::try_from(EPOCH.elapsed().as_millis()).unwrap_or(u64::MAX - 1);
        self.last_change_ms.fetch_max(elapsed + 1, Ordering::AcqRel);
    }
}

impl Default for PageActivity {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ActivePageLoad {
    fn drop(&mut self) {
        // Stamped first, so the count reaching zero never pairs with the
        // load's start time.
        self.0.touch();
        self.0.loading.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_starts_when_the_last_load_ends_or_its_page_goes_away() {
        let activity = Arc::new(PageActivity::new());
        assert_eq!(activity.quiet_for(Instant::now()), Some(Duration::MAX));
        let first = activity.begin_load();
        let second = activity.begin_load();
        assert_eq!(activity.quiet_for(Instant::now()), None);
        drop(first);
        assert_eq!(activity.quiet_for(Instant::now()), None);
        drop(second);
        let quiet = activity.quiet_for(Instant::now() + Duration::from_secs(3));
        assert!(quiet.is_some_and(|q| q >= Duration::from_secs(2)));
        assert!(activity
            .quiet_for(Instant::now())
            .is_some_and(|q| q < Duration::from_secs(1)));
    }
}
