//! Frame documents the frame check refused, held before their requests leave.
//! WebView2 starts a frame's request without waiting for
//! FrameNavigationStarting to answer, so a cancel there only stops the frame
//! from showing what the server already sent. Each refusal therefore adds a
//! request filter for that one address, and the request is answered at
//! WebResourceRequested, which the browser raises after the frame check and
//! before the request is sent. No other document is filtered, so a main
//! document never waits on the host.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2_2, ICoreWebView2_22, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
};
use webview2_com::{NavigationStartingEventHandler, WebResourceRequestedEventHandler};
use windows_core::{Interface, HSTRING, PWSTR};

use super::{take_pwstr_bounded, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT};

/// Refusals a view holds at once. A refusal past this is let through as
/// before rather than displacing one still waiting for its request.
const CAPACITY: usize = 64;
/// A request follows its frame check within milliseconds; a refusal older
/// than this belongs to a navigation that never sent one.
const LIFETIME: Duration = Duration::from_secs(5);

/// The view's document request filters, one added per refusal. WebView2
/// counts a filter added twice and needs it removed twice.
pub(crate) trait DocumentFilters {
    fn add(&self, url: &str) -> windows_core::Result<()>;
    fn remove(&self, url: &str) -> windows_core::Result<()>;
}

impl DocumentFilters for ICoreWebView2_22 {
    fn add(&self, url: &str) -> windows_core::Result<()> {
        // SAFETY: a live interface on the view's thread; the string outlives
        // the call.
        unsafe {
            self.AddWebResourceRequestedFilterWithRequestSourceKinds(
                &HSTRING::from(url),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
            )
        }
    }

    fn remove(&self, url: &str) -> windows_core::Result<()> {
        // SAFETY: as in `add`, for a filter `add` installed.
        unsafe {
            self.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                &HSTRING::from(url),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
            )
        }
    }
}

/// Refused frame documents waiting for their request, each with the filter
/// that routes that request here.
pub(crate) struct RefusedFrameDocuments<F: DocumentFilters> {
    filters: F,
    entries: VecDeque<(String, Instant)>,
}

impl<F: DocumentFilters> RefusedFrameDocuments<F> {
    pub(crate) fn new(filters: F) -> Self {
        Self {
            filters,
            entries: VecDeque::new(),
        }
    }

    /// Holds the request for a refused frame's `url`; false when it cannot
    /// be held and goes out as before.
    pub(crate) fn record(&mut self, url: &str, now: Instant) -> bool {
        self.expire(now);
        if self.entries.len() == CAPACITY {
            return false;
        }
        let url = without_fragment(url);
        if self.filters.add(url).is_err() {
            return false;
        }
        self.entries.push_back((url.to_owned(), now));
        true
    }

    /// Whether `url` is a refused frame's document; the refusal is spent.
    pub(crate) fn take(&mut self, url: &str, now: Instant) -> bool {
        self.expire(now);
        let url = without_fragment(url);
        let Some(index) = self.entries.iter().position(|(entry, _)| entry == url) else {
            return false;
        };
        self.release(index);
        true
    }

    /// The main frame is going to `url`: no refusal may answer it.
    pub(crate) fn forget(&mut self, url: &str, now: Instant) {
        self.expire(now);
        let url = without_fragment(url);
        while let Some(index) = self.entries.iter().position(|(entry, _)| entry == url) {
            self.release(index);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        while !self.entries.is_empty() {
            self.release(0);
        }
    }

    fn expire(&mut self, now: Instant) {
        while self
            .entries
            .front()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= LIFETIME)
        {
            self.release(0);
        }
    }

    fn release(&mut self, index: usize) {
        if let Some((url, _)) = self.entries.remove(index) {
            let _ = self.filters.remove(&url);
        }
    }
}

type Held = Rc<RefCell<RefusedFrameDocuments<ICoreWebView2_22>>>;

/// A view's refused frame requests: answered with an empty 403 when one
/// arrives, and dropped when the main frame itself goes to that address.
/// It does no matching of its own: the frame check has already judged it.
pub(crate) struct RefusedFrameRequests {
    core: ICoreWebView2,
    documents: Held,
    request_token: Option<i64>,
    navigation_token: Option<i64>,
}

impl RefusedFrameRequests {
    pub(crate) fn hold(core: &ICoreWebView2) -> windows_core::Result<Self> {
        let core22 = core.cast::<ICoreWebView2_22>()?;
        let environment = unsafe { core.cast::<ICoreWebView2_2>()?.Environment()? };
        let response = unsafe {
            environment.CreateWebResourceResponse(
                None,
                403,
                windows_core::w!("Blocked"),
                windows_core::w!("Cache-Control: no-store\r\nContent-Length: 0"),
            )?
        };
        let mut held = Self {
            core: core.clone(),
            documents: Rc::new(RefCell::new(RefusedFrameDocuments::new(core22))),
            request_token: None,
            navigation_token: None,
        };

        let documents = held.documents.clone();
        let requests = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            // Every request the content filter registers for reaches this
            // handler too; with no refusal waiting, nothing is read.
            if documents
                .try_borrow()
                .map_or(true, |documents| documents.is_empty())
            {
                return Ok(());
            }
            let Some(args) = args else {
                return Ok(());
            };
            let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
            unsafe { args.ResourceContext(&mut context)? };
            if context != COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
                return Ok(());
            }
            let request = unsafe { args.Request()? };
            let mut uri = PWSTR::null();
            unsafe { request.Uri(&mut uri)? };
            let Some(uri) = take_pwstr_bounded(uri, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT)
            else {
                return Ok(());
            };
            let refused = documents
                .try_borrow_mut()
                .is_ok_and(|mut documents| documents.take(&uri, Instant::now()));
            if refused {
                unsafe { args.SetResponse(&response)? };
            }
            Ok(())
        }));
        let mut token = 0_i64;
        unsafe { held.core.add_WebResourceRequested(&requests, &mut token)? };
        held.request_token = Some(token);

        let documents = held.documents.clone();
        let navigations = NavigationStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            if documents
                .try_borrow()
                .map_or(true, |documents| documents.is_empty())
            {
                return Ok(());
            }
            let mut uri = PWSTR::null();
            unsafe { args.Uri(&mut uri)? };
            if let (Some(uri), Ok(mut documents)) = (
                take_pwstr_bounded(uri, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT),
                documents.try_borrow_mut(),
            ) {
                documents.forget(&uri, Instant::now());
            }
            Ok(())
        }));
        let mut token = 0_i64;
        unsafe { held.core.add_NavigationStarting(&navigations, &mut token)? };
        held.navigation_token = Some(token);
        Ok(held)
    }

    pub(crate) fn handle(&self) -> RefusedFrameHandle {
        RefusedFrameHandle(Rc::downgrade(&self.documents))
    }
}

/// What the frame check keeps of the view's held requests: nothing once the
/// view lets them go.
pub(crate) struct RefusedFrameHandle(
    std::rc::Weak<RefCell<RefusedFrameDocuments<ICoreWebView2_22>>>,
);

impl RefusedFrameHandle {
    pub(crate) fn refuse(&self, url: &str) {
        if let Some(documents) = self.0.upgrade() {
            if let Ok(mut documents) = documents.try_borrow_mut() {
                documents.record(url, Instant::now());
            }
        }
    }
}

impl Drop for RefusedFrameRequests {
    fn drop(&mut self) {
        if let Some(token) = self.request_token.take() {
            let _ = unsafe { self.core.remove_WebResourceRequested(token) };
        }
        if let Some(token) = self.navigation_token.take() {
            let _ = unsafe { self.core.remove_NavigationStarting(token) };
        }
        if let Ok(mut documents) = self.documents.try_borrow_mut() {
            documents.clear();
        }
    }
}

/// A fragment is never sent, so the frame check sees it and the request
/// does not.
fn without_fragment(url: &str) -> &str {
    url.split_once('#').map_or(url, |(sent, _)| sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const AD: &str = "https://ads.example/frame?slot=1";

    /// Filters as WebView2 counts them: per address, added and removed.
    #[derive(Default)]
    struct CountedFilters {
        live: RefCell<std::collections::HashMap<String, usize>>,
        fail_adds: std::cell::Cell<bool>,
    }

    impl CountedFilters {
        fn count(&self, url: &str) -> usize {
            self.live.borrow().get(url).copied().unwrap_or(0)
        }
        fn total(&self) -> usize {
            self.live.borrow().values().sum()
        }
    }

    impl DocumentFilters for &CountedFilters {
        fn add(&self, url: &str) -> windows_core::Result<()> {
            if self.fail_adds.get() {
                return Err(windows::Win32::Foundation::E_INVALIDARG.into());
            }
            *self.live.borrow_mut().entry(url.to_owned()).or_default() += 1;
            Ok(())
        }
        fn remove(&self, url: &str) -> windows_core::Result<()> {
            let mut live = self.live.borrow_mut();
            let count = live.get_mut(url).expect("removed a filter never added");
            *count -= 1;
            if *count == 0 {
                live.remove(url);
            }
            Ok(())
        }
    }

    #[test]
    fn a_refused_frame_document_is_answered_once_and_its_filter_goes() {
        let filters = CountedFilters::default();
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        assert!(refused.record(AD, now));
        assert_eq!(filters.count(AD), 1);
        assert!(refused.take(AD, now));
        assert_eq!(filters.count(AD), 0);
        assert!(!refused.take(AD, now));
        assert!(refused.is_empty());
    }

    #[test]
    fn two_refusals_of_one_address_add_and_remove_its_filter_twice() {
        let filters = CountedFilters::default();
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        assert!(refused.record(AD, then));
        let later = then + Duration::from_secs(3);
        assert!(refused.record(AD, later));
        assert_eq!(filters.count(AD), 2);

        // The first lapses while the second is still waiting: the address
        // stays filtered for it.
        let lapsed = then + LIFETIME;
        assert!(!refused.take("https://news.example/", lapsed));
        assert_eq!(filters.count(AD), 1);
        assert!(refused.take(AD, lapsed));
        assert_eq!(filters.count(AD), 0);
        assert!(!refused.take(AD, lapsed));
    }

    #[test]
    fn an_unrelated_document_passes() {
        let filters = CountedFilters::default();
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        refused.record(AD, now);
        assert!(!refused.take("https://ads.example/frame?slot=2", now));
        assert!(!refused.take("https://news.example/", now));
        assert!(refused.take(AD, now));
    }

    #[test]
    fn a_worker_script_the_page_loads_is_not_taken_for_its_frame() {
        let filters = CountedFilters::default();
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        refused.record("https://ads.example/frame.html", now);
        assert!(!refused.take("https://ads.example/worker.js", now));
        assert!(!refused.take("https://ads.example/frame.html.js", now));
        assert!(refused.take("https://ads.example/frame.html", now));
    }

    #[test]
    fn the_fragment_the_frame_check_sees_is_not_part_of_the_request() {
        let filters = CountedFilters::default();
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        refused.record(
            "https://ads.example/outer.html#url=https%3A%2F%2Fnews.example",
            now,
        );
        assert_eq!(filters.count("https://ads.example/outer.html"), 1);
        assert!(refused.take("https://ads.example/outer.html", now));

        refused.record(AD, now);
        assert!(refused.take(&format!("{AD}#late"), now));

        assert_eq!(
            without_fragment("https://a.example/p?q=1#f#g"),
            "https://a.example/p?q=1"
        );
        assert_eq!(
            without_fragment("https://a.example/p?q=1"),
            "https://a.example/p?q=1"
        );
        assert_eq!(
            without_fragment("https://a.example/#"),
            "https://a.example/"
        );
    }

    #[test]
    fn an_expired_refusal_passes_and_takes_its_filter_with_it() {
        let filters = CountedFilters::default();
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        refused.record(AD, then);
        assert!(!refused.take(AD, then + LIFETIME));
        assert!(refused.is_empty());
        assert_eq!(filters.total(), 0);
    }

    #[test]
    fn the_main_frame_going_to_a_refused_address_is_never_answered() {
        let filters = CountedFilters::default();
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        refused.record("https://shop.example/", now);
        refused.record("https://shop.example/", now);
        refused.forget("https://shop.example/#top", now);
        assert_eq!(filters.total(), 0);
        assert!(!refused.take("https://shop.example/", now));
    }

    #[test]
    fn a_main_document_at_a_stale_refused_address_is_never_answered() {
        let filters = CountedFilters::default();
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        refused.record("https://shop.example/", then);
        let later = then + LIFETIME + Duration::from_millis(1);
        assert!(!refused.take("https://shop.example/", later));
        assert_eq!(filters.total(), 0);
    }

    #[test]
    fn an_answered_refusal_leaves_no_filter_for_a_later_load() {
        let filters = CountedFilters::default();
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        refused.record(AD, now);
        assert!(refused.take(AD, now));
        assert_eq!(filters.total(), 0);
        assert!(!refused.take(AD, now + Duration::from_millis(10)));
    }

    #[test]
    fn a_full_view_lets_a_further_refusal_through_and_keeps_those_waiting() {
        let filters = CountedFilters::default();
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        for slot in 0..CAPACITY {
            assert!(refused.record(&format!("https://ads.example/frame?slot={slot}"), now));
        }
        assert!(!refused.record("https://ads.example/frame?slot=late", now));
        assert_eq!(filters.total(), CAPACITY);
        assert_eq!(filters.count("https://ads.example/frame?slot=late"), 0);
        assert!(refused.take("https://ads.example/frame?slot=0", now));
        assert!(refused.record("https://ads.example/frame?slot=late", now));
    }

    #[test]
    fn a_filter_that_cannot_be_added_lets_the_request_through() {
        let filters = CountedFilters::default();
        filters.fail_adds.set(true);
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        assert!(!refused.record(AD, now));
        assert!(refused.is_empty());
        assert!(!refused.take(AD, now));
    }

    #[test]
    fn letting_the_view_go_removes_every_filter() {
        let filters = CountedFilters::default();
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::new(&filters);
        refused.record(AD, now);
        refused.record(AD, now);
        refused.record("https://ads.example/other", now);
        refused.clear();
        assert_eq!(filters.total(), 0);
    }
}
