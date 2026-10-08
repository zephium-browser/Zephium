use objc2_foundation::NSString;
// Copyright 2020-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::{Arc, Mutex};

use objc2::{
  define_class, msg_send, rc::Retained, runtime::NSObject, DefinedClass, MainThreadOnly,
};
use objc2_foundation::{MainThreadMarker, NSError, NSObjectProtocol};
#[cfg(target_os = "macos")]
use objc2_foundation::{
  NSURLAuthenticationChallenge, NSURLAuthenticationMethodServerTrust, NSURLCredential,
  NSURLSessionAuthChallengeDisposition,
};
use objc2_web_kit::{
  WKDownload, WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
  WKNavigationResponse, WKNavigationResponsePolicy,
};

#[cfg(target_os = "ios")]
use crate::wkwebview::ios::WKWebView::WKWebView;
#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;

use crate::{
  url_from_webview,
  wkwebview::{
    download::{navigation_download_action, navigation_download_response},
    navigation::{
      begin_programmatic_navigation, cancel_programmatic_navigation, did_commit_navigation,
      did_fail_navigation, did_finish_navigation, did_receive_server_redirect,
      did_start_provisional_navigation, navigation_policy, navigation_policy_response,
      register_programmatic_navigation, web_content_process_did_terminate,
      AppleNavigationEventState,
    },
  },
  AppleNavigationAction, NavigationEvent, NavigationFailure, NavigationId, PageLoadEvent,
  WryWebView,
};

use super::wry_download_delegate::WryDownloadDelegate;

pub struct BlockedLoadCounter {
  pub identifier: Retained<NSString>,
  pub previous_identifier: Option<Retained<NSString>>,
  pub count: std::cell::Cell<u64>,
  pub aggregate: Arc<(
    std::sync::atomic::AtomicU64,
    std::sync::atomic::AtomicBool,
    std::sync::atomic::AtomicBool,
  )>,
}
impl BlockedLoadCounter {
  pub fn flush(&self, reset: bool) {
    let count = self.count.replace(0);
    if !reset && count != 0 {
      let _ = self.aggregate.0.fetch_update(
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
        |n| Some(n.saturating_add(count)),
      );
    }
  }
}
impl Drop for BlockedLoadCounter {
  fn drop(&mut self) {
    self.flush(false);
  }
}

pub struct WryNavigationDelegateIvars {
  pub blocked_loads: std::cell::RefCell<Option<BlockedLoadCounter>>,
  pub pending_scripts: Arc<Mutex<Option<Vec<String>>>>,
  pub has_download_handler: bool,
  #[cfg(target_os = "macos")]
  pub new_window_req_handler:
    Option<std::rc::Rc<dyn Fn(String, crate::NewWindowFeatures) -> crate::NewWindowResponse>>,
  pub navigation_policy_function: Box<dyn Fn(String, AppleNavigationAction) -> bool>,
  #[cfg(target_os = "macos")]
  pub main_frame_navigation_attempt_handler: Option<Box<dyn Fn(String)>>,
  pub download_delegate: Option<Retained<WryDownloadDelegate>>,
  pub on_page_load_handler: Option<Box<dyn Fn(PageLoadEvent)>>,
  pub navigation_event_handler: Option<Box<dyn Fn(NavigationEvent)>>,
  pub navigation_failure_handler: Option<Box<dyn Fn(NavigationId, NavigationFailure)>>,
  pub navigation_presentation_guard: Option<Box<dyn Fn()>>,
  // `AppleNavigationEventState` intentionally uses a non-wrapping `u128`
  // record token, which gives it 16-byte alignment on Apple 64-bit targets.
  // objc2 0.6 only supports Objective-C ivars aligned to at most 8 bytes, so
  // keep the state in an alignment-aware Rust allocation and store only the
  // Box pointer in the native object. The delegate still owns and drops the
  // state exactly with its Objective-C lifetime.
  pub navigation_event_state: Box<Mutex<AppleNavigationEventState>>,
  pub on_web_content_process_terminate_handler: Option<Box<dyn Fn()>>,
}

define_class!(
  #[unsafe(super(NSObject))]
  #[thread_kind = MainThreadOnly]
  #[ivars = WryNavigationDelegateIvars]
  pub struct WryNavigationDelegate;

  unsafe impl NSObjectProtocol for WryNavigationDelegate {
    #[unsafe(method(respondsToSelector:))]
    fn responds_to_selector(&self, selector: objc2::runtime::Sel) -> objc2::runtime::Bool {
      if selector == objc2::sel!(_webView:contentRuleListWithIdentifier:performedAction:forURL:) {
        return self.ivars().blocked_loads.borrow().is_some().into();
      }
      unsafe { msg_send![super(self), respondsToSelector: selector] }
    }
  }

  // WebKit's macOS context-menu download entry point is an optional private
  // delegate selector (present since the WKDownload API). It delivers the
  // public WKDownload object, not a URL to be replayed. Engines that do not
  // send it simply cannot use this path; navigation download hooks remain.
  #[cfg(target_os = "macos")]
  impl WryNavigationDelegate {
    #[unsafe(method(_webView:contentRuleListWithIdentifier:performedAction:forURL:))]
    fn content_rule_action(
      &self,
      _view: &WKWebView,
      identifier: &NSString,
      action: &objc2::runtime::AnyObject,
      _url: Option<&objc2_foundation::NSURL>,
    ) {
      let counter = self.ivars().blocked_loads.borrow();
      let Some(counter) = counter.as_ref() else {
        return;
      };
      if !identifier.isEqualToString(&counter.identifier)
        && !counter
          .previous_identifier
          .as_ref()
          .is_some_and(|previous| identifier.isEqualToString(previous))
      {
        return;
      }
      // The optional SPI getter is checked once when counting is installed.
      let blocked: bool = unsafe { msg_send![action, blockedLoad] };
      if blocked {
        let previous = counter.count.get();
        counter.count.set(previous.saturating_add(1));
        if previous == 0 {
          counter
            .aggregate
            .1
            .store(true, std::sync::atomic::Ordering::Relaxed);
        }
      }
    }
    #[unsafe(method(_webView:contextMenuDidCreateDownload:))]
    fn context_menu_download(&self, _webview: &WKWebView, download: &WKDownload) {
      if !self.ivars().has_download_handler {
        unsafe { download.cancel(None) };
        return;
      }
      if let Some(delegate) = &self.ivars().download_delegate {
        if let Some(native) = &delegate.ivars().native {
          if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| native(download))).is_err() {
            unsafe { download.cancel(None) };
          }
          return;
        }
      }
      unsafe { download.cancel(None) };
    }
  }

  unsafe impl WKNavigationDelegate for WryNavigationDelegate {
    #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
    fn navigation_policy(
      &self,
      webview: &WKWebView,
      action: &WKNavigationAction,
      handler: &block2::Block<dyn Fn(WKNavigationActionPolicy)>,
    ) {
      navigation_policy(self, webview, action, handler);
    }

    #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
    fn navigation_policy_response(
      &self,
      webview: &WKWebView,
      response: &WKNavigationResponse,
      handler: &block2::Block<dyn Fn(WKNavigationResponsePolicy)>,
    ) {
      navigation_policy_response(self, webview, response, handler);
    }

    #[unsafe(method(webView:didStartProvisionalNavigation:))]
    fn did_start_provisional_navigation(
      &self,
      webview: &WKWebView,
      navigation: Option<&WKNavigation>,
    ) {
      if let Some(navigation) = navigation {
        did_start_provisional_navigation(self, webview, navigation);
      }
    }

    #[unsafe(method(webView:didReceiveServerRedirectForProvisionalNavigation:))]
    fn did_receive_server_redirect(&self, webview: &WKWebView, navigation: Option<&WKNavigation>) {
      if let Some(navigation) = navigation {
        did_receive_server_redirect(self, webview, navigation);
      }
    }

    #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
    fn did_fail_provisional_navigation(
      &self,
      webview: &WKWebView,
      navigation: Option<&WKNavigation>,
      error: &NSError,
    ) {
      if let Some(navigation) = navigation {
        did_fail_navigation(self, webview, navigation, error, "provisional");
      }
    }

    #[unsafe(method(webView:didFinishNavigation:))]
    fn did_finish_navigation(&self, webview: &WKWebView, navigation: &WKNavigation) {
      did_finish_navigation(self, webview, navigation);
    }

    #[unsafe(method(webView:didCommitNavigation:))]
    fn did_commit_navigation(&self, webview: &WKWebView, navigation: &WKNavigation) {
      did_commit_navigation(self, webview, navigation);
    }

    #[unsafe(method(webView:didFailNavigation:withError:))]
    fn did_fail_navigation(
      &self,
      webview: &WKWebView,
      navigation: Option<&WKNavigation>,
      error: &NSError,
    ) {
      if let Some(navigation) = navigation {
        did_fail_navigation(self, webview, navigation, error, "committed");
      }
    }

    #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
    fn navigation_download_action(
      &self,
      webview: &WKWebView,
      action: &WKNavigationAction,
      download: &WKDownload,
    ) {
      navigation_download_action(self, webview, action, download);
    }

    #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
    fn navigation_download_response(
      &self,
      webview: &WKWebView,
      response: &WKNavigationResponse,
      download: &WKDownload,
    ) {
      navigation_download_response(self, webview, response, download);
    }

    #[unsafe(method(webViewWebContentProcessDidTerminate:))]
    fn web_content_process_did_terminate(&self, webview: &WKWebView) {
      web_content_process_did_terminate(self, webview);
    }

    #[cfg(target_os = "macos")]
    #[unsafe(method(webView:didReceiveAuthenticationChallenge:completionHandler:))]
    unsafe fn deny_authentication_challenge(
      &self,
      _webview: &WKWebView,
      challenge: &NSURLAuthenticationChallenge,
      completion_handler: &block2::DynBlock<
        dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential),
      >,
    ) {
      // HTTP authentication, client certificates, and server-trust decisions
      // have different safe defaults. Preserve WebKit/Foundation's ordinary
      // system trust evaluation for TLS certificates; cancel every challenge
      // that could otherwise select credentials or expose native auth UI.
      let protection_space = challenge.protectionSpace();
      let method = protection_space.authenticationMethod();
      let disposition = if method.isEqualToString(NSURLAuthenticationMethodServerTrust) {
        NSURLSessionAuthChallengeDisposition::PerformDefaultHandling
      } else {
        NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge
      };
      completion_handler.call((disposition, std::ptr::null_mut()));
    }
  }
);

impl WryNavigationDelegate {
  pub(crate) fn begin_programmatic_navigation(&self) -> bool {
    begin_programmatic_navigation(self)
  }

  pub(crate) fn cancel_programmatic_navigation(&self) {
    cancel_programmatic_navigation(self);
  }

  pub(crate) fn register_programmatic_navigation(
    &self,
    navigation: &WKNavigation,
    url: String,
  ) -> bool {
    register_programmatic_navigation(self, navigation, url)
  }

  #[allow(clippy::too_many_arguments)]
  pub fn new(
    webview: Retained<WryWebView>,
    pending_scripts: Arc<Mutex<Option<Vec<String>>>>,
    has_download_handler: bool,
    #[cfg(target_os = "macos")] new_window_req_handler: Option<
      std::rc::Rc<dyn Fn(String, crate::NewWindowFeatures) -> crate::NewWindowResponse>,
    >,
    navigation_handler: Option<Box<dyn Fn(String) -> bool>>,
    apple_navigation_action_handler: Option<Box<dyn Fn(String, AppleNavigationAction) -> bool>>,
    #[cfg(target_os = "macos")] main_frame_navigation_attempt_handler: Option<Box<dyn Fn(String)>>,
    download_delegate: Option<Retained<WryDownloadDelegate>>,
    on_page_load_handler: Option<Box<dyn Fn(PageLoadEvent, String)>>,
    navigation_event_handler: Option<Box<dyn Fn(NavigationEvent)>>,
    navigation_failure_handler: Option<Box<dyn Fn(NavigationId, NavigationFailure)>>,
    navigation_presentation_guard: Option<Box<dyn Fn()>>,
    on_web_content_process_terminate_handler: Option<Box<dyn Fn()>>,
    mtm: MainThreadMarker,
  ) -> Retained<Self> {
    let navigation_policy_function =
      Box::new(move |url: String, action: AppleNavigationAction| -> bool {
        if let Some(navigation_handler) = apple_navigation_action_handler.as_ref() {
          (navigation_handler)(url, action)
        } else {
          navigation_handler
            .as_ref()
            .map_or(true, |navigation_handler| (navigation_handler)(url))
        }
      });

    let on_page_load_handler = if let Some(handler) = on_page_load_handler {
      let custom_handler = Box::new(move |event| {
        handler(event, url_from_webview(&webview).unwrap_or_default());
      }) as Box<dyn Fn(PageLoadEvent)>;
      Some(custom_handler)
    } else {
      None
    };

    let on_web_content_process_terminate_handler =
      if let Some(handler) = on_web_content_process_terminate_handler {
        let custom_handler = Box::new(move || {
          handler();
        }) as Box<dyn Fn()>;
        Some(custom_handler)
      } else {
        None
      };

    let delegate = mtm
      .alloc::<WryNavigationDelegate>()
      .set_ivars(WryNavigationDelegateIvars {
        blocked_loads: Default::default(),
        pending_scripts,
        navigation_policy_function,
        #[cfg(target_os = "macos")]
        main_frame_navigation_attempt_handler,
        has_download_handler,
        #[cfg(target_os = "macos")]
        new_window_req_handler,
        download_delegate,
        on_page_load_handler,
        navigation_event_handler,
        navigation_failure_handler,
        navigation_presentation_guard,
        navigation_event_state: Box::new(Mutex::new(AppleNavigationEventState::default())),
        on_web_content_process_terminate_handler,
      });

    unsafe { msg_send![super(delegate), init] }
  }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
  use std::mem::{align_of, size_of};

  use objc2::ClassType;

  use super::*;

  #[test]
  fn navigation_delegate_ivars_register_with_a_supported_native_layout() {
    let ivars_alignment = align_of::<WryNavigationDelegateIvars>();
    assert!(
      matches!(ivars_alignment, 1 | 2 | 4 | 8),
      "objc2 cannot register an ivar with alignment {ivars_alignment}"
    );

    // `class()` executes objc2's real ClassBuilder registration path. This is
    // the path that aborted before the navigation state was boxed.
    let class = WryNavigationDelegate::class();
    let ivar = class
      .instance_variable(c"ivars")
      .expect("defined-class ivars must be registered with Objective-C");
    let ivar_offset = usize::try_from(ivar.offset()).expect("ivar offset must be non-negative");
    let ivar_end = ivar_offset
      .checked_add(size_of::<WryNavigationDelegateIvars>())
      .expect("ivar extent must fit in usize");

    assert_eq!(ivar_offset % ivars_alignment, 0);
    assert!(ivar_end <= class.instance_size());
    assert!(class.instance_size() >= NSObject::class().instance_size());
  }
}
