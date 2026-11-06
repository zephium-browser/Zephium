//! Frame documents the frame check refused, held before their requests leave.
//! WebView2 starts a frame's request without waiting for
//! FrameNavigationStarting to answer, so a cancel there only stops the frame
//! from showing what the server already sent. The view therefore filters its
//! document requests from the start, and answers a request for a refused
//! frame's address itself, before it is sent. Every other document passes
//! untouched.

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
use windows_core::{Interface, PCWSTR, PWSTR};

use super::{take_pwstr_bounded, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT};

/// Refusals a view holds at once. A refusal past this is let through as
/// before rather than displacing one still waiting for its request.
const CAPACITY: usize = 64;
/// A request follows its frame check within milliseconds; a refusal older
/// than this belongs to a navigation that never sent one.
const LIFETIME: Duration = Duration::from_secs(5);
/// Every web document of the view, cross-origin frames included.
const PATTERNS: [PCWSTR; 2] = [windows_core::w!("https://*"), windows_core::w!("http://*")];

/// Refused frame documents waiting for their request.
#[derive(Default)]
pub(crate) struct RefusedFrameDocuments {
    entries: VecDeque<(String, Instant)>,
}

impl RefusedFrameDocuments {
    /// Holds the request for a refused frame's `url`; false when it cannot
    /// be held and goes out as before.
    pub(crate) fn record(&mut self, url: &str, now: Instant) -> bool {
        self.expire(now);
        if self.entries.len() == CAPACITY {
            return false;
        }
        self.entries
            .push_back((without_fragment(url).to_owned(), now));
        true
    }

    /// Whether `url` is a refused frame's document; the refusal is spent.
    pub(crate) fn take(&mut self, url: &str, now: Instant) -> bool {
        self.expire(now);
        let url = without_fragment(url);
        let Some(index) = self.entries.iter().position(|(entry, _)| entry == url) else {
            return false;
        };
        self.entries.remove(index);
        true
    }

    /// The main frame is going to `url`: no refusal may answer it.
    pub(crate) fn forget(&mut self, url: &str, now: Instant) {
        self.expire(now);
        let url = without_fragment(url);
        self.entries.retain(|(entry, _)| entry != url);
    }

    /// Nothing is waiting, so a request needs no look.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn expire(&mut self, now: Instant) {
        while self
            .entries
            .front()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= LIFETIME)
        {
            self.entries.pop_front();
        }
    }
}

/// Where the view's document filter is registered.
pub(crate) trait DocumentFilterTarget {
    fn add(&self, pattern: PCWSTR) -> windows_core::Result<()>;
    fn remove(&self, pattern: PCWSTR) -> windows_core::Result<()>;
}

impl DocumentFilterTarget for ICoreWebView2_22 {
    fn add(&self, pattern: PCWSTR) -> windows_core::Result<()> {
        // SAFETY: a live interface on the view's thread; the pattern is a
        // static wide string.
        unsafe {
            self.AddWebResourceRequestedFilterWithRequestSourceKinds(
                pattern,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
            )
        }
    }

    fn remove(&self, pattern: PCWSTR) -> windows_core::Result<()> {
        // SAFETY: as in `add`, for a filter `add` installed.
        unsafe {
            self.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                pattern,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
            )
        }
    }
}

/// The view's document filter, registered whole or not at all, and taken
/// off with the view. It is ours alone, so removing it removes no one
/// else's.
pub(crate) struct DocumentFilter<T: DocumentFilterTarget> {
    target: T,
    added: usize,
}

impl<T: DocumentFilterTarget> DocumentFilter<T> {
    pub(crate) fn register(target: T) -> windows_core::Result<Self> {
        let mut filter = Self { target, added: 0 };
        for pattern in PATTERNS {
            filter.target.add(pattern)?;
            filter.added += 1;
        }
        Ok(filter)
    }
}

impl<T: DocumentFilterTarget> Drop for DocumentFilter<T> {
    fn drop(&mut self) {
        while self.added > 0 {
            self.added -= 1;
            let _ = self.target.remove(PATTERNS[self.added]);
        }
    }
}

type Held = Rc<RefCell<RefusedFrameDocuments>>;

/// A view's refused frame requests: answered with an empty 403 when one
/// arrives, and never once the main frame itself goes to that address.
/// It does no matching of its own: the frame check has already judged it.
pub(crate) struct RefusedFrameRequests {
    core: ICoreWebView2,
    documents: Held,
    request_token: Option<i64>,
    navigation_token: Option<i64>,
    // Taken off after the handlers, when this is dropped.
    _filter: Option<DocumentFilter<ICoreWebView2_22>>,
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
            documents: Rc::default(),
            request_token: None,
            navigation_token: None,
            _filter: None,
        };

        let documents = held.documents.clone();
        let requests = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            // Every document of the view, and every request the content
            // filter registers for, reaches this handler; with no refusal
            // waiting, nothing is read.
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

        held._filter = Some(DocumentFilter::register(core22)?);
        Ok(held)
    }

    pub(crate) fn handle(&self) -> RefusedFrameHandle {
        RefusedFrameHandle(Rc::downgrade(&self.documents))
    }
}

/// What the frame check keeps of the view's held requests: nothing once the
/// view lets them go.
pub(crate) struct RefusedFrameHandle(std::rc::Weak<RefCell<RefusedFrameDocuments>>);

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
            documents.entries.clear();
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
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    const AD: &str = "https://ads.example/frame?slot=1";

    #[test]
    fn a_refused_frame_document_is_answered_once() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        assert!(refused.record(AD, now));
        assert!(refused.take(AD, now));
        assert!(!refused.take(AD, now));
        assert!(refused.is_empty());
    }

    #[test]
    fn nothing_waiting_needs_no_look() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        assert!(refused.is_empty());
        refused.record(AD, now);
        assert!(!refused.is_empty());
        refused.take(AD, now);
        assert!(refused.is_empty());
    }

    #[test]
    fn an_unrelated_document_passes() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record(AD, now);
        assert!(!refused.take("https://ads.example/frame?slot=2", now));
        assert!(!refused.take("https://ads.example/frameXslot=1", now));
        assert!(!refused.take("https://news.example/", now));
        assert!(refused.take(AD, now));
    }

    #[test]
    fn a_worker_script_the_page_loads_is_not_taken_for_its_frame() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record("https://ads.example/frame.html", now);
        assert!(!refused.take("https://ads.example/worker.js", now));
        assert!(!refused.take("https://ads.example/frame.html.js", now));
        assert!(refused.take("https://ads.example/frame.html", now));
    }

    #[test]
    fn the_fragment_the_frame_check_sees_is_not_part_of_the_request() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record(
            "https://ads.example/outer.html#url=https%3A%2F%2Fnews.example",
            now,
        );
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
    fn two_refusals_of_one_address_are_answered_one_each() {
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record(AD, then);
        refused.record(AD, then + Duration::from_secs(3));
        // The first lapses; the second still waits.
        assert!(refused.take(AD, then + LIFETIME));
        assert!(!refused.take(AD, then + LIFETIME));
    }

    #[test]
    fn an_expired_refusal_passes() {
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record(AD, then);
        assert!(!refused.take(AD, then + LIFETIME));
        assert!(refused.is_empty());
    }

    #[test]
    fn the_main_frame_going_to_a_refused_address_is_never_answered() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record("https://shop.example/", now);
        refused.record("https://shop.example/", now);
        refused.record(AD, now);
        refused.forget("https://shop.example/#top", now);
        assert!(!refused.take("https://shop.example/", now));
        assert!(refused.take(AD, now));
    }

    #[test]
    fn a_main_frame_navigation_elsewhere_lets_a_lapsed_refusal_go() {
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record(AD, then);
        refused.forget("https://news.example/", then + LIFETIME);
        assert!(refused.is_empty());
    }

    #[test]
    fn a_main_document_at_a_stale_refused_address_is_never_answered() {
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record("https://shop.example/", then);
        let later = then + LIFETIME + Duration::from_millis(1);
        assert!(!refused.take("https://shop.example/", later));
    }

    #[test]
    fn a_full_view_lets_a_further_refusal_through_and_keeps_those_waiting() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        for slot in 0..CAPACITY {
            assert!(refused.record(&format!("https://ads.example/frame?slot={slot}"), now));
        }
        assert!(!refused.record("https://ads.example/frame?slot=late", now));
        assert!(!refused.take("https://ads.example/frame?slot=late", now));
        assert!(refused.take("https://ads.example/frame?slot=0", now));
        assert!(refused.record("https://ads.example/frame?slot=late", now));
    }

    /// Filters as WebView2 counts them: per pattern, added and removed.
    #[derive(Default)]
    struct CountedFilters {
        live: RefCell<HashMap<String, usize>>,
        fail_add_at: Cell<Option<usize>>,
        adds: Cell<usize>,
    }

    impl CountedFilters {
        fn total(&self) -> usize {
            self.live.borrow().values().sum()
        }
    }

    impl DocumentFilterTarget for &CountedFilters {
        fn add(&self, pattern: PCWSTR) -> windows_core::Result<()> {
            let attempt = self.adds.get();
            self.adds.set(attempt + 1);
            if self.fail_add_at.get() == Some(attempt) {
                return Err(windows::Win32::Foundation::E_INVALIDARG.into());
            }
            let pattern = unsafe { pattern.to_string() }.expect("pattern is UTF-16");
            *self.live.borrow_mut().entry(pattern).or_default() += 1;
            Ok(())
        }

        fn remove(&self, pattern: PCWSTR) -> windows_core::Result<()> {
            let pattern = unsafe { pattern.to_string() }.expect("pattern is UTF-16");
            let mut live = self.live.borrow_mut();
            let count = live
                .get_mut(&pattern)
                .expect("removed a filter never added");
            *count -= 1;
            if *count == 0 {
                live.remove(&pattern);
            }
            Ok(())
        }
    }

    #[test]
    fn the_view_filters_its_web_documents_and_takes_the_filter_off_with_it() {
        let filters = CountedFilters::default();
        let filter = DocumentFilter::register(&filters).expect("registered");
        assert_eq!(filters.total(), 2);
        assert_eq!(filters.live.borrow().get("https://*"), Some(&1));
        assert_eq!(filters.live.borrow().get("http://*"), Some(&1));
        drop(filter);
        assert_eq!(filters.total(), 0);
    }

    #[test]
    fn a_filter_that_cannot_be_registered_whole_leaves_none_behind() {
        let filters = CountedFilters::default();
        filters.fail_add_at.set(Some(1));
        assert!(DocumentFilter::register(&filters).is_err());
        assert_eq!(filters.total(), 0);
    }
}
