//! Frame documents the frame check refused, handed to the view's request
//! filter. WebView2 starts a frame's request without waiting for
//! FrameNavigationStarting to answer, so a cancel there only stops the frame
//! from showing what the server already sent. The request itself is held at
//! WebResourceRequested, which the browser raises after the frame check and
//! before the request leaves.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2_2, ICoreWebView2_22, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
};
use webview2_com::WebResourceRequestedEventHandler;
use windows_core::{Interface, PCWSTR, PWSTR};

use super::{take_pwstr_bounded, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT};

/// Refusals waiting for their request. Each is taken by the first request
/// for its address, so a view never keeps more than a burst of them.
const CAPACITY: usize = 64;
/// A request follows its frame check within milliseconds; a refusal older
/// than this belongs to a navigation that never sent one.
const LIFETIME: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct RefusedFrameDocuments {
    entries: VecDeque<(String, Instant)>,
}

impl RefusedFrameDocuments {
    pub(crate) fn record(&mut self, url: &str, now: Instant) {
        self.expire(now);
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries
            .push_back((without_fragment(url).to_owned(), now));
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

const PATTERNS: [PCWSTR; 2] = [windows_core::w!("https://*"), windows_core::w!("http://*")];

/// The view's DOCUMENT-context requests, answered with an empty 403 when
/// they are a refused frame's. Main documents never raise
/// FrameNavigationStarting, so none is ever refused here. It does no matching
/// of its own: the frame check has already judged the address.
pub(crate) struct RefusedFrameRequests {
    core: ICoreWebView2,
    core22: ICoreWebView2_22,
    token: Option<i64>,
    filters: usize,
}

impl RefusedFrameRequests {
    pub(crate) fn hold(
        core: &ICoreWebView2,
        refused: Rc<RefCell<RefusedFrameDocuments>>,
    ) -> windows_core::Result<Self> {
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
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            // Every request the content filter registers for reaches this
            // handler too; with no refusal waiting, nothing is read.
            if refused
                .try_borrow()
                .map_or(true, |refused| refused.is_empty())
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
            let held = refused
                .try_borrow_mut()
                .is_ok_and(|mut refused| refused.take(&uri, Instant::now()));
            if held {
                unsafe { args.SetResponse(&response)? };
            }
            Ok(())
        }));
        let mut held = Self {
            core: core.clone(),
            core22,
            token: None,
            filters: 0,
        };
        for pattern in PATTERNS {
            unsafe {
                held.core22
                    .AddWebResourceRequestedFilterWithRequestSourceKinds(
                        pattern,
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
                        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
                    )?;
            }
            held.filters += 1;
        }
        let mut token = 0_i64;
        unsafe { held.core.add_WebResourceRequested(&handler, &mut token)? };
        held.token = Some(token);
        Ok(held)
    }
}

impl Drop for RefusedFrameRequests {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            let _ = unsafe { self.core.remove_WebResourceRequested(token) };
        }
        while self.filters > 0 {
            self.filters -= 1;
            let _ = unsafe {
                self.core22
                    .RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                        PATTERNS[self.filters],
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
                        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
                    )
            };
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

    const AD: &str = "https://ads.example/frame?slot=1";

    #[test]
    fn a_refused_frame_document_is_answered_once() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record(AD, now);
        assert!(refused.take(AD, now));
        assert!(!refused.take(AD, now));
        assert!(refused.is_empty());
    }

    #[test]
    fn an_unrelated_document_passes() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record(AD, now);
        assert!(!refused.take("https://ads.example/frame?slot=2", now));
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
    fn an_expired_refusal_passes() {
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record(AD, then);
        assert!(!refused.take(AD, then + LIFETIME));
        assert!(refused.is_empty());
    }

    #[test]
    fn a_main_document_at_a_stale_refused_address_is_never_answered() {
        let then = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        refused.record("https://shop.example/", then);
        let later = then + LIFETIME + Duration::from_millis(1);
        assert!(!refused.take("https://shop.example/", later));
        assert!(!refused.take("https://shop.example/", later));
    }

    #[test]
    fn a_full_set_lets_its_oldest_refusal_go() {
        let now = Instant::now();
        let mut refused = RefusedFrameDocuments::default();
        for slot in 0..=CAPACITY {
            refused.record(&format!("https://ads.example/frame?slot={slot}"), now);
        }
        assert!(!refused.take("https://ads.example/frame?slot=0", now));
        assert!(refused.take("https://ads.example/frame?slot=1", now));
        assert!(refused.take(&format!("https://ads.example/frame?slot={CAPACITY}"), now));
    }
}
