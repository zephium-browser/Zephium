// Copyright 2020-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#[cfg(target_os = "macos")]
use std::{
  cell::RefCell,
  num::NonZeroU64,
  panic::{catch_unwind, AssertUnwindSafe},
  ptr::null_mut,
  rc::Rc,
  sync::atomic::{AtomicU64, Ordering},
};

use block2::Block;
#[cfg(target_os = "macos")]
use block2::RcBlock;
use objc2::{
  define_class, msg_send,
  rc::Retained,
  runtime::{Bool, NSObject},
  DefinedClass, MainThreadOnly,
};
#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindowDelegate;
use objc2_foundation::{MainThreadMarker, NSObjectProtocol};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSArray, NSURL};

#[cfg(target_os = "macos")]
use objc2_web_kit::WKOpenPanelParameters;
use objc2_web_kit::{
  WKFrameInfo, WKMediaCaptureType, WKPermissionDecision, WKSecurityOrigin, WKUIDelegate,
};

#[cfg(target_os = "macos")]
use crate::{
  native_bounds::NativeStringLimit, PermissionOrigin, PermissionRequest,
  PermissionRequestDisposition, PermissionRequestId, PermissionRequestKind,
};
use crate::{
  native_bounds::{bounded_nsstring, PAGE_URL_LIMIT},
  NewWindowFeatures, NewWindowResponse, PermissionKind, PermissionResponse, WryWebView,
};

#[cfg(target_os = "macos")]
const MAX_PENDING_PERMISSION_REQUESTS: usize = 4;
#[cfg(target_os = "macos")]
const PERMISSION_ORIGIN_COMPONENT_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 512,
  max_utf8_bytes: 512,
};
#[cfg(target_os = "macos")]
static NEXT_PERMISSION_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(target_os = "macos")]
type PermissionDecisionHandler = RcBlock<dyn Fn(WKPermissionDecision)>;

#[cfg(target_os = "macos")]
struct PendingPermissionRequest {
  decision_handler: PermissionDecisionHandler,
}

#[cfg(target_os = "macos")]
struct BoundedPendingPermissionRequests<T> {
  entries: Vec<(PermissionRequestId, T)>,
}

#[cfg(target_os = "macos")]
impl<T> BoundedPendingPermissionRequests<T> {
  const fn new() -> Self {
    Self {
      entries: Vec::new(),
    }
  }

  fn insert(&mut self, id: PermissionRequestId, value: T) -> Result<(), T> {
    if self.entries.len() >= MAX_PENDING_PERMISSION_REQUESTS
      || self.entries.iter().any(|(candidate, _)| *candidate == id)
    {
      return Err(value);
    }
    self.entries.push((id, value));
    Ok(())
  }

  fn take(&mut self, id: PermissionRequestId) -> Option<T> {
    let index = self
      .entries
      .iter()
      .position(|(candidate, _)| *candidate == id)?;
    Some(self.entries.swap_remove(index).1)
  }

  fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
    self.entries.drain(..).map(|(_, value)| value)
  }
}

fn permission_decision(response: PermissionResponse) -> WKPermissionDecision {
  match response {
    PermissionResponse::Allow => WKPermissionDecision::Grant,
    PermissionResponse::Deny | PermissionResponse::Default | PermissionResponse::Prompt => {
      WKPermissionDecision::Deny
    }
  }
}

#[cfg(target_os = "macos")]
struct NewWindow {
  #[allow(dead_code)]
  ns_window: Retained<objc2_app_kit::NSWindow>,
  #[allow(dead_code)]
  webview: Retained<objc2_web_kit::WKWebView>,
  #[allow(dead_code)]
  delegate: Retained<WryNSWindowDelegate>,
}

#[cfg(target_os = "macos")]
struct WryNSWindowDelegateIvars {
  on_close: Box<dyn Fn()>,
}

#[cfg(target_os = "macos")]
define_class!(
  #[unsafe(super(NSObject))]
  #[thread_kind = MainThreadOnly]
  #[ivars = WryNSWindowDelegateIvars]
  struct WryNSWindowDelegate;

  unsafe impl NSObjectProtocol for WryNSWindowDelegate {}

  unsafe impl NSWindowDelegate for WryNSWindowDelegate {
    #[unsafe(method(windowWillClose:))]
    unsafe fn will_close(&self, _notification: &objc2_foundation::NSNotification) {
      let on_close = &self.ivars().on_close;
      on_close();
    }
  }
);

#[cfg(target_os = "macos")]
impl WryNSWindowDelegate {
  pub fn new(mtm: MainThreadMarker, on_close: Box<dyn Fn()>) -> Retained<Self> {
    let delegate = mtm
      .alloc::<WryNSWindowDelegate>()
      .set_ivars(WryNSWindowDelegateIvars { on_close });
    unsafe { msg_send![super(delegate), init] }
  }
}

pub struct WryWebViewUIDelegateIvars {
  #[cfg(target_os = "macos")]
  page_close_handler: Option<Box<dyn Fn()>>,
  #[cfg(target_os = "macos")]
  new_window_req_handler:
    Option<std::rc::Rc<dyn Fn(String, NewWindowFeatures) -> NewWindowResponse>>,
  #[cfg(target_os = "macos")]
  new_windows: Rc<RefCell<Vec<NewWindow>>>,
  permission_handler: Option<Box<dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync>>,
  #[cfg(target_os = "macos")]
  permission_request_handler:
    Option<Box<dyn Fn(PermissionRequest) -> PermissionRequestDisposition>>,
  #[cfg(target_os = "macos")]
  pending_permission_requests: RefCell<BoundedPendingPermissionRequests<PendingPermissionRequest>>,
  #[cfg(target_os = "macos")]
  file_upload_handler: Option<Box<crate::file_upload::FileUploadHandler>>,
  #[cfg(target_os = "macos")]
  script_dialogs: super::super::script_dialog::ScriptDialogs,
}

#[cfg(target_os = "macos")]
impl Drop for WryWebViewUIDelegateIvars {
  fn drop(&mut self) {
    for pending in self.pending_permission_requests.get_mut().drain() {
      pending.decision_handler.call((WKPermissionDecision::Deny,));
    }
  }
}

define_class!(
  #[unsafe(super(NSObject))]
  #[thread_kind = MainThreadOnly]
  #[ivars = WryWebViewUIDelegateIvars]
  pub struct WryWebViewUIDelegate;

  unsafe impl NSObjectProtocol for WryWebViewUIDelegate {}

  unsafe impl WKUIDelegate for WryWebViewUIDelegate {
    #[cfg(target_os = "macos")]
    #[unsafe(method(webViewDidClose:))]
    fn page_requested_close(&self, _webview: &WryWebView) {
      if let Some(handler) = &self.ivars().page_close_handler {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(handler));
      }
    }
    #[cfg(target_os = "macos")]
    #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
    fn run_javascript_alert(
      &self,
      webview: &WryWebView,
      message: &objc2_foundation::NSString,
      frame: &WKFrameInfo,
      completion_handler: &Block<dyn Fn()>,
    ) {
      let completion = completion_handler.copy();
      self.ivars().script_dialogs.present(
        webview,
        frame,
        super::super::script_dialog::Kind::Alert,
        message,
        Box::new(move |_| completion.call(())),
      );
    }

    #[cfg(target_os = "macos")]
    #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
    fn run_javascript_confirm(
      &self,
      webview: &WryWebView,
      message: &objc2_foundation::NSString,
      frame: &WKFrameInfo,
      completion_handler: &Block<dyn Fn(Bool)>,
    ) {
      let completion = completion_handler.copy();
      self.ivars().script_dialogs.present(
        webview,
        frame,
        super::super::script_dialog::Kind::Confirm,
        message,
        Box::new(move |answer| {
          let accepted = matches!(answer, super::super::script_dialog::Answer::Accepted(_));
          completion.call((Bool::new(accepted),));
        }),
      );
    }

    #[cfg(target_os = "macos")]
    #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
    fn run_javascript_prompt(
      &self,
      webview: &WryWebView,
      prompt: &objc2_foundation::NSString,
      default_text: Option<&objc2_foundation::NSString>,
      frame: &WKFrameInfo,
      completion_handler: &Block<dyn Fn(*mut objc2_foundation::NSString)>,
    ) {
      let completion = completion_handler.copy();
      let default = default_text.map(|text| text.to_string()).unwrap_or_default();
      self.ivars().script_dialogs.present(
        webview,
        frame,
        super::super::script_dialog::Kind::Prompt(default),
        prompt,
        Box::new(move |answer| match answer {
          super::super::script_dialog::Answer::Accepted(Some(text)) => {
            let text = objc2_foundation::NSString::from_str(&text);
            completion.call((Retained::as_ptr(&text).cast_mut(),));
          }
          _ => completion.call((null_mut(),)),
        }),
      );
    }

    #[cfg(target_os = "macos")]
    #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
    fn run_file_upload_panel(
      &self,
      webview: &WryWebView,
      parameters: &WKOpenPanelParameters,
      frame: &WKFrameInfo,
      handler: &block2::Block<dyn Fn(*const NSArray<NSURL>)>,
    ) {
      let Some(broker) = &self.ivars().file_upload_handler else {
        handler.call((std::ptr::null(),));
        return;
      };
      let native_origin = unsafe { frame.securityOrigin() };
      let Some(origin) = Self::permission_origin(&native_origin) else {
        handler.call((std::ptr::null(),));
        return;
      };
      let request = crate::FileUploadRequest {
        origin,
        allows_multiple_selection: unsafe { parameters.allowsMultipleSelection() },
        allows_directories: unsafe { parameters.allowsDirectories() },
      };
      let completion = handler.copy();
      let responder = crate::FileUploadResponder::new(move |urls| {
        completion.call((urls.map_or(std::ptr::null(), |urls| urls as *const _),));
      });
      // Unwinding drops the responder and denies once. The native block and
      // selected URLs remain on the owning main thread throughout.
      let _ = catch_unwind(AssertUnwindSafe(|| broker(webview, request, responder)));
    }

    #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
    fn request_media_capture_permission(
      &self,
      _webview: &WryWebView,
      origin: &WKSecurityOrigin,
      _frame: &WKFrameInfo,
      capture_type: WKMediaCaptureType,
      decision_handler: &Block<dyn Fn(WKPermissionDecision)>,
    ) {
      #[cfg(target_os = "macos")]
      {
        let request_kind = match capture_type {
          WKMediaCaptureType::Camera => PermissionRequestKind::Single(PermissionKind::Camera),
          WKMediaCaptureType::Microphone => {
            PermissionRequestKind::Single(PermissionKind::Microphone)
          }
          WKMediaCaptureType::CameraAndMicrophone => PermissionRequestKind::CameraAndMicrophone,
          _ => PermissionRequestKind::Single(PermissionKind::Other),
        };
        if self.broker_permission_request(origin, request_kind, decision_handler) {
          return;
        }
      }

      // Call user's permission handler if set
      let decision = if let Some(handler) = &self.ivars().permission_handler {
        match capture_type {
          WKMediaCaptureType::Camera => permission_decision(handler(PermissionKind::Camera)),
          WKMediaCaptureType::Microphone => {
            permission_decision(handler(PermissionKind::Microphone))
          }
          WKMediaCaptureType::CameraAndMicrophone => {
            let mic_res = handler(PermissionKind::Microphone);
            let cam_res = handler(PermissionKind::Camera);

            match (mic_res, cam_res) {
              (PermissionResponse::Allow, PermissionResponse::Allow) => WKPermissionDecision::Grant,
              (PermissionResponse::Deny, _) | (_, PermissionResponse::Deny) => {
                WKPermissionDecision::Deny
              }
              _ => WKPermissionDecision::Deny,
            }
          }
          _ => permission_decision(handler(PermissionKind::Other)),
        }
      } else {
        WKPermissionDecision::Deny
      };

      (*decision_handler).call((decision,));
    }

    #[cfg(target_os = "macos")]
    #[unsafe(method(webView:requestDeviceOrientationAndMotionPermissionForOrigin:initiatedByFrame:decisionHandler:))]
    fn request_device_orientation_and_motion_permission(
      &self,
      _webview: &WryWebView,
      _origin: &WKSecurityOrigin,
      _frame: &WKFrameInfo,
      decision_handler: &Block<dyn Fn(WKPermissionDecision)>,
    ) {
      // WebKit's default for an omitted delegate method is `Prompt`. Route the
      // request through the same construction-time policy as every other raw
      // capability, and keep missing handlers and `Default` fail-closed.
      let decision = self
        .ivars()
        .permission_handler
        .as_ref()
        .map(|handler| permission_decision(handler(PermissionKind::Sensors)))
        .unwrap_or(WKPermissionDecision::Deny);

      // The native completion block is invoked exactly once on every path.
      decision_handler.call((decision,));
    }

    #[cfg(target_os = "macos")]
    #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
    unsafe fn create_web_view_for_navigation_action(
      &self,
      webview: &WryWebView,
      configuration: &objc2_web_kit::WKWebViewConfiguration,
      action: &objc2_web_kit::WKNavigationAction,
      window_features: &objc2_web_kit::WKWindowFeatures,
    ) -> Option<Retained<objc2_web_kit::WKWebView>> {
      // `define_class!` wraps `method_id` return values for Objective-C
      // ownership conventions. Keep fail-closed early returns inside a Rust
      // closure so they return `Option`, not the macro's retained ABI wrapper.
      (|| -> Option<Retained<objc2_web_kit::WKWebView>> {
        if let Some(new_window_req_handler) = &self.ivars().new_window_req_handler {
          let request = action.request();
          let url = request
            .URL()
            .and_then(|url| url.absoluteString())
            .and_then(|url| bounded_nsstring(&url, PAGE_URL_LIMIT))?;

          let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            new_window_req_handler(
              url,
              NewWindowFeatures {
                // The per-view WKPreferences gate denies automatic script popups.
                user_initiated: true,
                foreground: {
                  let flags = action.modifierFlags();
                  let background = flags.contains(objc2_app_kit::NSEventModifierFlags::Command)
                    || action.buttonNumber() == 2;
                  !background || flags.contains(objc2_app_kit::NSEventModifierFlags::Shift)
                },
                size: if let (Some(width), Some(height)) =
                  (window_features.width(), window_features.height())
                {
                  Some(dpi::LogicalSize::new(
                    width.doubleValue(),
                    height.doubleValue(),
                  ))
                } else {
                  None
                },
                position: if let (Some(x), Some(y)) = (window_features.x(), window_features.y()) {
                  Some(dpi::LogicalPosition::new(x.doubleValue(), y.doubleValue()))
                } else {
                  None
                },
                opener: crate::NewWindowOpener {
                  webview: webview.into(),
                  target_configuration: configuration.into(),
                },
              },
            )
          }))
          .unwrap_or(NewWindowResponse::Deny);
          match response {
            NewWindowResponse::Allow => {
              let mtm = MainThreadMarker::new()?;
              let current_window = webview.window()?;
              let screen = current_window.screen()?;
              let screen_frame = screen.frame();

              let defaults = current_window.frame();
              let size = objc2_foundation::NSSize::new(
                window_features
                  .width()
                  .map_or(defaults.size.width, |width| width.doubleValue()),
                window_features
                  .height()
                  .map_or(defaults.size.height, |height| height.doubleValue()),
              );
              let position = objc2_foundation::NSPoint::new(
                window_features
                  .x()
                  .map_or(defaults.origin.x, |x| x.doubleValue()),
                window_features.y().map_or(defaults.origin.y, |y| {
                  screen_frame.size.height - y.doubleValue() - size.height
                }),
              );
              let rect = objc2_foundation::NSRect::new(position, size);

              let mut flags = objc2_app_kit::NSWindowStyleMask::Titled
                | objc2_app_kit::NSWindowStyleMask::Closable
                | objc2_app_kit::NSWindowStyleMask::Miniaturizable;
              let resizable = window_features
                .allowsResizing()
                .map_or(true, |resizable| resizable.boolValue());
              if resizable {
                flags |= objc2_app_kit::NSWindowStyleMask::Resizable;
              }

              let window = objc2_app_kit::NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<objc2_app_kit::NSWindow>(),
                rect,
                flags,
                objc2_app_kit::NSBackingStoreType::Buffered,
                false,
              );

              // SAFETY: Disable auto-release when closing windows.
              // This is required when creating `NSWindow` outside a window
              // controller.
              window.setReleasedWhenClosed(false);

              let webview = objc2_web_kit::WKWebView::initWithFrame_configuration(
                mtm.alloc::<objc2_web_kit::WKWebView>(),
                window.frame(),
                configuration,
              );

              let new_windows = self.ivars().new_windows.clone();
              let window_id = Retained::as_ptr(&window) as usize;
              let delegate = WryNSWindowDelegate::new(
                mtm,
                Box::new(move || {
                  if let Ok(mut windows) = new_windows.try_borrow_mut() {
                    windows
                      .retain(|window| Retained::as_ptr(&window.ns_window) as usize != window_id);
                  }
                }),
              );
              window.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));

              window.setContentView(Some(&webview));
              window.makeKeyAndOrderFront(None);

              let Ok(mut new_windows) = self.ivars().new_windows.try_borrow_mut() else {
                window.close();
                return None;
              };
              new_windows.push(NewWindow {
                ns_window: window,
                webview: webview.clone(),
                delegate,
              });

              Some(webview)
            }
            NewWindowResponse::Create { webview } => Some(webview),
            NewWindowResponse::Deny => None,
          }
        } else {
          None
        }
      })()
    }
  }
);

impl WryWebViewUIDelegate {
  #[cfg(target_os = "macos")]
  fn next_permission_request_id() -> Option<PermissionRequestId> {
    let value = NEXT_PERMISSION_REQUEST_ID
      .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        current.checked_add(1)
      })
      .ok()?;
    NonZeroU64::new(value).map(PermissionRequestId::new)
  }

  #[cfg(target_os = "macos")]
  fn permission_origin(origin: &WKSecurityOrigin) -> Option<PermissionOrigin> {
    // SAFETY: WebKit supplied a live `WKSecurityOrigin` reference for this
    // delegate invocation. The generated accessors return retained immutable
    // values, which we bound before copying into Rust.
    let (native_scheme, native_host, raw_port) =
      unsafe { (origin.protocol(), origin.host(), origin.port()) };
    let scheme = bounded_nsstring(&native_scheme, PERMISSION_ORIGIN_COMPONENT_LIMIT)?;
    let host = bounded_nsstring(&native_host, PERMISSION_ORIGIN_COMPONENT_LIMIT)?;
    if scheme.is_empty() || host.is_empty() {
      return None;
    }
    let port = if raw_port == 0 {
      None
    } else {
      Some(u16::try_from(raw_port).ok()?)
    };
    Some(PermissionOrigin::new(scheme, host, port))
  }

  #[cfg(target_os = "macos")]
  fn broker_permission_request(
    &self,
    origin: &WKSecurityOrigin,
    kind: PermissionRequestKind,
    decision_handler: &Block<dyn Fn(WKPermissionDecision)>,
  ) -> bool {
    let Some(handler) = &self.ivars().permission_request_handler else {
      return false;
    };
    let Some(id) = Self::next_permission_request_id() else {
      decision_handler.call((WKPermissionDecision::Deny,));
      return true;
    };
    let Some(origin) = Self::permission_origin(origin) else {
      decision_handler.call((WKPermissionDecision::Deny,));
      return true;
    };
    let Ok(mut pending) = self.ivars().pending_permission_requests.try_borrow_mut() else {
      decision_handler.call((WKPermissionDecision::Deny,));
      return true;
    };
    if pending
      .insert(
        id,
        PendingPermissionRequest {
          decision_handler: decision_handler.copy(),
        },
      )
      .is_err()
    {
      drop(pending);
      decision_handler.call((WKPermissionDecision::Deny,));
      return true;
    }
    drop(pending);

    let disposition = catch_unwind(AssertUnwindSafe(|| {
      handler(PermissionRequest::new(id, origin, kind))
    }))
    .unwrap_or(PermissionRequestDisposition::Deny);
    match disposition {
      PermissionRequestDisposition::Allow => {
        self.resolve_permission_request(id, PermissionResponse::Allow);
      }
      PermissionRequestDisposition::Deny => {
        self.resolve_permission_request(id, PermissionResponse::Deny);
      }
      PermissionRequestDisposition::Defer => {}
    }
    true
  }

  #[cfg(target_os = "macos")]
  pub fn resolve_permission_request(
    &self,
    request: PermissionRequestId,
    response: PermissionResponse,
  ) -> bool {
    let Ok(mut requests) = self.ivars().pending_permission_requests.try_borrow_mut() else {
      return false;
    };
    let Some(pending) = requests.take(request) else {
      return false;
    };
    drop(requests);
    pending
      .decision_handler
      .call((permission_decision(response),));
    true
  }

  pub fn new(
    mtm: MainThreadMarker,
    #[cfg(target_os = "macos")] page_close_handler: Option<Box<dyn Fn()>>,
    new_window_req_handler: Option<
      std::rc::Rc<dyn Fn(String, NewWindowFeatures) -> NewWindowResponse>,
    >,
    permission_handler: Option<Box<dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync>>,
    #[cfg(target_os = "macos")] permission_request_handler: Option<
      Box<dyn Fn(PermissionRequest) -> PermissionRequestDisposition>,
    >,
    #[cfg(target_os = "macos")] file_upload_handler: Option<
      Box<crate::file_upload::FileUploadHandler>,
    >,
  ) -> Retained<Self> {
    #[cfg(target_os = "ios")]
    let _new_window_req_handler = new_window_req_handler;

    let delegate = mtm
      .alloc::<WryWebViewUIDelegate>()
      .set_ivars(WryWebViewUIDelegateIvars {
        #[cfg(target_os = "macos")]
        page_close_handler,
        #[cfg(target_os = "macos")]
        new_window_req_handler,
        #[cfg(target_os = "macos")]
        new_windows: Rc::new(RefCell::new(vec![])),
        permission_handler,
        #[cfg(target_os = "macos")]
        permission_request_handler,
        #[cfg(target_os = "macos")]
        pending_permission_requests: RefCell::new(BoundedPendingPermissionRequests::new()),
        #[cfg(target_os = "macos")]
        file_upload_handler,
        #[cfg(target_os = "macos")]
        script_dialogs: Default::default(),
      });
    unsafe { msg_send![super(delegate), init] }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn native_permission_default_is_fail_closed() {
    assert_eq!(
      permission_decision(PermissionResponse::Default),
      WKPermissionDecision::Deny
    );
    assert_eq!(
      permission_decision(PermissionResponse::Deny),
      WKPermissionDecision::Deny
    );
    assert_eq!(
      permission_decision(PermissionResponse::Allow),
      WKPermissionDecision::Grant
    );
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn deferred_permission_registry_is_exact_bounded_and_reusable() {
    fn id(value: u64) -> PermissionRequestId {
      PermissionRequestId::new(NonZeroU64::new(value).unwrap())
    }

    let mut pending = BoundedPendingPermissionRequests::new();
    for value in 1..=MAX_PENDING_PERMISSION_REQUESTS as u64 {
      assert_eq!(pending.insert(id(value), value), Ok(()));
    }
    assert_eq!(pending.insert(id(1), 99), Err(99));
    assert_eq!(pending.insert(id(99), 100), Err(100));
    assert_eq!(pending.take(id(2)), Some(2));
    assert_eq!(pending.take(id(2)), None);
    assert_eq!(pending.insert(id(99), 99), Ok(()));
    let mut retained: Vec<_> = pending.drain().collect();
    retained.sort_unstable();
    assert_eq!(retained, vec![1, 3, 4, 99]);
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn permission_origin_remains_structured_and_request_identity_is_opaque() {
    let origin = PermissionOrigin::new("https".into(), "example.com".into(), Some(8443));
    let request = PermissionRequest::new(
      PermissionRequestId::new(NonZeroU64::new(7).unwrap()),
      origin,
      PermissionRequestKind::CameraAndMicrophone,
    );
    assert_eq!(request.id().get(), 7);
    assert_eq!(request.origin().scheme(), "https");
    assert_eq!(request.origin().host(), "example.com");
    assert_eq!(request.origin().port(), Some(8443));
    assert_eq!(request.kind(), PermissionRequestKind::CameraAndMicrophone);
  }
}
