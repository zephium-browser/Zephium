// Copyright 2020-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

mod download;
#[cfg(target_os = "macos")]
mod drag_drop;
mod navigation;
#[cfg(target_os = "macos")]
mod script_dialog;
#[cfg(feature = "mac-proxy")]
mod proxy;
#[cfg(target_os = "macos")]
mod synthetic_mouse_events;
mod util;

#[cfg(target_os = "ios")]
mod ios;

mod class;
pub(crate) use class::wry_navigation_delegate::BlockedLoadCounter;
pub use class::wry_web_view::WryWebView;
#[cfg(target_os = "macos")]
use class::wry_web_view_parent::WryWebViewParent;
use class::{
  document_title_changed_observer::*,
  url_scheme_handler,
  wry_download_delegate::WryDownloadDelegate,
  wry_navigation_delegate::WryNavigationDelegate,
  wry_web_view::WryWebViewIvars,
  wry_web_view_delegate::{WryWebViewDelegate, IPC_MESSAGE_HANDLER_NAME},
  wry_web_view_ui_delegate::WryWebViewUIDelegate,
};

use dpi::{LogicalPosition, LogicalSize};
#[cfg(target_os = "macos")]
use objc2::runtime::Bool;
#[cfg(any(debug_assertions, feature = "devtools", feature = "mac-proxy"))]
use objc2::runtime::NSObject;
use objc2::{
  rc::Retained,
  runtime::{AnyObject, ProtocolObject},
  AllocAnyThread, DeclaredClass, MainThreadOnly, Message,
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
  NSApplication, NSAutoresizingMaskOptions, NSResponder, NSTitlebarSeparatorStyle, NSView,
};
#[cfg(target_os = "macos")]
use objc2_core_foundation::CGSize;
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{
  ns_string, MainThreadMarker, NSArray, NSBundle, NSDate, NSError, NSHTTPCookie,
  NSHTTPCookieDomain, NSHTTPCookieExpires, NSHTTPCookieMaximumAge, NSHTTPCookieName,
  NSHTTPCookiePath, NSHTTPCookiePropertyKey, NSHTTPCookieSecure, NSHTTPCookieValue,
  NSHTTPCookieVersion, NSJSONSerialization, NSMutableDictionary, NSMutableURLRequest, NSNumber,
  NSObjectNSKeyValueCoding, NSObjectProtocol, NSString, NSUTF8StringEncoding, NSURL, NSUUID,
};
#[cfg(target_os = "ios")]
use objc2_ui_kit::{UIScrollView, UIViewAutoresizing};

#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindow;
#[cfg(target_os = "ios")]
use objc2_ui_kit::UIView as NSView;
use once_cell::sync::Lazy;
// #[cfg(target_os = "ios")]
// use objc2_ui_kit::UIWindow as NSWindow;

#[cfg(target_os = "ios")]
use crate::wkwebview::ios::WKWebView::WKWebView;
#[cfg(target_os = "ios")]
use crate::wkwebview::util::operating_system_version;

#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;

use objc2_web_kit::{
  WKAudiovisualMediaTypes, WKInactiveSchedulingPolicy, WKURLSchemeHandler, WKUserContentController,
  WKUserScript, WKUserScriptInjectionTime, WKWebViewConfiguration, WKWebsiteDataStore,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(target_os = "macos")]
use std::cell::{Cell, UnsafeCell};
use std::{
  cell::RefCell,
  collections::HashMap,
  net::Ipv4Addr,
  panic::AssertUnwindSafe,
  ptr::NonNull,
  str::{self, FromStr},
  sync::{Arc, Mutex, RwLock},
  time::Duration,
};

#[cfg(feature = "mac-proxy")]
use crate::{
  proxy::ProxyConfig,
  wkwebview::proxy::{
    nw_endpoint_t, nw_proxy_config_create_http_connect, nw_proxy_config_create_socksv5,
  },
};

use crate::{
  native_bounds::{bounded_nsstring, PAGE_URL_LIMIT},
  BackgroundThrottlingPolicy, Error, Rect, RequestAsyncResponder, Result, WebViewAttributes, RGBA,
};
#[cfg(target_os = "macos")]
use crate::{PermissionRequestId, PermissionResponse};

use http::Request;

use crate::util::Counter;

static COUNTER: Counter = Counter::new();

static WEBVIEW_STATE: Lazy<RwLock<HashMap<String, WebViewState>>> = Lazy::new(Default::default);

struct WebViewState {
  pub protocol_ptrs:
    Vec<Arc<dyn Fn(crate::WebViewId, Request<Vec<u8>>, RequestAsyncResponder) + Send + Sync>>,
}

#[derive(Debug, Default, Copy, Clone)]
pub struct PrintMargin {
  pub top: f32,
  pub right: f32,
  pub bottom: f32,
  pub left: f32,
}

#[derive(Debug, Default, Clone)]
pub struct PrintOptions {
  pub margins: PrintMargin,
}

#[cfg(target_os = "macos")]
pub(crate) struct RetainedHostWindow {
  // WKWebView/AppKit access is main-thread-only. A raw interior slot avoids a
  // RefCell panic in the source-compatible infallible `ns_window()` API.
  // Replacement publishes the new retained value before releasing the old
  // one, so Objective-C deallocation reentrancy always observes valid state.
  value: UnsafeCell<Retained<NSWindow>>,
  updating: Cell<bool>,
}

#[cfg(target_os = "macos")]
struct HostWindowUpdate<'a>(&'a Cell<bool>);

#[cfg(target_os = "macos")]
impl Drop for HostWindowUpdate<'_> {
  fn drop(&mut self) {
    self.0.set(false);
  }
}

#[cfg(target_os = "macos")]
impl RetainedHostWindow {
  fn new(window: Retained<NSWindow>) -> Self {
    Self {
      value: UnsafeCell::new(window),
      updating: Cell::new(false),
    }
  }

  fn begin_update(&self) -> Option<HostWindowUpdate<'_>> {
    (!self.updating.replace(true)).then_some(HostWindowUpdate(&self.updating))
  }

  pub(crate) fn get(&self) -> Retained<NSWindow> {
    // SAFETY: `InnerWebView` is confined by its MainThreadMarker. Replacement
    // performs no callback-capable operation until after the slot contains
    // the new value, and cloning does not mutate the stored retain handle.
    unsafe { (&*self.value.get()).clone() }
  }

  fn replace(&self, window: Retained<NSWindow>) {
    // SAFETY: main-thread confinement excludes concurrent access. `replace`
    // completes the pointer-sized state transition before `old_window` is
    // released; a reentrant getter during deallocation sees `window`.
    let old_window = unsafe { self.value.get().replace(window) };
    drop(old_window);
  }
}

#[cfg(target_os = "macos")]
struct UnfocusedChildFocusSnapshot {
  app_active: bool,
  // Retain both authority owners across callback-capable AppKit operations.
  // Pointer-only snapshots could ABA-match if a window closed synchronously
  // and a replacement reused its address during `addSubview:`.
  key_window: Option<Retained<NSWindow>>,
  main_window: Option<Retained<NSWindow>>,
  responder: Option<Retained<NSResponder>>,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnfocusedChildFocusDecision {
  Preserved,
  RestoreOwned,
  Refuse,
}

#[cfg(target_os = "macos")]
const fn unfocused_child_focus_decision(
  authority_preserved: bool,
  responder_preserved: bool,
  current_responder_owned: bool,
) -> UnfocusedChildFocusDecision {
  if !authority_preserved {
    UnfocusedChildFocusDecision::Refuse
  } else if responder_preserved {
    UnfocusedChildFocusDecision::Preserved
  } else if current_responder_owned {
    UnfocusedChildFocusDecision::RestoreOwned
  } else {
    UnfocusedChildFocusDecision::Refuse
  }
}

#[cfg(target_os = "macos")]
fn window_identity(window: Option<&Retained<NSWindow>>) -> Option<usize> {
  window.map(|window| Retained::as_ptr(window) as usize)
}

#[cfg(target_os = "macos")]
fn responder_identity(responder: Option<&Retained<NSResponder>>) -> Option<usize> {
  responder.map(|responder| Retained::as_ptr(responder) as usize)
}

#[cfg(target_os = "macos")]
fn capture_unfocused_child_focus(
  app: &NSApplication,
  host_window: &NSWindow,
) -> UnfocusedChildFocusSnapshot {
  let key_window = app.keyWindow();
  let main_window = app.mainWindow();
  UnfocusedChildFocusSnapshot {
    app_active: app.isActive(),
    key_window,
    main_window,
    responder: host_window.firstResponder(),
  }
}

#[cfg(target_os = "macos")]
fn focus_authority_preserved(snapshot: &UnfocusedChildFocusSnapshot, app: &NSApplication) -> bool {
  let key_window = app.keyWindow();
  let main_window = app.mainWindow();
  app.isActive() == snapshot.app_active
    && window_identity(key_window.as_ref()) == window_identity(snapshot.key_window.as_ref())
    && window_identity(main_window.as_ref()) == window_identity(snapshot.main_window.as_ref())
}

#[cfg(target_os = "macos")]
fn responder_is_owned_by_webview(
  current: Option<&Retained<NSResponder>>,
  webview: &WryWebView,
) -> bool {
  let exact_webview = current.as_ref().is_some_and(|responder| {
    std::ptr::eq(
      Retained::as_ptr(responder).cast::<std::ffi::c_void>(),
      std::ptr::from_ref(webview).cast::<std::ffi::c_void>(),
    )
  });
  let owned_descendant = current
    .cloned()
    .and_then(|responder| responder.downcast::<NSView>().ok())
    .is_some_and(|view| view.isDescendantOf(webview));
  exact_webview || owned_descendant
}

#[cfg(target_os = "macos")]
fn preserve_unfocused_child_focus(
  snapshot: &UnfocusedChildFocusSnapshot,
  app: &NSApplication,
  host_window: &NSWindow,
  webview: &WryWebView,
) -> Result<()> {
  let current = host_window.firstResponder();
  let responder_preserved =
    responder_identity(current.as_ref()) == responder_identity(snapshot.responder.as_ref());
  let authority_preserved = focus_authority_preserved(snapshot, app);

  match unfocused_child_focus_decision(
    authority_preserved,
    responder_preserved,
    responder_is_owned_by_webview(current.as_ref(), webview),
  ) {
    UnfocusedChildFocusDecision::Preserved => Ok(()),
    UnfocusedChildFocusDecision::RestoreOwned => {
      if !host_window.makeFirstResponder(snapshot.responder.as_deref())
        || !focus_authority_preserved(snapshot, app)
        || responder_identity(host_window.firstResponder().as_ref())
          != responder_identity(snapshot.responder.as_ref())
      {
        return Err(Error::MacosFocusPreservationFailed);
      }
      Ok(())
    }
    UnfocusedChildFocusDecision::Refuse => Err(Error::MacosFocusPreservationFailed),
  }
}

#[cfg(target_os = "macos")]
fn rollback_unfocused_child_attachment(
  snapshot: &UnfocusedChildFocusSnapshot,
  app: &NSApplication,
  host_window: &NSWindow,
  webview: &WryWebView,
) {
  // Detach before the constructor starts dropping its retained WK subtree.
  // AppKit callbacks may change authority or install a foreign responder, so
  // restoration is attempted only from the exact authority we captured and
  // only while the failed child still owns first-responder state.
  webview.removeFromSuperview();
  if !focus_authority_preserved(snapshot, app) {
    return;
  }
  let current = host_window.firstResponder();
  if responder_identity(current.as_ref()) == responder_identity(snapshot.responder.as_ref()) {
    return;
  }
  if !responder_is_owned_by_webview(current.as_ref(), webview) {
    return;
  }
  if host_window.makeFirstResponder(snapshot.responder.as_deref()) {
    // Validation is intentionally value-only: the original constructor error
    // remains authoritative, and rollback must never attempt another mutation.
    let _restored = focus_authority_preserved(snapshot, app)
      && responder_identity(host_window.firstResponder().as_ref())
        == responder_identity(snapshot.responder.as_ref());
  }
}

pub(crate) struct InnerWebView {
  id: String,
  mtm: MainThreadMarker,
  pub webview: Retained<WryWebView>,
  pub manager: Retained<WKUserContentController>,
  data_store: Retained<WKWebsiteDataStore>,
  ns_view: Retained<NSView>,
  #[cfg(target_os = "macos")]
  // Successful construction proves that the host view belongs to a window.
  // Keep that owning reference explicitly instead of asking AppKit again and
  // unwrapping a value that may become `nil` during detach/teardown.
  pub(crate) ns_window: RetainedHostWindow,
  #[allow(dead_code)]
  is_child: bool,
  pending_scripts: Arc<Mutex<Option<Vec<String>>>>,
  // Note that if following functions signatures are changed in the future,
  // all functions pointer declarations in objc callbacks below all need to get updated.
  ipc_handler_delegate: Option<Retained<WryWebViewDelegate>>,
  #[allow(dead_code)]
  // We need this the keep the reference count
  document_title_changed_observer: Option<Retained<DocumentTitleChangedObserver>>,
  #[allow(dead_code)]
  // We need this the keep the reference count
  pub(crate) navigation_policy_delegate: Retained<WryNavigationDelegate>,
  #[allow(dead_code)]
  // We need this the keep the reference count
  download_delegate: Option<Retained<WryDownloadDelegate>>,
  #[allow(dead_code)]
  // We need this the keep the reference count
  ui_delegate: Retained<WryWebViewUIDelegate>,
  #[cfg(target_os = "macos")]
  // We need this to update the traffic light inset
  parent_view: Option<Retained<WryWebViewParent>>,
}

impl InnerWebView {
  #[cfg(target_os = "macos")]
  pub(crate) fn resolve_permission_request(
    &self,
    request: PermissionRequestId,
    response: PermissionResponse,
  ) -> bool {
    self
      .ui_delegate
      .resolve_permission_request(request, response)
  }

  pub fn new(
    window: &impl HasWindowHandle,
    attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
  ) -> Result<Self> {
    let ns_view = match window.window_handle()?.as_raw() {
      #[cfg(target_os = "macos")]
      RawWindowHandle::AppKit(w) => w.ns_view.as_ptr(),
      #[cfg(target_os = "ios")]
      RawWindowHandle::UiKit(w) => w.ui_view.as_ptr(),
      _ => return Err(Error::UnsupportedWindowHandle),
    };

    unsafe { Self::new_ns_view(&*(ns_view as *mut NSView), attributes, pl_attrs, false) }
  }

  pub fn new_as_child(
    window: &impl HasWindowHandle,
    attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
  ) -> Result<Self> {
    let ns_view = match window.window_handle()?.as_raw() {
      #[cfg(target_os = "macos")]
      RawWindowHandle::AppKit(w) => w.ns_view.as_ptr(),
      #[cfg(target_os = "ios")]
      RawWindowHandle::UiKit(w) => w.ui_view.as_ptr(),
      _ => return Err(Error::UnsupportedWindowHandle),
    };

    unsafe { Self::new_ns_view(&*(ns_view as *mut NSView), attributes, pl_attrs, true) }
  }

  fn new_ns_view(
    ns_view: &NSView,
    attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
    is_child: bool,
  ) -> Result<Self> {
    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;

    #[cfg(target_os = "macos")]
    let focuses_during_initial_construction = attributes.focuses_during_initial_construction();
    #[cfg(target_os = "macos")]
    let app = NSApplication::sharedApplication(mtm);

    let webview_id = attributes
      .id
      .map(|id| id.to_string())
      .unwrap_or_else(|| COUNTER.next().to_string());

    // Safety: objc runtime calls are unsafe
    unsafe {
      #[cfg(target_os = "macos")]
      let host_window = ns_view
        .window()
        .ok_or(Error::NativeObjectUnavailable("host NSWindow"))?;
      #[cfg(target_os = "macos")]
      let unfocused_child_focus = (is_child && !focuses_during_initial_construction)
        .then(|| capture_unfocused_child_focus(&app, &host_window));

      #[cfg(target_os = "macos")]
      let using_existing_config = pl_attrs.webview_configuration.is_some();
      #[cfg(target_os = "ios")]
      let using_existing_config = false;
      #[cfg(target_os = "macos")]
      let config = pl_attrs
        .webview_configuration
        .unwrap_or_else(|| WKWebViewConfiguration::new(mtm));
      #[cfg(target_os = "ios")]
      let config = WKWebViewConfiguration::new(mtm);

      // Incognito mode
      let (os_major_version, _, _) = util::operating_system_version();
      #[cfg(target_os = "macos")]
      let custom_data_store_available = os_major_version >= 14;
      #[cfg(target_os = "ios")]
      let custom_data_store_available = os_major_version >= 17;

      let data_store = if using_existing_config {
        config.websiteDataStore()
      } else {
        let data_store = match (
          attributes.incognito,
          custom_data_store_available,
          pl_attrs.data_store_identifier,
        ) {
          (true, _, _) => WKWebsiteDataStore::nonPersistentDataStore(mtm),
          // if data_store_identifier is given and custom data stores are available, use custom store
          (false, true, Some(data_store)) => {
            let identifier = NSUUID::from_bytes(data_store);
            // <https://developer.apple.com/documentation/webkit/wkwebsitedatastore/init(foridentifier:)>
            // Available: macOS 14+, iOS 17+
            WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm)
          }
          _ => WKWebsiteDataStore::defaultDataStore(mtm),
        };
        config.setWebsiteDataStore(&data_store);
        data_store
      };

      // Register Custom Protocols
      let mut protocol_ptrs = Vec::new();
      for (name, function) in attributes.custom_protocols {
        // <https://developer.apple.com/documentation/webkit/wkwebviewconfiguration/urlschemehandler(forurlscheme:)>
        // Available: macOS 10.13+, iOS 11+
        let already_registered = using_existing_config
          && config
            .urlSchemeHandlerForURLScheme(&NSString::from_str(&name))
            .is_some();

        if already_registered {
          #[cfg(feature = "tracing")]
          tracing::debug!("Custom protocol {} already registered", name);
          continue;
        }

        let url_scheme_handler_cls = url_scheme_handler::create(&name)?;
        let handler: *mut AnyObject = objc2::msg_send![url_scheme_handler_cls, new];
        // SAFETY: Objective-C `new` returns a +1 retain count. Wrapping it
        // releases the creator's ownership after WebKit retains the handler,
        // including on registration failure.
        let handler = Retained::from_raw(handler)
          .ok_or(Error::NativeObjectUnavailable("custom URL scheme handler"))?;
        let handler_ptr = Retained::as_ptr(&handler) as *mut AnyObject;
        let protocol_index = protocol_ptrs.len();
        protocol_ptrs.push(Arc::from(function));

        let handler_ref = &mut *handler_ptr;
        let ivar = handler_ref
          .class()
          .instance_variable(c"protocol_index")
          .ok_or(Error::NativeObjectUnavailable(
            "custom URL scheme protocol index",
          ))?;
        let ivar_delegate: &mut usize = ivar.load_mut(handler_ref);
        *ivar_delegate = protocol_index;

        let set_result = objc2::exception::catch(AssertUnwindSafe(|| {
          // <https://developer.apple.com/documentation/webkit/wkwebviewconfiguration/seturlschemehandler(_:forurlscheme:)>
          // Available: macOS 10.13+, iOS 11+
          config.setURLSchemeHandler_forURLScheme(
            Some(&*(handler_ptr.cast::<ProtocolObject<dyn WKURLSchemeHandler>>())),
            &NSString::from_str(&name),
          );
        }));
        if set_result.is_err() {
          return Err(Error::UrlSchemeRegisterError(name));
        }
      }

      let protocol_count = protocol_ptrs.len();
      WEBVIEW_STATE
        .write()
        .map_err(|_| Error::WebKitStatePoisoned("custom protocol registry"))?
        .insert(webview_id.clone(), WebViewState { protocol_ptrs });

      // WebView and manager
      let manager = config.userContentController();
      let webview = WryWebView::alloc(mtm).set_ivars(WryWebViewIvars {
        webview_id: webview_id.clone(),
        is_child,
        #[cfg(target_os = "macos")]
        drag_drop_handler: attributes.drag_drop_handler,
        #[cfg(target_os = "macos")]
        accept_first_mouse: Bool::new(attributes.accept_first_mouse),
        #[cfg(target_os = "macos")]
        context_menu_handler: pl_attrs.context_menu_handler,
        #[cfg(target_os = "ios")]
        input_accessory_view_builder: pl_attrs.input_accessory_view_builder,
        custom_protocol_task_ids: Default::default(),
        custom_protocol_admission: (0..protocol_count)
          .map(|_| {
            crate::native_admission::InFlightAdmission::new(
              crate::native_admission::CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT,
            )
          })
          .collect(),
      });

      let _preference = config.preferences();
      // WebKit admits script-created windows only during native user activation.
      _preference.setJavaScriptCanOpenWindowsAutomatically(false);
      let _yes = NSNumber::numberWithBool(true);
      let _no = NSNumber::numberWithBool(false);

      #[cfg(target_os = "ios")]
      {
        // <https://developer.apple.com/documentation/webkit/wkwebviewconfiguration/limitsnavigationstoappbounddomains>
        // Available: macOS 11+, iOS 14+
        if pl_attrs.limit_navigations_to_app_bound_domains && operating_system_version().0 >= 14 {
          config.setLimitsNavigationsToAppBoundDomains(true);
        }
      }
      #[cfg(feature = "mac-proxy")]
      if let Some(proxy_config) = attributes.proxy_config {
        let proxy_config = match proxy_config {
          ProxyConfig::Http(endpoint) => {
            let nw_endpoint = nw_endpoint_t::try_from(endpoint)?;
            nw_proxy_config_create_http_connect(nw_endpoint, std::ptr::null_mut())
          }
          ProxyConfig::Socks5(endpoint) => {
            let nw_endpoint = nw_endpoint_t::try_from(endpoint)?;
            nw_proxy_config_create_socksv5(nw_endpoint)
          }
        };

        let proxy_config = NonNull::new(proxy_config).ok_or(Error::ProxyEndpointCreationFailed)?;
        let proxies: Retained<NSArray<NSObject>> = NSArray::arrayWithObject(proxy_config.as_ref());
        data_store.setValue_forKey(Some(&proxies), ns_string!("proxyConfigurations"));
      }

      // NOTE: Private API — `allowsPictureInPictureMediaPlayback` is a private
      // KVC key on WKPreferences. This must be per-view: Cargo feature
      // unification means privileged Tauri chrome and untrusted browser views
      // share the same compiled Wry implementation.
      _preference.setValue_forKey(
        Some(&NSNumber::numberWithBool(
          attributes.picture_in_picture_enabled,
        )),
        ns_string!("allowsPictureInPictureMediaPlayback"),
      );

      if attributes.javascript_disabled {
        // <https://developer.apple.com/documentation/webkit/wkwebviewconfiguration/defaultwebpagepreferences>
        // Available: macOS 10.15+, iOS 13+
        let web_page_preferences = config.defaultWebpagePreferences();
        // <https://developer.apple.com/documentation/webkit/wkwebpagepreferences/allowscontentjavascript>
        // Available: macOS 10.15+, iOS 13+
        web_page_preferences.setAllowsContentJavaScript(false);
      }

      #[cfg(target_os = "ios")]
      config.setValue_forKey(Some(&_yes), ns_string!("allowsInlineMediaPlayback"));

      if attributes.autoplay {
        // <https://developer.apple.com/documentation/webkit/wkwebviewconfiguration/mediatypesrequiringuseractionforplayback>
        // Available: macOS 10.12+, iOS 10+
        config.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::None);
      }

      #[cfg(feature = "transparent")]
      if attributes.transparent || attributes.background_color.is_some() {
        let no = NSNumber::numberWithBool(false);
        #[cfg(target_os = "macos")]
        {
          let version = util::operating_system_version();
          if version.0 > 10 || (version.0 == 10 && version.1 >= 14) {
            // NOTE: Private API — `drawsBackground`.
            // Available: macOS 10.14+ (no public doc).
            config.setValue_forKey(Some(&no), ns_string!("drawsBackground"));
          }
        }
        #[cfg(target_os = "ios")]
        {
          // NOTE: Private API — `drawsBackground`.
          config.setValue_forKey(Some(&no), ns_string!("drawsBackground"));
        }
      }

      // <https://developer.apple.com/documentation/webkit/wkpreferences/iselementfullscreenenabled>
      // Available: macOS 12.3+. Set per view either way, so a view that must
      // not go fullscreen is not left on WebKit's or a sibling's default.
      #[cfg(target_os = "macos")]
      _preference.setElementFullscreenEnabled(attributes.fullscreen_enabled);
      #[cfg(all(feature = "fullscreen", target_os = "ios"))]
      // NOTE: Private API — `fullScreenEnabled` is a private KVC key on WKPreferences.
      _preference.setValue_forKey(
        Some(&NSNumber::numberWithBool(attributes.fullscreen_enabled)),
        ns_string!("fullScreenEnabled"),
      );

      #[cfg(target_os = "macos")]
      let webview = {
        let scale_factor = host_window.backingScaleFactor();
        let (x, y) = attributes
          .bounds
          .map(|b| b.position.to_logical::<f64>(scale_factor))
          .map(Into::into)
          .unwrap_or((0, 0));
        let (w, h) = if is_child {
          attributes
            .bounds
            .map(|b| b.size.to_logical::<u32>(scale_factor))
            .map(Into::into)
        } else {
          None
        }
        .unwrap_or_else(|| {
          if is_child {
            let frame = NSView::frame(ns_view);
            (frame.size.width as u32, frame.size.height as u32)
          } else {
            (0, 0)
          }
        });

        let frame = CGRect {
          origin: if is_child {
            window_position(ns_view, x, y, h as f64)
          } else {
            CGPoint::new(x as f64, (0 - y - h as i32) as f64)
          },
          size: CGSize::new(w as f64, h as f64),
        };
        let webview: Retained<WryWebView> =
          objc2::msg_send![super(webview), initWithFrame: frame, configuration: &**config];

        // Set the under-page background color for overscroll areas (public API, macOS 12+).
        // drawsBackground is already disabled on the config above, so the window background
        // shows through. This handles the color visible when scrolling past page bounds.
        if os_major_version >= 12 {
          if let Some((red, green, blue, alpha)) = attributes.background_color {
            let color = objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(
              red as f64 / 255.0,
              green as f64 / 255.0,
              blue as f64 / 255.0,
              alpha as f64 / 255.0,
            );
            // <https://developer.apple.com/documentation/webkit/wkwebview/underpagebackgroundcolor>
            // Available: macOS 12+, iOS 15+
            webview.setUnderPageBackgroundColor(Some(&color));
          }
        }

        webview
      };
      #[cfg(target_os = "ios")]
      let webview = {
        let frame = ns_view.frame();
        let webview: Retained<WryWebView> =
          objc2::msg_send![super(webview), initWithFrame: frame, configuration: &**config];
        if let Some((red, green, blue, alpha)) = attributes.background_color {
          // This is required first since the webview color is applied too late.
          webview.setOpaque(false);

          let color = objc2_ui_kit::UIColor::colorWithRed_green_blue_alpha(
            red as f64 / 255.0,
            green as f64 / 255.0,
            blue as f64 / 255.0,
            alpha as f64 / 255.0,
          );

          if !is_child {
            ns_view.setBackgroundColor(Some(&color));
          }
          // This has to be monitored as it may clash with isOpaque = true.
          // The webview background color may also applied too late so actually not that useful.
          webview.setBackgroundColor(Some(&color));
        }
        webview
      };

      // change background throttling policy if attributes.background_throttling is set
      // which works for iOS 17.0+,iPadOS 17.0+,Mac Catalyst 17.0+, macOS 14.0+, visionOS 1.0+
      #[cfg(any(target_os = "ios", target_os = "macos"))]
      {
        let is_supported_os = (cfg!(target_os = "ios") && os_major_version >= 17)
          || (cfg!(target_os = "macos") && os_major_version >= 14);

        if is_supported_os {
          if let Some(policy) = attributes.background_throttling {
            let policy_value = match policy {
              BackgroundThrottlingPolicy::Disabled => WKInactiveSchedulingPolicy::None.0,
              BackgroundThrottlingPolicy::Suspend => WKInactiveSchedulingPolicy::Suspend.0,
              BackgroundThrottlingPolicy::Throttle => WKInactiveSchedulingPolicy::Throttle.0,
            };

            if let Ok(policy_number) = policy_value.try_into() {
              // <https://developer.apple.com/documentation/webkit/wkpreferences/inactiveschedulingpolicy>
              // Available: macOS 14+, iOS 17+
              _preference.setValue_forKey(
                Some(&NSNumber::numberWithInt(policy_number)),
                ns_string!("inactiveSchedulingPolicy"),
              );
            }
          }
        }
      }

      #[cfg(target_os = "macos")]
      {
        if is_child {
          // fixed element
          webview.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        } else {
          // Auto-resize
          webview.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewHeightSizable
              | NSAutoresizingMaskOptions::ViewWidthSizable,
          );
        }

        webview.setAllowsBackForwardNavigationGestures(attributes.back_forward_navigation_gestures);

        // <https://developer.apple.com/documentation/webkit/wkpreferences/tabfocuseslinks>
        // Available: macOS 12+
        _preference.setValue_forKey(Some(&_yes), ns_string!("tabFocusesLinks"));
      }
      #[cfg(target_os = "ios")]
      {
        webview.setAutoresizingMask(
          UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight,
        );

        // disable scroll bounce by default
        // https://developer.apple.com/documentation/webkit/wkwebview/1614784-scrollview?language=objc
        // But not exist in objc2-web-kit
        let scroll_view: Retained<UIScrollView> = objc2::msg_send![&webview, scrollView];
        // let scroll_view: Retained<UIScrollView> = webview.ivars().scrollView; // FIXME: not test yet
        scroll_view.setBounces(false)
      }

      if !attributes.visible {
        webview.setHidden(true);
      }

      #[cfg(any(debug_assertions, feature = "devtools"))]
      if attributes.devtools {
        // <https://developer.apple.com/documentation/webkit/wkwebview/isinspectable>
        // Available: macOS 13.3+, iOS 16.4+
        let has_inspectable_property: bool =
          NSObject::respondsToSelector(&webview, objc2::sel!(setInspectable:));
        if has_inspectable_property {
          webview.setInspectable(true);
        }
        // NOTE: Private API — `developerExtrasEnabled` is a private KVC key on WKPreferences.
        // this cannot be on an `else` statement, it does not work on macOS :(
        let dev = ns_string!("developerExtrasEnabled");
        _preference.setValue_forKey(Some(&_yes), dev);
      }

      // Message handler
      let ipc_handler_delegate = if let Some(ipc_handler) = attributes.ipc_handler {
        let delegate = WryWebViewDelegate::new(manager.clone(), ipc_handler, mtm);
        Some(delegate)
      } else {
        None
      };

      // Document title changed handler
      let document_title_changed_observer =
        if let Some(handler) = attributes.document_title_changed_handler {
          let delegate = DocumentTitleChangedObserver::new(webview.clone(), handler);
          Some(delegate)
        } else {
          None
        };

      let pending_scripts = Arc::new(Mutex::new(Some(Vec::new())));
      let downloads_denied = attributes.download_policy.inspect_metadata(|| ()).is_none();
      #[cfg(target_os = "macos")]
      let native_downloads = pl_attrs.native_download_handler.is_some();
      #[cfg(not(target_os = "macos"))]
      let native_downloads = false;
      let has_download_handler =
        !downloads_denied && (native_downloads || attributes.download_started_handler.is_some());
      // Download handler
      let download_delegate = if !downloads_denied
        && (native_downloads
          || attributes.download_started_handler.is_some()
          || attributes.download_completed_handler.is_some())
      {
        let delegate = WryDownloadDelegate::new(
          attributes.download_started_handler,
          attributes.download_completed_handler,
          #[cfg(target_os = "macos")]
          pl_attrs.native_download_handler,
          mtm,
        );
        Some(delegate)
      } else {
        None
      };

      let new_window_req_handler: Option<
        std::rc::Rc<dyn Fn(String, crate::NewWindowFeatures) -> crate::NewWindowResponse>,
      > = attributes.new_window_req_handler.map(std::rc::Rc::from);
      let navigation_policy_delegate = WryNavigationDelegate::new(
        webview.clone(),
        pending_scripts.clone(),
        has_download_handler,
        #[cfg(target_os = "macos")]
        new_window_req_handler.clone(),
        attributes.navigation_handler,
        attributes.apple_navigation_action_handler,
        #[cfg(target_os = "macos")]
        attributes.main_frame_navigation_attempt_handler,
        download_delegate.clone(),
        attributes.on_page_load_handler,
        attributes.navigation_event_handler,
        attributes.navigation_failure_handler,
        attributes.navigation_presentation_guard,
        pl_attrs.on_web_content_process_terminate_handler,
        mtm,
      );

      let proto_navigation_policy_delegate = ProtocolObject::from_ref(&*navigation_policy_delegate);
      webview.setNavigationDelegate(Some(proto_navigation_policy_delegate));

      let ui_delegate: Retained<WryWebViewUIDelegate> = WryWebViewUIDelegate::new(
        mtm,
        #[cfg(target_os = "macos")]
        attributes.page_close_handler,
        new_window_req_handler,
        attributes.permission_handler,
        #[cfg(target_os = "macos")]
        pl_attrs.permission_request_handler,
        #[cfg(target_os = "macos")]
        pl_attrs.file_upload_handler,
      );
      let proto_ui_delegate = ProtocolObject::from_ref(&*ui_delegate);
      webview.setUIDelegate(Some(proto_ui_delegate));

      // ns window is required for the print operation
      #[cfg(target_os = "macos")]
      {
        // <https://developer.apple.com/documentation/appkit/nswindow/titlebarseparatorstyle>
        // Available: macOS 11+
        let can_set_titlebar_style =
          host_window.respondsToSelector(objc2::sel!(setTitlebarSeparatorStyle:));
        if can_set_titlebar_style {
          host_window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
        }
      }

      #[cfg_attr(target_os = "ios", allow(unused_mut))]
      let mut w = Self {
        id: webview_id,
        mtm,
        webview: webview.clone(),
        manager,
        ns_view: ns_view.retain(),
        #[cfg(target_os = "macos")]
        ns_window: RetainedHostWindow::new(host_window.clone()),
        data_store,
        pending_scripts,
        ipc_handler_delegate,
        document_title_changed_observer,
        navigation_policy_delegate,
        download_delegate,
        ui_delegate,
        is_child,
        #[cfg(target_os = "macos")]
        parent_view: None,
      };

      // Only expose the page-to-native object when a real handler exists.
      // A raw browser tab must not receive even an inert native-looking API.
      if w.ipc_handler_delegate.is_some() {
        w.init(
          r#"Object.defineProperty(window, 'ipc', {
  value: Object.freeze({postMessage: function(s) {
    if (typeof s === 'string' && s.length <= 65536) {
      window.webkit.messageHandlers.ipc.postMessage(s);
    }
  }})
});"#,
          true,
        );
      }
      for init_script in attributes.initialization_scripts {
        w.init(&init_script.script, init_script.for_main_frame_only);
      }

      // Set user agent
      if let Some(user_agent) = attributes.user_agent {
        w.set_user_agent(user_agent.as_str())
      }

      // Navigation
      if let Some(url) = attributes.url {
        w.navigate_to_url(url.as_str(), attributes.headers)?;
      } else if let Some(html) = attributes.html {
        w.navigate_to_string(&html);
      }

      // Allow Link Preview
      w.webview.setAllowsLinkPreview(pl_attrs.allow_link_preview);

      // Inject the web view into the window as main content
      #[cfg(target_os = "macos")]
      {
        if is_child {
          ns_view.addSubview(&webview);
          if let Some(snapshot) = &unfocused_child_focus {
            if let Err(error) =
              preserve_unfocused_child_focus(snapshot, &app, &host_window, &webview)
            {
              rollback_unfocused_child_attachment(snapshot, &app, &host_window, &webview);
              return Err(error);
            }
          }
        } else {
          let parent_view = WryWebViewParent::new(mtm);

          if let Some(position) = pl_attrs.traffic_light_inset {
            parent_view.set_traffic_light_inset(&host_window, position);
          }

          parent_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewHeightSizable
              | NSAutoresizingMaskOptions::ViewWidthSizable,
          );
          parent_view.addSubview(&webview);

          // Tell the webview receive keyboard events in the window.
          // See https://github.com/tauri-apps/wry/issues/739
          host_window.setContentView(Some(&parent_view));
          host_window.makeFirstResponder(Some(&webview));

          w.parent_view = Some(parent_view);
        }

        // Construction declared as unfocused must not activate the app. In
        // particular, a hidden/background child cannot request global focus as
        // a constructor side effect.
        if focuses_during_initial_construction {
          if os_major_version >= 14 {
            // <https://developer.apple.com/documentation/appkit/nsapplication/activate()>
            // Available: macOS 14+
            NSApplication::activate(&app);
          } else {
            #[allow(deprecated)]
            NSApplication::activateIgnoringOtherApps(&app, true);
          }
        }
      }

      #[cfg(target_os = "ios")]
      {
        ns_view.addSubview(&webview);
      }

      Ok(w)
    }
  }

  pub fn id(&self) -> crate::WebViewId<'_> {
    &self.id
  }

  pub fn url(&self) -> crate::Result<String> {
    url_from_webview(&self.webview)
  }

  pub fn document_title(&self) -> crate::Result<Option<String>> {
    Ok(unsafe {
      self
        .webview
        .title()
        .and_then(|title| bounded_nsstring(&title, crate::native_bounds::PAGE_TITLE_LIMIT))
    })
  }

  pub fn eval(&self, js: &str, callback: Option<impl Fn(String) + Send + 'static>) -> Result<()> {
    let mut pending_scripts = self
      .pending_scripts
      .lock()
      .map_err(|_| Error::WebKitStatePoisoned("pending initialization scripts"))?;
    if let Some(scripts) = &mut *pending_scripts {
      scripts.push(js.into());
      return Ok(());
    }
    drop(pending_scripts);

    // Safety: objc runtime calls are unsafe
    unsafe {
      #[cfg(feature = "tracing")]
      let span = Mutex::new(Some(tracing::debug_span!("wry::eval").entered()));

      // we need to check if the callback exists outside the handler otherwise it's a segfault
      if let Some(callback) = callback {
        let handler = block2::RcBlock::new(move |val: *mut AnyObject, _err: *mut NSError| {
          #[cfg(feature = "tracing")]
          if let Ok(mut span) = span.lock() {
            span.take();
          }

          let mut result = String::new();

          if !val.is_null() {
            let json_ns_data = NSJSONSerialization::dataWithJSONObject_options_error(
              &*val,
              objc2_foundation::NSJSONWritingOptions::FragmentsAllowed,
            );
            if let Ok(json_ns_data) = json_ns_data {
              let json_string = NSString::alloc();
              if let Some(json_string) =
                NSString::initWithData_encoding(json_string, &json_ns_data, NSUTF8StringEncoding)
              {
                result = json_string.to_string();
              }
            }
          }

          callback(result);
        });

        self
          .webview
          .evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&handler));
      } else {
        #[cfg(feature = "tracing")]
        let handler = Some(block2::RcBlock::new(
          move |_val: *mut AnyObject, _err: *mut NSError| {
            if let Ok(mut span) = span.lock() {
              span.take();
            }
          },
        ));
        #[cfg(not(feature = "tracing"))]
        let handler: Option<block2::RcBlock<dyn Fn(*mut AnyObject, *mut NSError)>> = None;

        self
          .webview
          .evaluateJavaScript_completionHandler(&NSString::from_str(js), handler.as_deref());
      }
    }

    Ok(())
  }

  fn init(&self, js: &str, for_main_only: bool) {
    // Safety: objc runtime calls are unsafe
    unsafe {
      let userscript = WKUserScript::alloc(self.mtm);
      let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
        userscript,
        &NSString::from_str(js),
        WKUserScriptInjectionTime::AtDocumentStart,
        for_main_only,
      );
      self.manager.addUserScript(&script);
    }
  }

  pub fn load_url(&self, url: &str) -> crate::Result<()> {
    self.navigate_to_url(url, None)
  }

  pub fn load_url_with_headers(&self, url: &str, headers: http::HeaderMap) -> crate::Result<()> {
    self.navigate_to_url(url, Some(headers))
  }

  pub fn load_html(&self, html: &str) -> crate::Result<()> {
    self.navigate_to_string(html);
    Ok(())
  }

  /// Reloads the current page.
  pub fn reload(&self) -> crate::Result<()> {
    // Safety: objc runtime calls are unsafe
    unsafe { self.webview.reload() }
      .map(|_| ())
      .ok_or(crate::Error::NativeObjectUnavailable("WKWebView.reload"))
  }

  pub fn go_forward(&self) -> Result<()> {
    unsafe { self.webview.goForward() }
      .map(|_| ())
      .ok_or(crate::Error::NativeObjectUnavailable("WKWebView.goForward"))
  }

  pub fn go_back(&self) -> Result<()> {
    unsafe { self.webview.goBack() }
      .map(|_| ())
      .ok_or(crate::Error::NativeObjectUnavailable("WKWebView.goBack"))
  }

  pub fn can_go_forward(&self) -> Result<bool> {
    Ok(unsafe { self.webview.canGoForward() })
  }

  pub fn can_go_back(&self) -> Result<bool> {
    Ok(unsafe { self.webview.canGoBack() })
  }

  pub fn clear_all_browsing_data(&self) -> Result<()> {
    unsafe {
      let config = self.webview.configuration();
      let store = config.websiteDataStore();
      let all_data_types = WKWebsiteDataStore::allWebsiteDataTypes(self.mtm);
      let date = NSDate::dateWithTimeIntervalSince1970(0.0);
      let handler = block2::RcBlock::new(|| {});
      store.removeDataOfTypes_modifiedSince_completionHandler(&all_data_types, &date, &handler);
    }
    Ok(())
  }

  fn navigate_to_url(&self, url: &str, headers: Option<http::HeaderMap>) -> crate::Result<()> {
    if url.len() > PAGE_URL_LIMIT.max_utf8_bytes
      || url.encode_utf16().count() > PAGE_URL_LIMIT.max_utf16_units
    {
      return Err(Error::WebKitNavigationIdentityUnavailable);
    }
    // Safety: objc runtime calls are unsafe
    unsafe {
      let url = NSURL::URLWithString(&NSString::from_str(url))
        .ok_or(Error::NativeObjectUnavailable("navigation URL"))?;
      let absolute_url = url
        .absoluteString()
        .and_then(|url| bounded_nsstring(&url, PAGE_URL_LIMIT))
        .ok_or(Error::WebKitNavigationIdentityUnavailable)?;
      let request = NSMutableURLRequest::requestWithURL(&url);
      if let Some(headers) = headers {
        for (name, value) in headers.iter() {
          let key = NSString::from_str(name.as_str());
          let value = NSString::from_str(value.to_str().unwrap_or_default());
          request.addValue_forHTTPHeaderField(&value, &key);
        }
      }
      if !self
        .navigation_policy_delegate
        .begin_programmatic_navigation()
      {
        return Err(Error::WebKitNavigationIdentityUnavailable);
      }
      let Some(navigation) = self.webview.loadRequest(&request) else {
        self
          .navigation_policy_delegate
          .cancel_programmatic_navigation();
        return Err(Error::WebKitNavigationIdentityUnavailable);
      };
      if !self
        .navigation_policy_delegate
        .register_programmatic_navigation(&navigation, absolute_url)
      {
        self.webview.stopLoading();
        return Err(Error::WebKitNavigationIdentityUnavailable);
      }
    }

    Ok(())
  }

  fn navigate_to_string(&self, html: &str) {
    // Safety: objc runtime calls are unsafe
    unsafe {
      self
        .webview
        .loadHTMLString_baseURL(&NSString::from_str(html), None);
    }
  }

  fn set_user_agent(&self, user_agent: &str) {
    unsafe {
      self
        .webview
        .setCustomUserAgent(Some(&NSString::from_str(user_agent)));
    }
  }

  pub fn print(&self) -> crate::Result<()> {
    self.print_with_options(&PrintOptions::default())
  }

  pub fn print_with_options(&self, _options: &PrintOptions) -> crate::Result<()> {
    // Safety: objc runtime calls are unsafe
    #[cfg(target_os = "macos")]
    unsafe {
      // <https://developer.apple.com/documentation/webkit/wkwebview/printoperation(with:)>
      // Available: macOS 11+
      let can_print = self
        .webview
        .respondsToSelector(objc2::sel!(printOperationWithPrintInfo:));
      if can_print {
        // Create a shared print info
        let print_info = objc2_app_kit::NSPrintInfo::sharedPrintInfo();
        // let print_info: id = msg_send![print_info, init];
        print_info.setTopMargin(_options.margins.top.into());
        print_info.setRightMargin(_options.margins.right.into());
        print_info.setBottomMargin(_options.margins.bottom.into());
        print_info.setLeftMargin(_options.margins.left.into());

        // Create new print operation from the webview content
        let print_operation = self.webview.printOperationWithPrintInfo(&print_info);

        // Allow the modal to detach from the current thread and be non-blocker
        print_operation.setCanSpawnSeparateThread(true);

        // Launch the modal
        let window = self
          .webview
          .window()
          .ok_or(Error::NativeObjectUnavailable("print host NSWindow"))?;
        print_operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
          &window,
          None,
          None,
          std::ptr::null_mut(),
        )
      }
    }

    Ok(())
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn open_devtools(&self) {
    #[cfg(target_os = "macos")]
    unsafe {
      // taken from <https://github.com/WebKit/WebKit/blob/784f93cb80a386c29186c510bba910b67ce3adc1/Source/WebKit/UIProcess/API/Cocoa/WKWebView.mm#L1939>
      let tool: Retained<AnyObject> = objc2::msg_send![&self.webview, _inspector];
      let () = objc2::msg_send![&tool, show];
    }
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn close_devtools(&self) {
    #[cfg(target_os = "macos")]
    unsafe {
      // taken from <https://github.com/WebKit/WebKit/blob/784f93cb80a386c29186c510bba910b67ce3adc1/Source/WebKit/UIProcess/API/Cocoa/WKWebView.mm#L1939>
      let tool: Retained<AnyObject> = objc2::msg_send![&self.webview, _inspector];
      let () = objc2::msg_send![&tool, close];
    }
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn is_devtools_open(&self) -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
      // taken from <https://github.com/WebKit/WebKit/blob/784f93cb80a386c29186c510bba910b67ce3adc1/Source/WebKit/UIProcess/API/Cocoa/WKWebView.mm#L1939>
      let tool: Retained<AnyObject> = objc2::msg_send![&self.webview, _inspector];
      let is_visible: bool = objc2::msg_send![&tool, isVisible];
      is_visible
    }
    #[cfg(not(target_os = "macos"))]
    false
  }

  pub fn zoom(&self, scale_factor: f64) -> crate::Result<()> {
    unsafe {
      // <https://developer.apple.com/documentation/webkit/wkwebview/pagezoom>
      // Available: macOS 11+, iOS 14+
      self.webview.setPageZoom(scale_factor);
    }

    Ok(())
  }

  pub fn set_background_color(&self, _background_color: RGBA) -> Result<()> {
    #[cfg(target_os = "ios")]
    unsafe {
      let (red, green, blue, alpha) = _background_color;

      let color = objc2_ui_kit::UIColor::colorWithRed_green_blue_alpha(
        red as f64 / 255.0,
        green as f64 / 255.0,
        blue as f64 / 255.0,
        alpha as f64 / 255.0,
      );

      if !self.is_child {
        self.ns_view.setBackgroundColor(Some(&color));
      }
      // This has to be monitored as it may clash with isOpaque = true.
      // The webview background color may also applied too late so actually not that useful.
      self.webview.setBackgroundColor(Some(&color));
    }

    #[cfg(all(target_os = "macos", feature = "transparent"))]
    unsafe {
      let (red, green, blue, alpha) = _background_color;

      // Disable the default white background using the same drawsBackground KVC key
      // as the `transparent` feature. On the webview instance (vs config) for runtime changes.
      // NOTE: Private API — `drawsBackground` is a private KVC key on WKWebView instance.
      let no = NSNumber::numberWithBool(false);
      self
        .webview
        .setValue_forKey(Some(&no), ns_string!("drawsBackground"));

      let (os_major_version, _, _) = util::operating_system_version();
      if os_major_version >= 12 {
        let color = objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(
          red as f64 / 255.0,
          green as f64 / 255.0,
          blue as f64 / 255.0,
          alpha as f64 / 255.0,
        );
        // <https://developer.apple.com/documentation/webkit/wkwebview/underpagebackgroundcolor>
        // Available: macOS 12+, iOS 15+
        self.webview.setUnderPageBackgroundColor(Some(&color));
      }
    }

    Ok(())
  }

  pub fn bounds(&self) -> crate::Result<Rect> {
    #[allow(unused_unsafe)]
    unsafe {
      let parent = self
        .webview
        .superview()
        .ok_or(Error::NativeObjectUnavailable("WebView superview"))?;
      let parent_frame = parent.frame();
      let webview_frame = self.webview.frame();

      Ok(Rect {
        position: LogicalPosition::new(
          webview_frame.origin.x,
          parent_frame.size.height - webview_frame.origin.y - webview_frame.size.height,
        )
        .into(),
        size: LogicalSize::new(webview_frame.size.width, webview_frame.size.height).into(),
      })
    }
  }

  pub fn set_bounds(&self, #[allow(unused)] bounds: Rect) -> crate::Result<()> {
    #[cfg(target_os = "macos")]
    if self.is_child {
      let window = self
        .webview
        .window()
        .ok_or(Error::NativeObjectUnavailable("WebView NSWindow"))?;
      let scale_factor = window.backingScaleFactor();
      let (x, y) = bounds.position.to_logical::<f64>(scale_factor).into();
      let (width, height) = bounds.size.to_logical::<i32>(scale_factor).into();

      unsafe {
        let parent_view = self
          .webview
          .superview()
          .ok_or(Error::NativeObjectUnavailable("WebView superview"))?;
        let frame = CGRect {
          origin: window_position(&parent_view, x, y, height),
          size: CGSize::new(width, height),
        };
        self.webview.setFrame(frame);
      }
    }

    Ok(())
  }

  pub fn set_visible(&self, visible: bool) -> Result<()> {
    self.webview.setHidden(!visible);
    Ok(())
  }

  pub fn focus(&self) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
      let window = self
        .webview
        .window()
        .ok_or(Error::NativeObjectUnavailable("WebView NSWindow"))?;
      window.makeFirstResponder(Some(&self.webview));
    }
    Ok(())
  }

  pub fn focus_parent(&self) -> Result<()> {
    if let Some(window) = self.webview.window() {
      #[cfg(target_os = "macos")]
      window.makeFirstResponder(Some(&self.ns_view));
      #[cfg(target_os = "ios")]
      unsafe {
        window.becomeFirstResponder()
      };
    }

    Ok(())
  }

  unsafe fn cookie_from_wkwebview(cookie: &NSHTTPCookie) -> cookie::Cookie<'static> {
    let name = cookie.name().to_string();
    let value = cookie.value().to_string();

    let mut cookie_builder = cookie::CookieBuilder::new(name, value);

    let domain = cookie.domain().to_string();
    cookie_builder = cookie_builder.domain(domain);

    let path = cookie.path().to_string();
    cookie_builder = cookie_builder.path(path);

    let http_only = cookie.isHTTPOnly();
    cookie_builder = cookie_builder.http_only(http_only);

    let secure = cookie.isSecure();
    cookie_builder = cookie_builder.secure(secure);

    // Using string comparison because of https://github.com/tauri-apps/wry/issues/1616
    let (major, minor, _) = util::operating_system_version();
    if major > 10 || (major == 10 && minor >= 15) {
      // <https://developer.apple.com/documentation/foundation/httpcookie/samesitepolicy>
      // Available: macOS 10.15+, iOS 13+
      let same_site = cookie.sameSitePolicy();
      let same_site = match same_site {
        Some(policy) if policy.to_string() == "lax" => cookie::SameSite::Lax,
        Some(policy) if policy.to_string() == "strict" => cookie::SameSite::Strict,
        _ => cookie::SameSite::None,
      };
      cookie_builder = cookie_builder.same_site(same_site);
    }

    let expires = cookie.expiresDate();
    let expires = match expires {
      Some(datetime) => {
        cookie::time::OffsetDateTime::from_unix_timestamp(datetime.timeIntervalSince1970() as i64)
          .ok()
          .map(cookie::Expiration::DateTime)
      }
      None => Some(cookie::Expiration::Session),
    };
    if let Some(expires) = expires {
      cookie_builder = cookie_builder.expires(expires);
    }

    cookie_builder.build()
  }

  unsafe fn cookie_into_wkwebview(cookie: &cookie::Cookie<'_>) -> Result<Retained<NSHTTPCookie>> {
    let nstring_true: &'static NSString = ns_string!("TRUE");
    let nstring_false: &'static NSString = ns_string!("FALSE");
    let nstring_0: &'static NSString = ns_string!("0");
    let nstring_1: &'static NSString = ns_string!("1");

    let name = NSString::from_str(cookie.name());
    let value = NSString::from_str(cookie.value());
    let path = cookie.path().map_or_else(NSString::new, NSString::from_str);
    let domain = cookie
      .domain()
      .map_or_else(NSString::new, NSString::from_str);

    let properties: Retained<NSMutableDictionary<NSHTTPCookiePropertyKey, AnyObject>> =
      NSMutableDictionary::from_slices(
        &[
          NSHTTPCookieName,
          NSHTTPCookieValue,
          NSHTTPCookiePath,
          NSHTTPCookieDomain,
        ],
        &[&name, &value, &path, &domain],
      );

    if let Some(max_age_) = cookie.max_age() {
      let max_age = NSString::from_str(&max_age_.whole_seconds().to_string());
      properties.insert(NSHTTPCookieMaximumAge, &*max_age);
      properties.insert(NSHTTPCookieVersion, nstring_1);
    } else if let Some(dt) = cookie.expires_datetime() {
      let expires = NSDate::dateWithTimeIntervalSince1970(dt.unix_timestamp() as f64);
      properties.insert(NSHTTPCookieExpires, &*expires);
      properties.insert(NSHTTPCookieVersion, nstring_0);
    }

    if let Some(secure) = cookie.secure() {
      let secure = if secure { nstring_true } else { nstring_false };
      properties.insert(NSHTTPCookieSecure, secure);
    }

    if let Some(http_only) = cookie.http_only() {
      let http_only = if http_only {
        nstring_true
      } else {
        nstring_false
      };
      // ref:
      // - <https://stackoverflow.com/a/41697557>
      // - <https://developer.apple.com/forums/thread/701770?answerId=706717022#706717022>
      properties.insert(ns_string!("HttpOnly"), http_only);
    }

    // Using strings because of https://github.com/tauri-apps/wry/issues/1616
    if let Some(same_site) = cookie.same_site() {
      let key = ns_string!("SameSite");
      let lax = ns_string!("lax");
      let strict = ns_string!("strict");
      match same_site {
        cookie::SameSite::Lax => {
          properties.insert(key, lax);
        }
        cookie::SameSite::Strict => {
          properties.insert(key, strict);
        }
        cookie::SameSite::None => {}
      };
    }

    NSHTTPCookie::cookieWithProperties(&properties).ok_or(Error::InvalidCookie)
  }

  pub fn cookies_for_url(&self, url: &str) -> Result<Vec<cookie::Cookie<'static>>> {
    let url = url::Url::parse(url)?;

    self.cookies().map(|cookies| {
      cookies.into_iter().filter(|cookie: &cookie::Cookie| {
        let secure = cookie.secure().unwrap_or_default();
        // domain is the same
        cookie.domain() == url.domain()
          // and one of
          && (
            // cookie is secure and url is https
            (secure && url.scheme() == "https") ||
            // or cookie is secure and is localhost
            (
              secure && url.scheme() == "http" &&
              (url.domain() == Some("localhost") || url.domain().and_then(|d| Ipv4Addr::from_str(d).ok()).map(|ip| ip.is_loopback()).unwrap_or(false))
            ) ||
            // or cookie is not secure
            (!secure)
          )
      }).collect()
    })
  }

  pub fn cookies(&self) -> Result<Vec<cookie::Cookie<'static>>> {
    let (tx, rx) = std::sync::mpsc::channel();

    unsafe {
      // <https://developer.apple.com/documentation/webkit/wkwebsitedatastore/httpcookiestore>
      // <https://developer.apple.com/documentation/webkit/wkhttpcookiestore/getallcookies(_:)>
      // Available: macOS 10.13+, iOS 11+
      self
        .data_store
        .httpCookieStore()
        .getAllCookies(&block2::RcBlock::new(
          move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
            let cookies = cookies.as_ref();
            let cookies = cookies
              .to_vec()
              .into_iter()
              .map(|cookie| Self::cookie_from_wkwebview(&cookie))
              .collect();
            let _ = tx.send(cookies);
          },
        ));

      wait_for_blocking_operation(rx)
    }
  }

  pub fn set_cookie(&self, cookie: &cookie::Cookie<'_>) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();

    unsafe {
      let wkwebview_cookie = Self::cookie_into_wkwebview(cookie)?;
      // <https://developer.apple.com/documentation/webkit/wkwebsitedatastore/httpcookiestore>
      // <https://developer.apple.com/documentation/webkit/wkhttpcookiestore/setcookie(_:completionhandler:)>
      // Available: macOS 10.13+, iOS 11+
      self
        .data_store
        .httpCookieStore()
        .setCookie_completionHandler(
          &wkwebview_cookie,
          Some(&block2::RcBlock::new(move || {
            let _ = tx.send(());
          })),
        );
      wait_for_blocking_operation(rx)
    }
  }

  pub fn delete_cookie(&self, cookie: &cookie::Cookie<'_>) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();

    unsafe {
      let wkwebview_cookie = Self::cookie_into_wkwebview(cookie)?;
      // <https://developer.apple.com/documentation/webkit/wkwebsitedatastore/httpcookiestore>
      // <https://developer.apple.com/documentation/webkit/wkhttpcookiestore/deletecookie(_:completionhandler:)>
      // Available: macOS 10.13+, iOS 11+
      self
        .data_store
        .httpCookieStore()
        .deleteCookie_completionHandler(
          &wkwebview_cookie,
          Some(&block2::RcBlock::new(move || {
            let _ = tx.send(());
          })),
        );
      wait_for_blocking_operation(rx)
    }
  }

  #[cfg(target_os = "macos")]
  pub(crate) fn reparent(&self, window: *mut NSWindow) -> crate::Result<()> {
    let _update = self
      .ns_window
      .begin_update()
      .ok_or(Error::WebKitStatePoisoned("reentrant host NSWindow update"))?;
    unsafe {
      let window =
        NonNull::new(window).ok_or(Error::NativeObjectUnavailable("reparent NSWindow"))?;
      let window = window.as_ref();
      let content_view = window
        .contentView()
        .ok_or(Error::NativeObjectUnavailable("reparent content view"))?;
      let retained_window = window.retain();
      content_view.addSubview(&self.webview);
      self.ns_window.replace(retained_window);
    }

    Ok(())
  }

  #[cfg(target_os = "macos")]
  pub(crate) fn set_traffic_light_inset(&self, position: dpi::Position) -> crate::Result<()> {
    if let Some(parent_view) = &self.parent_view {
      let window = self
        .webview
        .window()
        .ok_or(Error::NativeObjectUnavailable("WebView NSWindow"))?;
      parent_view.set_traffic_light_inset(&window, position);
    }

    Ok(())
  }

  /// Fetches all Data Store Identifiers of this application
  ///
  /// Needs to run on main thread and needs an event loop to run.
  pub fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(
    cb: F,
  ) -> crate::Result<()> {
    // make the RcBlock callback be a FnOnce
    let cb = RefCell::new(Some(cb));
    let block = block2::RcBlock::new(move |stores: NonNull<NSArray<NSUUID>>| {
      let uuid_list = unsafe { stores.as_ref() }
        .to_vec()
        .iter()
        .map(|uuid| uuid.as_bytes())
        .collect();
      if let Some(cb) = cb.take() {
        cb(uuid_list);
      }
    });

    match MainThreadMarker::new() {
      Some(mtn) => unsafe {
        // <https://developer.apple.com/documentation/webkit/wkwebsitedatastore/fetchalldatastoreidentifiers(_:)>
        // Available: macOS 14+, iOS 17+
        WKWebsiteDataStore::fetchAllDataStoreIdentifiers(&block, mtn);
        Ok(())
      },
      None => Err(Error::NotMainThread),
    }
  }

  /// Deletes a Data Store by an identifier
  ///
  /// Needs to run on main thread and needs an event loop to run.
  pub fn remove_data_store<F: FnOnce(crate::Result<()>) + Send + 'static>(uuid: &[u8; 16], cb: F) {
    let Some(mtm) = MainThreadMarker::new() else {
      cb(Err(Error::NotMainThread));
      return;
    };
    let identifier = NSUUID::from_bytes(uuid.to_owned());

    // make the RcBlock callback be a FnOnce
    let cb = RefCell::new(Some(cb));
    let block = block2::RcBlock::new(move |error: *mut NSError| {
      if error.is_null() {
        if let Some(cb) = cb.take() {
          cb(Ok(()));
        }
      } else if let Some(cb) = cb.take() {
        cb(Err(Error::DataStoreInUse));
      }
    });

    unsafe {
      // <https://developer.apple.com/documentation/webkit/wkwebsitedatastore/remove(foridentifier:completionhandler:)>
      // Available: macOS 14+, iOS 17+
      WKWebsiteDataStore::removeDataStoreForIdentifier_completionHandler(&identifier, &block, mtm);
    }
  }
}

pub fn url_from_webview(webview: &WKWebView) -> Result<String> {
  let url = unsafe { webview.URL() }.ok_or_else(|| {
    Error::Io(std::io::Error::other(
      "WKWebView did not provide a URL for the current navigation",
    ))
  })?;
  let absolute_url = url.absoluteString().ok_or_else(|| {
    Error::Io(std::io::Error::other(
      "WKWebView URL did not provide an absolute string",
    ))
  })?;
  bounded_nsstring(&absolute_url, PAGE_URL_LIMIT).ok_or_else(|| {
    Error::Io(std::io::Error::other(
      "WKWebView URL exceeded the native-to-Rust allocation limit",
    ))
  })
}

pub fn platform_webview_version() -> Result<String> {
  unsafe {
    let Some(bundle) = NSBundle::bundleWithIdentifier(ns_string!("com.apple.WebKit")) else {
      return Err(Error::Io(std::io::Error::other(
        "failed to locate com.apple.WebKit bundle",
      )));
    };
    let Some(dict) = bundle.infoDictionary() else {
      return Err(Error::Io(std::io::Error::other(
        "failed to get WebKit info dictionary",
      )));
    };

    let Some(webkit_version) = dict.objectForKey(ns_string!("CFBundleVersion")) else {
      return Err(Error::Io(std::io::Error::other(
        "failed to get WebKit version",
      )));
    };

    let Ok(webkit_version) = webkit_version.downcast::<NSString>() else {
      return Err(Error::Io(std::io::Error::other(
        "failed to parse WebKit version",
      )));
    };

    bundle.unload();
    Ok(webkit_version.to_string())
  }
}

impl Drop for InnerWebView {
  fn drop(&mut self) {
    url_scheme_handler::cancel_pending_for_webview(&self.id);
    for admission in &self.webview.ivars().custom_protocol_admission {
      admission.seal_and_drain();
    }
    if let Ok(mut state) = WEBVIEW_STATE.write() {
      state.remove(&self.id);
    }

    // We need to drop handler closures here
    unsafe {
      if let Some(ipc_handler) = self.ipc_handler_delegate.take() {
        let ipc = ns_string!(IPC_MESSAGE_HANDLER_NAME);
        // this will decrease the retain count of the ipc handler and trigger the drop
        ipc_handler
          .ivars()
          .controller
          .removeScriptMessageHandlerForName(ipc);
      }

      // Remove webview from window's NSView before dropping.
      self.webview.removeFromSuperview();
      self.webview.retain();
      self.manager.retain();
    }
  }
}

/// Converts from wry screen-coordinates to macOS screen-coordinates.
/// wry: top-left is (0, 0) and y increasing downwards
/// macOS:
///   Default coordinate system: a bottom-left is (0, 0) and y increasing upwards.
///   Flipped coordinate system: a top-left is (0, 0) and y increasing downwards.
#[allow(dead_code)]
unsafe fn window_position(view: &NSView, x: i32, y: i32, height: f64) -> CGPoint {
  let is_flipped = {
    #[cfg(target_os = "macos")]
    {
      view.isFlipped()
    }
    #[cfg(not(target_os = "macos"))]
    {
      false
    }
  };

  if is_flipped {
    CGPoint::new(x as f64, y as f64)
  } else {
    let frame: CGRect = view.frame();
    CGPoint::new(x as f64, frame.size.height - y as f64 - height)
  }
}

/// Wait synchronously for the NSRunLoop to run until a receiver has a message.
unsafe fn wait_for_blocking_operation<T>(rx: std::sync::mpsc::Receiver<T>) -> Result<T> {
  let interval = Duration::from_millis(2);
  let interval_as_secs = interval.as_secs_f64();
  let limit = 1.;
  let mut elapsed = 0.;
  // run event loop until we get the response back, blocking for at most 3 seconds
  loop {
    if let Ok(response) = rx.recv_timeout(interval) {
      return Ok(response);
    }
    elapsed += interval_as_secs;
    if elapsed >= limit {
      return Err(Error::Io(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "timed out waiting for cookies response",
      )));
    }

    // Go progress the event loop if we didn't get the result
    let rl = objc2_foundation::NSRunLoop::mainRunLoop();
    let limit_date = NSDate::dateWithTimeIntervalSinceNow(interval_as_secs);

    let mode = ns_string!("NSDefaultRunLoopMode");

    rl.acceptInputForMode_beforeDate(mode, &limit_date);
  }
}

#[cfg(test)]
mod security_policy_tests {
  #[cfg(target_os = "macos")]
  use super::{unfocused_child_focus_decision, UnfocusedChildFocusDecision as Decision};

  #[cfg(target_os = "macos")]
  #[test]
  fn unfocused_child_focus_transaction_restores_only_an_owned_responder() {
    for authority_preserved in [false, true] {
      for responder_preserved in [false, true] {
        for current_responder_owned in [false, true] {
          let expected = if !authority_preserved {
            Decision::Refuse
          } else if responder_preserved {
            Decision::Preserved
          } else if current_responder_owned {
            Decision::RestoreOwned
          } else {
            Decision::Refuse
          };
          assert_eq!(
            unfocused_child_focus_decision(
              authority_preserved,
              responder_preserved,
              current_responder_owned,
            ),
            expected
          );
        }
      }
    }
  }

  #[test]
  fn navigation_identity_gate_brackets_load_request() {
    let source = include_str!("mod.rs");
    let navigation = source
      .split("fn navigate_to_url")
      .nth(1)
      .and_then(|source| source.split("fn navigate_to_string").next())
      .expect("navigate_to_url implementation");
    let begin = navigation
      .find(".begin_programmatic_navigation()")
      .expect("programmatic gate begins before native navigation");
    let load = navigation
      .find(".loadRequest(&request)")
      .expect("native navigation request");
    let register = navigation
      .find(".register_programmatic_navigation(&navigation, absolute_url)")
      .expect("exact returned identity registration");
    assert!(begin < load);
    assert!(load < register);
    assert!(navigation.contains(".cancel_programmatic_navigation()"));
  }

  #[test]
  fn native_media_kvc_values_come_from_per_view_attributes() {
    let source = include_str!("mod.rs");
    assert!(source.contains("attributes.picture_in_picture_enabled"));
    assert!(
      source.contains("_preference.setElementFullscreenEnabled(attributes.fullscreen_enabled)")
    );
    assert!(!source
      .contains("_preference.setValue_forKey(Some(&_yes), ns_string!(\"fullScreenEnabled\"))"));
  }
}
