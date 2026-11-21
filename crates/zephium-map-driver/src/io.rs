use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use url::Url;
use zephium_map::session::{Expect, Failure, FetchResult};

use crate::memo::Memo;

pub type Reading<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub struct ResultsPage {
    /// The serialised live DOM of the committed results page.
    pub html: String,
    pub url: Url,
    pub truncated: bool,
}

pub trait ResultsSource: Send + Sync {
    fn load(&self, url: Url) -> Reading<Result<ResultsPage, Failure>>;
}

pub struct FetchRequest {
    pub url: Url,
    pub expect: Expect,
    pub max_bytes: u32,
    pub timeout: Duration,
}

pub trait Fetcher: Send + Sync {
    fn fetch(&self, request: FetchRequest) -> Reading<Result<FetchResult, Failure>>;
}

pub trait Clock: Send + Sync {
    /// Monotonic time since an arbitrary origin.
    fn now(&self) -> Duration;
    /// Wall-clock seconds, used only to judge a memo's age.
    fn unix(&self) -> u64;
}

pub struct SystemClock {
    origin: tokio::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
    }
}

#[derive(Clone)]
pub struct Io {
    pub results: Arc<dyn ResultsSource>,
    pub fetch: Arc<dyn Fetcher>,
    pub memo: Arc<dyn Memo>,
    pub clock: Arc<dyn Clock>,
}
