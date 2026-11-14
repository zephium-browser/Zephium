// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#[cfg(feature = "x11")]
use dpi::LogicalPosition;
use dpi::LogicalSize;
use ffi::CookieManageExt;
#[cfg(feature = "x11")]
use gdkx11::{
  ffi::{gdk_x11_window_foreign_new_for_display, GdkX11Display},
  X11Display,
};
#[cfg(feature = "x11")]
use gtk::glib::{self, translate::FromGlibPtrFull};
use gtk::glib::{Cast, IsA};
use gtk::{
  gdk::{self},
  gio::Cancellable,
  prelude::*,
};
use http::Request;
use javascriptcore::ValueExt;
use raw_window_handle::HasWindowHandle;
#[cfg(feature = "x11")]
use raw_window_handle::RawWindowHandle;
#[cfg(feature = "x11")]
use std::ffi::c_ulong;
#[cfg(any(debug_assertions, feature = "devtools"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
  cell::Cell,
  collections::HashMap,
  rc::Rc,
  sync::{Arc, Mutex},
  time::{Duration, Instant},
};
#[cfg(any(debug_assertions, feature = "devtools"))]
use webkit2gtk::WebInspectorExt;
use webkit2gtk::{
  AuthenticationRequestExt, AutoplayPolicy, ColorChooserRequestExt, CookieManagerExt,
  FileChooserRequestExt, GeolocationPermissionRequest, InputMethodContextExt, LoadEvent,
  NavigationPolicyDecision, NavigationPolicyDecisionExt, NetworkProxyMode, NetworkProxySettings,
  NotificationPermissionRequest, PermissionRequestExt, PointerLockPermissionRequest,
  PolicyDecisionType, PrintOperationExt, SettingsExt, URIRequest, URIRequestExt,
  UserContentInjectedFrames, UserContentManager, UserContentManagerExt, UserMediaPermissionRequest,
  UserMediaPermissionRequestExt, UserScript, UserScriptInjectionTime,
  WebContextExt as Webkit2gtkWeContextExt, WebView, WebViewExt, WebsiteDataManagerExt,
  WebsiteDataManagerExtManual, WebsitePolicies,
};
use webkit2gtk_sys::{
  webkit_get_major_version, webkit_get_micro_version, webkit_get_minor_version,
  webkit_policy_decision_ignore, webkit_policy_decision_use, webkit_uri_request_get_uri,
  webkit_user_media_permission_is_for_display_device, webkit_web_view_get_title,
  webkit_web_view_get_uri,
};
#[cfg(feature = "x11")]
use x11_dl::xlib::*;

pub use web_context::WebContextImpl;

#[cfg(test)]
pub(crate) use self::web_context::WebContextExt;

use crate::{
  native_bounds::{
    bounded_utf8_bytes, bounded_utf8_c_string, IPC_PAYLOAD_LIMIT, PAGE_TITLE_LIMIT, PAGE_URL_LIMIT,
  },
  proxy::ProxyConfig,
  web_context::WebContext,
  Error, NavigationEvent, NavigationEventPhase, NavigationId, NewWindowFeatures, NewWindowOpener,
  NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, Rect, Result,
  WebViewAttributes, RGBA,
};

#[cfg(not(test))]
use self::web_context::WebContextExt;

const WEBVIEW_ID: &str = "webview_id";
const NATIVE_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const IPC_HANDLER_NAME: &str = "wryIpc";
const IPC_WORLD_NAME: &str = "wry-ipc-isolated-world";
const IPC_BRIDGE_EVENT: &str = "wry-ipc-message-v1";

const IPC_PAGE_BRIDGE_TEMPLATE: &str = r#"
(() => {
  "use strict";
  const dispatch = EventTarget.prototype.dispatchEvent;
  const CustomEventConstructor = CustomEvent;
  const target = document;
  const bridge = Object.freeze({
    postMessage(value) {
      if (typeof value !== "string" || value.length > __WRY_UTF16_LIMIT__) return;
      dispatch.call(target, new CustomEventConstructor("__WRY_IPC_EVENT__", { detail: value }));
    }
  });
  Object.defineProperty(window, "ipc", { value: bridge });
})();
"#;

const IPC_ISOLATED_BRIDGE_TEMPLATE: &str = r#"
(() => {
  "use strict";
  const handler = window.webkit?.messageHandlers?.["__WRY_IPC_HANDLER__"];
  if (!handler || typeof handler.postMessage !== "function") return;

  const withinUtf8Limit = (value) => {
    let bytes = 0;
    for (let index = 0; index < value.length; index += 1) {
      const unit = value.charCodeAt(index);
      if (unit < 0x80) {
        bytes += 1;
      } else if (unit < 0x800) {
        bytes += 2;
      } else if (unit >= 0xD800 && unit <= 0xDBFF && index + 1 < value.length) {
        const next = value.charCodeAt(index + 1);
        if (next >= 0xDC00 && next <= 0xDFFF) {
          bytes += 4;
          index += 1;
        } else {
          bytes += 3;
        }
      } else {
        bytes += 3;
      }
      if (bytes > __WRY_UTF8_LIMIT__) return false;
    }
    return true;
  };

  document.addEventListener("__WRY_IPC_EVENT__", (event) => {
    const value = event.detail;
    if (
      typeof value !== "string" ||
      value.length > __WRY_UTF16_LIMIT__ ||
      !withinUtf8Limit(value)
    ) return;
    handler.postMessage(value);
  });
})();
"#;

fn ipc_page_bridge_script() -> String {
  IPC_PAGE_BRIDGE_TEMPLATE
    .replace(
      "__WRY_UTF16_LIMIT__",
      &IPC_PAYLOAD_LIMIT.max_utf16_units.to_string(),
    )
    .replace("__WRY_IPC_EVENT__", IPC_BRIDGE_EVENT)
}

fn ipc_isolated_bridge_script() -> String {
  IPC_ISOLATED_BRIDGE_TEMPLATE
    .replace("__WRY_IPC_HANDLER__", IPC_HANDLER_NAME)
    .replace("__WRY_IPC_EVENT__", IPC_BRIDGE_EVENT)
    .replace(
      "__WRY_UTF16_LIMIT__",
      &IPC_PAYLOAD_LIMIT.max_utf16_units.to_string(),
    )
    .replace(
      "__WRY_UTF8_LIMIT__",
      &IPC_PAYLOAD_LIMIT.max_utf8_bytes.to_string(),
    )
}

#[cfg(test)]
mod ipc_bridge_tests {
  use super::*;

  #[test]
  fn page_world_has_no_direct_native_message_handler() {
    let script = ipc_page_bridge_script();
    assert!(!script.contains("messageHandlers"));
    assert!(!script.contains("__WRY_"));
    assert!(script.contains(IPC_BRIDGE_EVENT));
    assert!(script.contains(&IPC_PAYLOAD_LIMIT.max_utf16_units.to_string()));
  }

  #[test]
  fn isolated_world_checks_both_native_string_limits() {
    let script = ipc_isolated_bridge_script();
    assert!(!script.contains("__WRY_"));
    assert!(script.contains(IPC_HANDLER_NAME));
    assert!(script.contains(IPC_BRIDGE_EVENT));
    assert!(script.contains(&IPC_PAYLOAD_LIMIT.max_utf16_units.to_string()));
    assert!(script.contains(&IPC_PAYLOAD_LIMIT.max_utf8_bytes.to_string()));
  }
}

fn bounded_webview_title(webview: &WebView) -> Option<String> {
  // SAFETY: WebKit owns the returned pointer through this call. The helper
  // performs a bounded scan and copies only after finding a terminator.
  unsafe {
    bounded_utf8_c_string(
      webkit_web_view_get_title(webview.as_ptr()),
      PAGE_TITLE_LIMIT,
    )
  }
}

fn bounded_webview_uri(webview: &WebView) -> Option<String> {
  // SAFETY: same ownership contract as `bounded_webview_title`.
  unsafe { bounded_utf8_c_string(webkit_web_view_get_uri(webview.as_ptr()), PAGE_URL_LIMIT) }
}

fn bounded_request_uri(request: &URIRequest) -> Option<String> {
  // SAFETY: WebKit owns the request and returned URI pointer through this call.
  unsafe { bounded_utf8_c_string(webkit_uri_request_get_uri(request.as_ptr()), PAGE_URL_LIMIT) }
}

fn user_media_request_is_for_display_device(request: &UserMediaPermissionRequest) -> bool {
  // SAFETY: the typed GLib wrapper keeps this native request alive for the
  // duration of the permission callback. WebKitGTK added this read-only query
  // in 2.34; Wry's v2_40 API floor guarantees that the symbol is available.
  unsafe { webkit_user_media_permission_is_for_display_device(request.as_ptr()) != 0 }
}

/// WebKitGTK exposes one ordered main-frame load sequence per WebView but no
/// public navigation token. Allocate one non-wrapping identity at
/// `WEBKIT_LOAD_STARTED` and retain it through redirects, commit, and the
/// mandatory terminal `WEBKIT_LOAD_FINISHED` event. A load failure is emitted
/// before that terminal event, so `failed` suppresses a false success.
#[derive(Clone, Copy, Default)]
struct GtkNavigationSequence {
  next: u64,
  active: Option<GtkActiveNavigation>,
  exhausted: bool,
}

#[derive(Clone, Copy)]
struct GtkActiveNavigation {
  id: NavigationId,
  failed: bool,
  committed: bool,
}

impl GtkNavigationSequence {
  fn started(&mut self) -> Option<NavigationId> {
    if self.exhausted {
      return None;
    }
    let Some(next) = self.next.checked_add(1) else {
      self.exhausted = true;
      self.active = None;
      return None;
    };
    self.next = next;
    let id = NavigationId::from_raw(next);
    self.active = Some(GtkActiveNavigation {
      id,
      failed: false,
      committed: false,
    });
    Some(id)
  }

  #[cfg(test)]
  fn active(&self) -> Option<NavigationId> {
    self
      .active
      .filter(|active| !active.failed)
      .map(|active| active.id)
  }

  fn redirected(&self) -> Option<NavigationId> {
    self
      .active
      .filter(|active| !active.failed && !active.committed)
      .map(|active| active.id)
  }

  fn committed(&mut self) -> Option<NavigationId> {
    let active = self.active.as_mut()?;
    if active.failed || active.committed {
      return None;
    }
    active.committed = true;
    Some(active.id)
  }

  fn failed(&mut self) -> Option<NavigationId> {
    let active = self.active.as_mut()?;
    if active.failed {
      return None;
    }
    active.failed = true;
    Some(active.id)
  }

  fn finished(&mut self) -> Option<NavigationId> {
    self
      .active
      .take()
      .filter(|active| !active.failed)
      .map(|active| active.id)
  }
}

#[cfg(test)]
mod navigation_sequence_tests {
  use super::*;

  #[test]
  fn redirects_keep_one_identity_and_failure_suppresses_success() {
    let mut sequence = GtkNavigationSequence::default();
    let first = sequence.started().unwrap();
    assert_eq!(sequence.active(), Some(first));
    assert_eq!(sequence.failed(), Some(first));
    assert_eq!(sequence.active(), None);
    assert_eq!(sequence.finished(), None);

    let second = sequence.started().unwrap();
    assert_ne!(first, second);
    assert_eq!(sequence.active(), Some(second));
    assert_eq!(sequence.redirected(), Some(second));
    assert_eq!(sequence.committed(), Some(second));
    assert_eq!(sequence.committed(), None);
    assert_eq!(sequence.redirected(), None);
    assert_eq!(sequence.finished(), Some(second));
  }

  #[test]
  fn identity_exhaustion_is_terminal_instead_of_wrapping() {
    let mut sequence = GtkNavigationSequence {
      next: u64::MAX,
      ..Default::default()
    };
    assert_eq!(sequence.started(), None);
    sequence.next = 0;
    assert_eq!(sequence.started(), None);
  }

  #[test]
  fn commit_without_a_started_sequence_has_no_synthetic_identity() {
    let mut sequence = GtkNavigationSequence::default();
    assert_eq!(sequence.committed(), None);
    assert_eq!(sequence.finished(), None);
  }
}

mod drag_drop;
mod synthetic_mouse_events;
mod web_context;

#[cfg(feature = "x11")]
struct X11Data {
  is_child: bool,
  xlib: Xlib,
  x11_display: *mut std::ffi::c_void,
  x11_window: c_ulong,
  gtk_window: gtk::Window,
}

#[cfg(feature = "x11")]
impl Drop for X11Data {
  fn drop(&mut self) {
    unsafe { (self.xlib.XDestroyWindow)(self.x11_display as _, self.x11_window) };
    self.gtk_window.close();
  }
}

pub(crate) struct InnerWebView {
  id: String,
  pub webview: WebView,
  #[cfg(any(debug_assertions, feature = "devtools"))]
  is_inspector_open: Arc<AtomicBool>,
  pending_scripts: Arc<Mutex<Option<Vec<String>>>>,
  is_in_fixed_parent: Cell<bool>,

  #[cfg(feature = "x11")]
  x11: Option<X11Data>,
}

impl Drop for InnerWebView {
  fn drop(&mut self) {
    unsafe { self.webview.destroy() }
  }
}

impl InnerWebView {
  pub fn new<W: HasWindowHandle>(
    window: &W,
    attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
  ) -> Result<Self> {
    #[cfg(feature = "x11")]
    {
      Self::new_x11(window, attributes, pl_attrs, false)
    }
    #[cfg(not(feature = "x11"))]
    {
      let _ = window;
      let _ = attributes;
      let _ = pl_attrs;
      Err(Error::UnsupportedWindowHandle)
    }
  }

  pub fn new_as_child<W: HasWindowHandle>(
    parent: &W,
    attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
  ) -> Result<Self> {
    #[cfg(feature = "x11")]
    {
      Self::new_x11(parent, attributes, pl_attrs, true)
    }
    #[cfg(not(feature = "x11"))]
    {
      let _ = parent;
      let _ = attributes;
      let _ = pl_attrs;
      Err(Error::UnsupportedWindowHandle)
    }
  }

  #[cfg(feature = "x11")]
  fn new_x11<W: HasWindowHandle>(
    window: &W,
    attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
    is_child: bool,
  ) -> Result<Self> {
    let parent = match window.window_handle()?.as_raw() {
      RawWindowHandle::Xlib(w) => w.window,
      _ => return Err(Error::UnsupportedWindowHandle),
    };

    let xlib = Xlib::open()?;

    let gdk_display = gdk::Display::default().ok_or(crate::Error::X11DisplayNotFound)?;
    let gx11_display: &X11Display = gdk_display
      .downcast_ref()
      .ok_or(crate::Error::X11DisplayNotFound)?;
    let raw = gx11_display.as_ptr();

    let x11_display = unsafe { gdkx11::ffi::gdk_x11_display_get_xdisplay(raw) };

    let x11_window = match is_child {
      true => Self::create_container_x11_window(&xlib, x11_display as _, parent, &attributes)?,
      false => parent,
    };

    let (gtk_window, vbox) = Self::create_gtk_window(raw, x11_window)?;

    let visible = attributes.visible;
    let guards_initial_presentation = attributes.guards_initial_presentation();

    Self::new_gtk(&vbox, attributes, pl_attrs).map(|mut w| {
      // Ordinary initially-hidden X11 views need this show/hide cycle or
      // later visibility changes can fail. A presentation-guarded view must
      // instead stay genuinely unmapped until the embedder has parked it;
      // applying the workaround here would expose its on-screen allocation.
      if !guards_initial_presentation {
        gtk_window.show_all();
        if !visible {
          let _ = w.set_visible(false);
        }
      }

      w.x11.replace(X11Data {
        is_child,
        xlib,
        x11_display: x11_display as _,
        x11_window,
        gtk_window,
      });

      w
    })
  }

  #[cfg(feature = "x11")]
  fn create_container_x11_window(
    xlib: &Xlib,
    display: *mut _XDisplay,
    parent: c_ulong,
    attributes: &WebViewAttributes,
  ) -> Result<c_ulong> {
    let scale_factor = scale_factor_from_x11(xlib, display, parent);
    let (x, y) = attributes
      .bounds
      .map(|b| b.position.to_physical::<f64>(scale_factor))
      .map(Into::into)
      .unwrap_or((0, 0));
    let (width, height) = attributes
      .bounds
      .map(|b| b.size.to_physical::<u32>(scale_factor))
      .map(Into::into)
      // it is unlikey that bounds are not set because
      // we have a default for it, but anyways we need to have a fallback
      // and we need to use 1 not 0 here otherwise xlib will crash
      .unwrap_or((1, 1));
    let (width, height) = (width.max(1), height.max(1));

    let window =
      unsafe { (xlib.XCreateSimpleWindow)(display, parent, x, y, width, height, 0, 0, 0) };
    if window == 0 {
      return Err(Error::X11WindowCreationFailed);
    }

    if attributes.maps_during_initial_construction() {
      unsafe { (xlib.XMapWindow)(display, window) };
    }

    Ok(window)
  }

  #[cfg(feature = "x11")]
  pub fn create_gtk_window(
    raw: *mut GdkX11Display,
    x11_window: c_ulong,
  ) -> Result<(gtk::Window, gtk::Box)> {
    // Gdk.Window
    let gdk_window = unsafe { gdk_x11_window_foreign_new_for_display(raw, x11_window) };
    if gdk_window.is_null() {
      return Err(Error::X11WindowCreationFailed);
    }
    let gdk_window = unsafe { gdk::Window::from_glib_full(gdk_window) };

    // Gtk.Window
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.connect_realize(glib::clone!(@weak gdk_window as wd => move |w| w.set_window(wd)));
    window.set_has_window(true);
    window.realize();

    // Gtk.Box (vertical)
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&vbox);

    Ok((window, vbox))
  }

  pub fn new_gtk<W>(
    container: &W,
    mut attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
  ) -> Result<Self>
  where
    W: IsA<gtk::Container>,
  {
    // `attach_handlers` moves the callback into the native signal closure.
    // Retain the construction policy separately so no later visibility path
    // can accidentally infer it from an already-consumed Option.
    let guards_initial_presentation = attributes.guards_initial_presentation();
    let maps_during_initial_construction = attributes.maps_during_initial_construction();
    let focuses_during_initial_construction = attributes.focuses_during_initial_construction();

    // default_context allows us to create a scoped context on-demand
    let mut default_context;
    let web_context = if attributes.incognito {
      match attributes.context.take() {
        Some(context) => {
          if !context.context().is_ephemeral() {
            return Err(Error::NonEphemeralIncognitoContext);
          }
          context
        }
        None => {
          default_context = WebContext::new_ephemeral()?;
          &mut default_context
        }
      }
    } else {
      match attributes.context.take() {
        Some(w) => w,
        None => {
          default_context = WebContext::try_new(None)?;
          &mut default_context
        }
      }
    };
    // `WebContext::new` is source-compatible and therefore cannot return an
    // error. Every build still re-reads the mandatory native properties and
    // fails before WebKit can create a WebView or launch a WebProcess.
    web_context.os.validate_security()?;
    if attributes.incognito {
      if let Some(related_view) = &pl_attrs.related_view {
        let related_context = related_view
          .context()
          .ok_or(Error::IncognitoRelatedViewContextMismatch)?;
        if !related_view.is_ephemeral()
          || related_context.as_ptr() != web_context.context().as_ptr()
        {
          return Err(Error::IncognitoRelatedViewContextMismatch);
        }
      }
    }
    if let Some(proxy_setting) = &attributes.proxy_config {
      let proxy_uri = match proxy_setting {
        ProxyConfig::Http(endpoint) => format!("http://{}:{}", endpoint.host, endpoint.port),
        ProxyConfig::Socks5(endpoint) => {
          format!("socks5://{}:{}", endpoint.host, endpoint.port)
        }
      };
      if let Some(website_data_manager) = web_context.context().website_data_manager() {
        let mut settings = NetworkProxySettings::new(Some(proxy_uri.as_str()), &[]);
        website_data_manager
          .set_network_proxy_settings(NetworkProxyMode::Custom, Some(&mut settings));
      }
    }

    // Extension loading
    if let Some(extension_path) = &pl_attrs.extension_path {
      web_context.os.set_web_extensions_directory(extension_path);
    }

    let webview = Self::create_webview(web_context, &attributes, &pl_attrs);

    // A transparent mapped WebKitGTK child can still publish a black accelerated
    // layer and intercept input. A guarded untrusted view therefore starts
    // insensitive, behind GTK's mapping barrier, transparent, and ordinarily
    // hidden. Do this immediately after allocation; every GTK setter can pump
    // callbacks. The embedder's exact attributed stage performs the first
    // fully-offscreen map after construction.
    if guards_initial_presentation {
      webview.set_sensitive(false);
      webview.set_child_visible(false);
      webview.set_opacity(0.0);
      webview.hide();
    }

    // Transparent
    if attributes.transparent {
      webview.set_background_color(&gtk::gdk::RGBA::new(0., 0., 0., 0.));
    } else {
      // background color
      if let Some((red, green, blue, alpha)) = attributes.background_color {
        webview.set_background_color(&gtk::gdk::RGBA::new(
          red as f64 / 255.0,
          green as f64 / 255.0,
          blue as f64 / 255.0,
          alpha as f64 / 255.0,
        ));
      }
    }

    // Webview Settings
    Self::set_webview_settings(&webview, &attributes);

    // Webview handlers
    Self::attach_handlers(&webview, web_context, &mut attributes);

    // Do not expose a page-visible bridge unless a native receiver exists.
    let has_ipc_handler = attributes.ipc_handler.is_some();
    if has_ipc_handler {
      Self::attach_ipc_handler(webview.clone(), &mut attributes)?;
    }

    // Drag drop handler
    if let Some(drag_drop_handler) = attributes.drag_drop_handler.take() {
      drag_drop::connect_drag_event(&webview, drag_drop_handler);
    }

    web_context.register_automation(webview.clone());

    let is_in_fixed_parent = Self::add_to_container(&webview, container, &attributes);
    if guards_initial_presentation {
      // Parenting or a re-entrant ancestor operation must not make a guarded
      // child mappable at its construction-time (on-screen) bounds.
      webview.set_child_visible(false);
      webview.hide();
    }

    #[cfg(any(debug_assertions, feature = "devtools"))]
    let is_inspector_open = Self::attach_inspector_handlers(&webview);

    let id = attributes
      .id
      .map(|id| id.to_string())
      .unwrap_or_else(|| (webview.as_ptr() as isize).to_string());
    unsafe { webview.set_data(WEBVIEW_ID, id.clone()) };

    let w = Self {
      id,
      webview,
      pending_scripts: Arc::new(Mutex::new(Some(Vec::new()))),

      is_in_fixed_parent: Cell::new(is_in_fixed_parent),
      #[cfg(feature = "x11")]
      x11: None,

      #[cfg(any(debug_assertions, feature = "devtools"))]
      is_inspector_open,
    };

    if has_ipc_handler {
      w.init(&ipc_page_bridge_script(), true)?;
    }

    // Initialize scripts
    for init_script in attributes.initialization_scripts {
      w.init(&init_script.script, init_script.for_main_frame_only)?;
    }

    // Run pending webview.eval() scripts once webview loads.
    let pending_scripts = w.pending_scripts.clone();
    w.webview.connect_load_changed(move |webview, event| {
      if let LoadEvent::Committed = event {
        let Ok(mut pending_scripts_) = pending_scripts.lock() else {
          return;
        };
        if let Some(pending_scripts) = pending_scripts_.take() {
          let cancellable: Option<&Cancellable> = None;
          for script in pending_scripts {
            webview.evaluate_javascript(&script, None, None, cancellable, |_| ());
          }
        }
      }
    });

    // Custom protocols handler
    for (name, handler) in attributes.custom_protocols {
      web_context.register_uri_scheme(&name, handler)?;
    }

    // Navigation
    if let Some(url) = attributes.url {
      web_context.load_uri(w.webview.clone(), url, attributes.headers);
    } else if let Some(html) = attributes.html {
      w.webview.load_html(&html, None);
    }

    if maps_during_initial_construction {
      w.webview.show_all();
    }

    if focuses_during_initial_construction {
      w.webview.grab_focus();
    }

    Ok(w)
  }

  fn create_webview(
    web_context: &WebContext,
    attributes: &WebViewAttributes,
    pl_attrs: &super::PlatformSpecificWebViewAttributes,
  ) -> WebView {
    let mut builder = WebView::builder()
      .user_content_manager(&UserContentManager::new())
      .is_controlled_by_automation(web_context.allows_automation());

    if attributes.autoplay {
      builder = builder.website_policies(
        &WebsitePolicies::builder()
          .autoplay(AutoplayPolicy::Allow)
          .build(),
      );
    }

    if let Some(related_view) = &pl_attrs.related_view {
      builder = builder.related_view(related_view);
    } else {
      builder = builder.web_context(web_context.context());
    }

    builder.build()
  }

  fn set_webview_settings(webview: &WebView, attributes: &WebViewAttributes) {
    // Disable input preedit,fcitx input editor can anchor at edit cursor position
    if let Some(input_context) = webview.input_method_context() {
      input_context.set_enable_preedit(false);
    }

    // use system scrollbars
    if let Some(context) = webview.context() {
      context.set_use_system_appearance_for_scrollbars(false);
    }

    if let Some(settings) = WebViewExt::settings(webview) {
      // Enable webgl, webaudio, canvas features as default.
      settings.set_enable_webgl(true);
      settings.set_enable_webaudio(true);
      // Fullscreen is a page-triggered native surface and must remain off
      // until the embedder owns an origin/gesture-labelled lifecycle broker.
      settings.set_enable_fullscreen(false);
      settings
        .set_enable_back_forward_navigation_gestures(attributes.back_forward_navigation_gestures);

      // Enable clipboard
      if attributes.clipboard {
        settings.set_javascript_can_access_clipboard(true);
      }

      // Enable App cache
      settings.set_enable_page_cache(true);

      // Set user agent
      settings.set_user_agent(attributes.user_agent.as_deref());

      // Devtools
      if attributes.devtools {
        settings.set_enable_developer_extras(true);
      }

      if attributes.javascript_disabled {
        settings.set_enable_javascript(false);
      }
    }
  }

  fn attach_handlers(
    webview: &WebView,
    web_context: &mut WebContext,
    attributes: &mut WebViewAttributes,
  ) {
    // Page close is a request, not permission to invalidate the embedder's
    // logical view/widget ownership. Preserve the child by default and retain
    // legacy destruction only behind an explicit host policy.
    let page_close_policy = attributes.page_close_policy;
    webview.connect_close(move |webview| {
      if page_close_policy == crate::PageClosePolicy::DestroyContainer {
        unsafe { webview.destroy() };
      }
    });

    // Synthetic mouse events
    synthetic_mouse_events::setup(webview);

    // Document title changed handler
    if let Some(document_title_changed_handler) = attributes.document_title_changed_handler.take() {
      webview.connect_title_notify(move |webview| {
        if let Some(new_title) = bounded_webview_title(webview) {
          document_title_changed_handler(new_title)
        }
      });
    }

    // Identity-bearing main-frame navigation handler. WebKitGTK's
    // `load-changed` contract defines Started -> zero or more Redirected ->
    // Committed -> Finished as one ordered load operation. Keep a synthetic
    // non-wrapping identity for that native sequence; URL equality is not an
    // identity and the committed URI is intentionally the final redirect.
    if let Some(navigation_event_handler) = attributes.navigation_event_handler.take() {
      let navigation_presentation_guard = attributes.navigation_presentation_guard.take();
      let navigation_event_handler = Rc::new(navigation_event_handler);
      let sequence = Rc::new(Cell::new(GtkNavigationSequence::default()));
      let changed_sequence = sequence.clone();
      let changed_handler = navigation_event_handler.clone();
      webview.connect_load_changed(move |webview, load_event| {
        let mut sequence = changed_sequence.get();
        let (transition, native_committed) = match load_event {
          LoadEvent::Started => (
            sequence
              .started()
              .map(|id| (id, NavigationEventPhase::Started)),
            false,
          ),
          LoadEvent::Redirected => (
            sequence
              .redirected()
              .map(|id| (id, NavigationEventPhase::Redirected)),
            false,
          ),
          LoadEvent::Committed => (
            sequence
              .committed()
              .map(|id| (id, NavigationEventPhase::Committed)),
            true,
          ),
          LoadEvent::Finished => (
            sequence
              .finished()
              .map(|id| (id, NavigationEventPhase::Finished)),
            false,
          ),
          _ => (None, false),
        };
        changed_sequence.set(sequence);
        if native_committed {
          if let Some(guard) = navigation_presentation_guard.as_ref() {
            // Conceal every native commit even when the synthetic identity is
            // missing or exhausted. The unattributed document has no legal
            // reveal path. Revoke the permit before native calls, then remove
            // input, mapping and paint before embedder dispatch. Every setter
            // may pump GTK; the final barrier dominates nested stale layout.
            guard();
            webview.set_sensitive(false);
            webview.set_child_visible(false);
            webview.set_opacity(0.0);
            webview.hide();
            webview.set_child_visible(false);
          }
        }
        if let (Some((id, phase)), Some(url)) = (transition, bounded_webview_uri(webview)) {
          changed_handler(NavigationEvent { id, phase, url });
        }
      });

      let failed_handler = navigation_event_handler.clone();
      webview.connect_load_failed(move |_webview, _load_event, failing_uri, _error| {
        let mut current = sequence.get();
        let id = current.failed();
        sequence.set(current);
        let url = bounded_utf8_bytes(failing_uri.as_bytes(), PAGE_URL_LIMIT);
        if let (Some(id), Some(url)) = (id, url) {
          failed_handler(NavigationEvent {
            id,
            phase: NavigationEventPhase::Failed,
            url,
          });
        }
        // Preserve WebKit's ordinary error-page behavior. The identity event
        // is observational and does not weaken the navigation policy.
        false
      });
    }

    // Legacy page load handler. A transiently absent native URI is not a page with an
    // empty origin, so do not manufacture an event for it.
    if let Some(on_page_load_handler) = attributes.on_page_load_handler.take() {
      webview.connect_load_changed(move |webview, load_event| match load_event {
        LoadEvent::Committed => {
          if let Some(uri) = bounded_webview_uri(webview) {
            on_page_load_handler(PageLoadEvent::Started, uri);
          }
        }
        LoadEvent::Finished => {
          if let Some(uri) = bounded_webview_uri(webview) {
            on_page_load_handler(PageLoadEvent::Finished, uri);
          }
        }
        _ => (),
      });
    }

    // window creation handler
    if let Some(new_window_req_handler) = attributes.new_window_req_handler.take() {
      let related_webviews = Rc::new(Mutex::new(HashMap::new()));
      webview.connect_create(move |webview, action| {
        let url = action
          .request()
          .and_then(|request| bounded_request_uri(&request))?;
        match new_window_req_handler(
          url.clone(),
          NewWindowFeatures {
            user_initiated: action.is_user_gesture(),
            foreground: true,
            size: None,
            position: None,
            opener: NewWindowOpener {
              webview: webview.clone(),
            },
          },
        ) {
          NewWindowResponse::Allow => {
            let related_webviews = related_webviews.clone();
            let toplevel = webview.toplevel()?;
            let parent_window = toplevel.downcast::<gtk::ApplicationWindow>().ok()?;
            let app = parent_window.application()?;

            let window = gtk::ApplicationWindow::builder()
              .application(&app)
              .title(&url)
              .build();
            let id = window.id();
            let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
            window.add(&box_);

            let related_webviews_ = related_webviews.clone();
            window.connect_destroy(move |_| {
              if let Ok(mut webviews) = related_webviews_.lock() {
                webviews.remove(&id);
              }
            });

            window.show_all();
            Self::new_gtk(
              &box_,
              WebViewAttributes {
                ..Default::default()
              },
              super::PlatformSpecificWebViewAttributes {
                related_view: Some(webview.clone()),
                ..Default::default()
              },
            )
            .ok()
            .and_then(|webview| {
              let widget = webview.webview.upcast_ref::<gtk::Widget>().clone();
              let Ok(mut webviews) = related_webviews.lock() else {
                return None;
              };
              webviews.insert(id, webview);
              Some(widget)
            })
          }
          NewWindowResponse::Create { webview } => Some(webview.upcast::<gtk::Widget>()),
          NewWindowResponse::Deny => None,
        }
      });
    }

    // Navigation handler
    if let Some(navigation_handler) = attributes.navigation_handler.take() {
      webview.connect_decide_policy(move |_webview, policy_decision, policy_type| {
        let handler = match policy_type {
          PolicyDecisionType::NavigationAction => &navigation_handler,
          _ => return false,
        };

        // Missing action/request/URI state during navigation or teardown must
        // not fall back to WebKit's default allow path.
        let allow = policy_decision
          .dynamic_cast_ref::<NavigationPolicyDecision>()
          .and_then(NavigationPolicyDecisionExt::navigation_action)
          .and_then(|action| action.request())
          .and_then(|request| bounded_request_uri(&request))
          .map(handler)
          .unwrap_or(false);
        let pointer = policy_decision.as_ptr();
        unsafe {
          if allow {
            webkit_policy_decision_use(pointer)
          } else {
            webkit_policy_decision_ignore(pointer)
          }
        }
        true
      });
    }

    // Every engine-owned prompt is denied unless an embedder explicitly
    // brokers and allows it. `Default` is also fail-closed in this fork: it
    // must never delegate to an unlabelled native WebKit prompt.
    let permission_handler = attributes.permission_handler.take();
    webview.connect_permission_request(move |_webview, request| {
      let response_for = |kind| {
        permission_handler
          .as_ref()
          .map(|handler| handler(kind))
          .unwrap_or(PermissionResponse::Deny)
      };
      if let Some(media_request) = request.downcast_ref::<UserMediaPermissionRequest>() {
        let is_audio = media_request.is_for_audio_device();
        let is_video = media_request.is_for_video_device();

        let is_display = user_media_request_is_for_display_device(media_request);

        if is_display {
          // Screen sharing request
          let response = response_for(PermissionKind::DisplayCapture);
          return match response {
            PermissionResponse::Allow => {
              request.allow();
              true
            }
            PermissionResponse::Deny => {
              request.deny();
              true
            }
            PermissionResponse::Default | PermissionResponse::Prompt => {
              request.deny();
              true
            }
          };
        }

        // For combined audio+video requests, check each individually.
        // Deny wins: if either is denied, deny the whole request.
        let mut allow = true;
        let mut handled = false;

        if is_audio {
          handled = true;
          match response_for(PermissionKind::Microphone) {
            PermissionResponse::Allow => {}
            PermissionResponse::Deny => allow = false,
            PermissionResponse::Default | PermissionResponse::Prompt => allow = false,
          }
        }

        if is_video && allow {
          handled = true;
          match response_for(PermissionKind::Camera) {
            PermissionResponse::Allow => {}
            PermissionResponse::Deny => allow = false,
            PermissionResponse::Default | PermissionResponse::Prompt => allow = false,
          }
        }

        if handled {
          if allow {
            request.allow();
          } else {
            request.deny();
          }
          true
        } else {
          request.deny();
          true
        }
      } else {
        let permission_kind = if request.is::<GeolocationPermissionRequest>() {
          PermissionKind::Geolocation
        } else if request.is::<NotificationPermissionRequest>() {
          PermissionKind::Notifications
        } else if request.is::<PointerLockPermissionRequest>() {
          PermissionKind::PointerLock
        } else {
          PermissionKind::Other
        };

        match response_for(permission_kind) {
          PermissionResponse::Allow => {
            request.allow();
            true
          }
          PermissionResponse::Deny => {
            request.deny();
            true
          }
          PermissionResponse::Default | PermissionResponse::Prompt => {
            request.deny();
            true
          }
        }
      }
    });

    // These surfaces have no origin-labelled Zephium broker yet. Claim the
    // signal and complete the request with a denial rather than allowing a
    // page to summon privileged native UI.
    webview.connect_authenticate(|_, request| {
      request.cancel();
      true
    });
    webview.connect_run_file_chooser(|_, request| {
      request.cancel();
      true
    });
    webview.connect_run_color_chooser(|_, request| {
      request.cancel();
      true
    });
    webview.connect_script_dialog(|_, dialog| {
      dialog.close();
      true
    });
    webview.connect_print(|_, _| true);

    // Download handler
    web_context.register_download_handler(
      attributes.download_policy,
      attributes.download_started_handler.take(),
      attributes.download_completed_handler.take(),
    )
  }

  fn add_to_container<W>(webview: &WebView, container: &W, attributes: &WebViewAttributes) -> bool
  where
    W: IsA<gtk::Container>,
  {
    let mut is_in_fixed_parent = false;

    if let Some(container) = container.dynamic_cast_ref::<gtk::Box>() {
      container.pack_start(webview, true, true, 0);
    } else if let Some(container) = container.dynamic_cast_ref::<gtk::Fixed>() {
      let scale_factor = webview.scale_factor() as f64;
      let (width, height) = attributes
        .bounds
        .map(|b| b.size.to_logical::<i32>(scale_factor))
        .map(Into::into)
        .unwrap_or((1, 1));
      let (x, y) = attributes
        .bounds
        .map(|b| b.position.to_logical::<i32>(scale_factor))
        .map(Into::into)
        .unwrap_or((0, 0));

      webview.set_size_request(width, height);
      container.put(webview, x, y);
      is_in_fixed_parent = true;
    } else {
      container.add(webview);
    }

    is_in_fixed_parent
  }

  fn attach_ipc_handler(webview: WebView, attributes: &mut WebViewAttributes) -> crate::Result<()> {
    // Message handler
    let Some(ipc_handler) = attributes.ipc_handler.take() else {
      return Ok(());
    };
    let manager = webview
      .user_content_manager()
      .ok_or(crate::Error::MissingManager)?;

    // The manager is owned by the WebView. Capturing a strong WebView here
    // would form WebView -> manager -> signal closure -> WebView and retain
    // the full native page after the embedder drops it.
    let weak_webview = webview.downgrade();

    // Connect before registering as recommended by the docs.
    manager.connect_script_message_received(Some(IPC_HANDLER_NAME), move |_m, msg| {
      #[cfg(feature = "tracing")]
      let _span = tracing::info_span!(parent: None, "wry::ipc::handle").entered();

      let Some(webview) = weak_webview.upgrade() else {
        return;
      };
      let (Some(js), Some(uri)) = (msg.js_value(), bounded_webview_uri(&webview)) else {
        return;
      };
      if !js.is_string() {
        return;
      }
      // The page cannot access this native handler: it exists only in the
      // isolated world, whose listener rejects UTF-16 and UTF-8 overflow
      // before calling WebKit's native message conversion. Keep the native
      // bound as a second fail-closed check before constructing a Rust String.
      let Some(bytes) = js.to_string_as_bytes() else {
        return;
      };
      let Some(body) = bounded_utf8_bytes(bytes.as_ref(), IPC_PAYLOAD_LIMIT) else {
        return;
      };
      let Ok(request) = Request::builder().uri(uri).body(body) else {
        return;
      };
      ipc_handler(request);
    });

    // A page-world registration would let hostile JavaScript bypass the
    // pre-allocation listener and force WebKit to materialize attacker-sized
    // GBytes in the UI process. Only the isolated world owns the handler.
    if !manager.register_script_message_handler_in_world(IPC_HANDLER_NAME, IPC_WORLD_NAME) {
      return Err(Error::InitScriptError);
    }
    manager.add_script(&UserScript::for_world(
      &ipc_isolated_bridge_script(),
      UserContentInjectedFrames::TopFrame,
      UserScriptInjectionTime::Start,
      IPC_WORLD_NAME,
      &[],
      &[],
    ));
    Ok(())
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  fn attach_inspector_handlers(webview: &WebView) -> Arc<AtomicBool> {
    let is_inspector_open = Arc::new(AtomicBool::default());
    if let Some(inspector) = webview.inspector() {
      let is_inspector_open_ = is_inspector_open.clone();
      inspector.connect_bring_to_front(move |_| {
        is_inspector_open_.store(true, Ordering::Relaxed);
        false
      });
      let is_inspector_open_ = is_inspector_open.clone();
      inspector.connect_closed(move |_| {
        is_inspector_open_.store(false, Ordering::Relaxed);
      });
    }
    is_inspector_open
  }

  pub fn id(&self) -> crate::WebViewId<'_> {
    &self.id
  }

  pub fn print(&self) -> Result<()> {
    let print = webkit2gtk::PrintOperation::new(&self.webview);
    print.run_dialog(None::<&gtk::Window>);
    Ok(())
  }

  pub fn url(&self) -> Result<String> {
    Ok(bounded_webview_uri(&self.webview).unwrap_or_default())
  }

  pub fn document_title(&self) -> Result<Option<String>> {
    Ok(bounded_webview_title(&self.webview))
  }

  pub fn eval(
    &self,
    js: &str,
    callback: Option<impl FnOnce(String) + Send + 'static>,
  ) -> Result<()> {
    {
      let mut pending_scripts = self
        .pending_scripts
        .lock()
        .map_err(|_| Error::GtkStatePoisoned("pending scripts"))?;
      if let Some(pending_scripts) = pending_scripts.as_mut() {
        pending_scripts.push(js.into());
        return Ok(());
      }
    }

    let cancellable: Option<&Cancellable> = None;

    #[cfg(feature = "tracing")]
    let span = tracing::debug_span!("wry::eval");

    self
      .webview
      .evaluate_javascript(js, None, None, cancellable, |result| {
        #[cfg(feature = "tracing")]
        let _span = span.enter();

        if let Some(callback) = callback {
          let result = result
            .map(|js| js.to_json(0))
            .unwrap_or_default()
            .unwrap_or_default()
            .to_string();

          callback(result);
        }
      });

    Ok(())
  }

  fn init(&self, js: &str, for_main_only: bool) -> Result<()> {
    if let Some(manager) = self.webview.user_content_manager() {
      let script = UserScript::new(
        js,
        if for_main_only {
          UserContentInjectedFrames::TopFrame
        } else {
          UserContentInjectedFrames::AllFrames
        },
        UserScriptInjectionTime::Start,
        &[],
        &[],
      );
      manager.add_script(&script);
    } else {
      return Err(Error::InitScriptError);
    }
    Ok(())
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn open_devtools(&self) {
    if let Some(inspector) = self.webview.inspector() {
      inspector.show();
      // `bring-to-front` is not received in this case
      self.is_inspector_open.store(true, Ordering::Relaxed);
    }
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn close_devtools(&self) {
    if let Some(inspector) = self.webview.inspector() {
      inspector.close();
    }
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn is_devtools_open(&self) -> bool {
    self.is_inspector_open.load(Ordering::Relaxed)
  }

  pub fn zoom(&self, scale_factor: f64) -> Result<()> {
    self.webview.set_zoom_level(scale_factor);
    Ok(())
  }

  pub fn set_background_color(&self, (red, green, blue, alpha): RGBA) -> Result<()> {
    self.webview.set_background_color(&gtk::gdk::RGBA::new(
      red as f64 / 255.0,
      green as f64 / 255.0,
      blue as f64 / 255.0,
      alpha as f64 / 255.0,
    ));
    Ok(())
  }

  pub fn load_url(&self, url: &str) -> Result<()> {
    self.webview.load_uri(url);
    Ok(())
  }

  pub fn load_url_with_headers(&self, url: &str, headers: http::HeaderMap) -> Result<()> {
    let req = URIRequest::builder().uri(url).build();

    if let Some(ref mut req_headers) = req.http_headers() {
      for (header, value) in headers.iter() {
        req_headers.append(
          header.to_string().as_str(),
          value.to_str().unwrap_or_default(),
        );
      }
    }

    self.webview.load_request(&req);

    Ok(())
  }

  pub fn load_html(&self, html: &str) -> Result<()> {
    self.webview.load_html(html, None);
    Ok(())
  }

  pub fn reload(&self) -> Result<()> {
    self.webview.reload();
    Ok(())
  }

  pub fn go_forward(&self) -> Result<()> {
    self.webview.go_forward();
    Ok(())
  }

  pub fn go_back(&self) -> Result<()> {
    self.webview.go_back();
    Ok(())
  }

  pub fn can_go_forward(&self) -> Result<bool> {
    Ok(self.webview.can_go_forward())
  }

  pub fn can_go_back(&self) -> Result<bool> {
    Ok(self.webview.can_go_back())
  }

  pub fn clear_all_browsing_data(&self) -> Result<()> {
    if let Some(context) = self.webview.context() {
      if let Some(data_manger) = context.website_data_manager() {
        data_manger.clear(
          webkit2gtk::WebsiteDataTypes::ALL,
          gtk::glib::TimeSpan::from_seconds(0),
          None::<&Cancellable>,
          |_| {},
        );
      }
    }

    Ok(())
  }

  pub fn bounds(&self) -> Result<Rect> {
    let mut bounds = Rect::default();

    #[cfg(feature = "x11")]
    if let Some(x11_data) = &self.x11 {
      unsafe {
        let attributes: XWindowAttributes = std::mem::zeroed();
        let mut attributes = std::mem::MaybeUninit::new(attributes).assume_init();

        let ok = (x11_data.xlib.XGetWindowAttributes)(
          x11_data.x11_display as _,
          x11_data.x11_window,
          &mut attributes,
        );

        if ok != 0 {
          bounds.position = LogicalPosition::new(attributes.x, attributes.y).into();
          bounds.size = LogicalSize::new(attributes.width, attributes.height).into();
        }
      }
      return Ok(bounds);
    }

    let (size, _) = self.webview.allocated_size();
    bounds.size = LogicalSize::new(size.width(), size.height()).into();

    Ok(bounds)
  }

  pub fn set_bounds(&self, bounds: Rect) -> Result<()> {
    let scale_factor = self.webview.scale_factor() as f64;
    let (width, height) = bounds.size.to_logical::<i32>(scale_factor).into();
    let (x, y) = bounds.position.to_logical::<i32>(scale_factor).into();

    #[cfg(feature = "x11")]
    if let Some(x11_data) = &self.x11 {
      let window = &x11_data.gtk_window;
      window.move_(x, y);
      if let Some(window) = window.window() {
        window.resize(width, height);
      }
      window.size_allocate(&gtk::Allocation::new(0, 0, width, height));
    }

    if self.is_in_fixed_parent.get() {
      self
        .webview
        .size_allocate(&gtk::Allocation::new(x, y, width, height));
    }

    Ok(())
  }

  #[cfg(feature = "x11")]
  fn set_visible_x11(&self, visible: bool) {
    if let Some(x11_data) = &self.x11 {
      if x11_data.is_child {
        if visible {
          unsafe { (x11_data.xlib.XMapWindow)(x11_data.x11_display as _, x11_data.x11_window) };
        } else {
          unsafe { (x11_data.xlib.XUnmapWindow)(x11_data.x11_display as _, x11_data.x11_window) };
        }
      }
    }
  }

  #[cfg(feature = "x11")]
  fn set_visible_gtk(&self, visible: bool) {
    if let Some(x11_data) = &self.x11 {
      if x11_data.is_child {
        if visible {
          x11_data.gtk_window.show_all();
        } else {
          x11_data.gtk_window.hide();
        }
      }
    }
  }

  pub fn set_visible(&self, visible: bool) -> Result<()> {
    #[cfg(feature = "x11")]
    self.set_visible_x11(visible);

    if visible {
      self.webview.show_all();
    } else {
      self.webview.hide();
    }

    #[cfg(feature = "x11")]
    self.set_visible_gtk(visible);

    Ok(())
  }

  pub fn focus(&self) -> Result<()> {
    self.webview.grab_focus();
    Ok(())
  }

  pub fn focus_parent(&self) -> Result<()> {
    if let Some(window) = self.webview.parent_window() {
      window.focus(gdk::ffi::GDK_CURRENT_TIME.try_into().unwrap_or(0));
    }

    Ok(())
  }

  fn cookie_from_soup_cookie(mut cookie: soup::Cookie) -> Option<cookie::Cookie<'static>> {
    let name = cookie.name()?.to_string();
    let value = cookie.value()?.to_string();
    if !is_valid_cookie_name(&name) || value.contains('\0') {
      return None;
    }

    let mut cookie_builder = cookie::CookieBuilder::new(name, value);

    if let Some(domain) = cookie.domain().map(|n| n.to_string()) {
      cookie_builder = cookie_builder.domain(domain);
    }

    if let Some(path) = cookie.path().map(|n| n.to_string()) {
      cookie_builder = cookie_builder.path(path);
    }

    let http_only = cookie.is_http_only();
    cookie_builder = cookie_builder.http_only(http_only);

    let secure = cookie.is_secure();
    cookie_builder = cookie_builder.secure(secure);

    let same_site = cookie.same_site_policy();
    let same_site = match same_site {
      soup::SameSitePolicy::Lax => cookie::SameSite::Lax,
      soup::SameSitePolicy::Strict => cookie::SameSite::Strict,
      soup::SameSitePolicy::None => cookie::SameSite::None,
      _ => cookie::SameSite::None,
    };
    cookie_builder = cookie_builder.same_site(same_site);

    let expires = cookie.expires();
    let expires = match expires {
      Some(datetime) => cookie::time::OffsetDateTime::from_unix_timestamp(datetime.to_unix())
        .ok()
        .map(cookie::Expiration::DateTime),
      None => Some(cookie::Expiration::Session),
    };
    if let Some(expires) = expires {
      cookie_builder = cookie_builder.expires(expires);
    }

    Some(cookie_builder.build())
  }

  fn cookie_into_soup_cookie(cookie: &cookie::Cookie<'_>) -> Result<soup::Cookie> {
    if !is_valid_cookie_name(cookie.name())
      || cookie.value().contains('\0')
      || cookie.domain().is_some_and(|value| value.contains('\0'))
      || cookie.path().is_some_and(|value| value.contains('\0'))
    {
      return Err(Error::InvalidCookie);
    }

    let mut soup_cookie = soup::Cookie::new(
      cookie.name(),
      cookie.value(),
      cookie.domain().unwrap_or(""),
      cookie.path().unwrap_or(""),
      cookie
        .max_age()
        .map(|d| d.whole_seconds().clamp(i32::MIN as i64, i32::MAX as i64) as i32)
        .unwrap_or(-1),
    );

    if let Some(dt) = cookie.expires_datetime() {
      if let Ok(expires) = gtk::glib::DateTime::from_unix_utc(dt.unix_timestamp()) {
        soup_cookie.set_expires(&expires);
      }
    }

    if let Some(http_only) = cookie.http_only() {
      soup_cookie.set_http_only(http_only);
    }

    if let Some(same_site) = cookie.same_site() {
      soup_cookie.set_same_site_policy(match same_site {
        cookie::SameSite::Lax => soup::SameSitePolicy::Lax,
        cookie::SameSite::Strict => soup::SameSitePolicy::Strict,
        cookie::SameSite::None => soup::SameSitePolicy::None,
      });
    }

    if let Some(secure) = cookie.secure() {
      soup_cookie.set_secure(secure);
    }

    Ok(soup_cookie)
  }

  fn wait_for_native_callback<T>(
    receiver: std::sync::mpsc::Receiver<T>,
    operation: &'static str,
  ) -> Result<T> {
    let deadline = Instant::now() + NATIVE_OPERATION_TIMEOUT;
    loop {
      // Never block the GTK thread while waiting for the completion that the
      // same main context must deliver.
      gtk::main_iteration_do(false);
      match receiver.try_recv() {
        Ok(response) => return Ok(response),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
          return Err(Error::GtkCompletionDropped(operation));
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
      }
      if Instant::now() >= deadline {
        return Err(Error::GtkOperationTimedOut(operation));
      }
      std::thread::sleep(Duration::from_millis(1));
    }
  }

  pub fn cookies_for_url(&self, url: &str) -> Result<Vec<cookie::Cookie<'static>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let cookies_manager = self
      .webview
      .website_data_manager()
      .and_then(|manager| manager.cookie_manager())
      .ok_or(Error::MissingManager)?;
    cookies_manager.cookies(url, None::<&Cancellable>, move |cookies| {
      let cookies = cookies.map(|cookies| {
        cookies
          .into_iter()
          .filter_map(Self::cookie_from_soup_cookie)
          .collect()
      });
      let _ = tx.send(cookies);
    });

    Self::wait_for_native_callback(rx, "cookies-for-url")?.map_err(Into::into)
  }

  pub fn cookies(&self) -> Result<Vec<cookie::Cookie<'static>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let cookies_manager = self
      .webview
      .website_data_manager()
      .and_then(|manager| manager.cookie_manager())
      .ok_or(Error::MissingManager)?;
    cookies_manager.all_cookies(None::<&Cancellable>, move |cookies| {
      let cookies = cookies.map(|cookies| {
        cookies
          .into_iter()
          .filter_map(Self::cookie_from_soup_cookie)
          .collect()
      });
      let _ = tx.send(cookies);
    })?;

    Self::wait_for_native_callback(rx, "all-cookies")?.map_err(Into::into)
  }

  pub fn set_cookie(&self, cookie: &cookie::Cookie<'_>) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let cookies_manager = self
      .webview
      .website_data_manager()
      .and_then(|manager| manager.cookie_manager())
      .ok_or(Error::MissingManager)?;
    let mut soup_cookie = Self::cookie_into_soup_cookie(cookie)?;
    cookies_manager.add_cookie(&mut soup_cookie, None::<&Cancellable>, move |ret| {
      let _ = tx.send(ret);
    });

    Self::wait_for_native_callback(rx, "set-cookie")?.map_err(Into::into)
  }

  pub fn delete_cookie(&self, cookie: &cookie::Cookie<'_>) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let cookies_manager = self
      .webview
      .website_data_manager()
      .and_then(|manager| manager.cookie_manager())
      .ok_or(Error::MissingManager)?;
    let mut soup_cookie = Self::cookie_into_soup_cookie(cookie)?;
    cookies_manager.delete_cookie(&mut soup_cookie, None::<&Cancellable>, move |ret| {
      let _ = tx.send(ret);
    });

    Self::wait_for_native_callback(rx, "delete-cookie")?.map_err(Into::into)
  }

  pub fn reparent<W>(&self, container: &W) -> Result<()>
  where
    W: gtk::prelude::IsA<gtk::Container>,
  {
    let destination = if container.dynamic_cast_ref::<gtk::Box>().is_some() {
      GtkParentKind::Box
    } else if container.dynamic_cast_ref::<gtk::Fixed>().is_some() {
      GtkParentKind::Fixed
    } else {
      GtkParentKind::Generic
    };
    let Some(parent_widget) = self.webview.parent() else {
      self.is_in_fixed_parent.set(false);
      return Err(Error::GtkReparentFailed);
    };
    let actual_was_fixed = parent_widget.dynamic_cast_ref::<gtk::Fixed>().is_some();
    let Ok(parent) = parent_widget.dynamic_cast::<gtk::Container>() else {
      self.is_in_fixed_parent.set(actual_was_fixed);
      return Err(Error::GtkReparentFailed);
    };
    // GTK container operations may synchronously emit signals. Publish the
    // detached mode before removal, then the target mode before insertion so
    // reentrant bounds updates never use stale geometry semantics.
    self.is_in_fixed_parent.set(false);
    parent.remove(&self.webview);

    match destination {
      GtkParentKind::Box => {
        let container = container
          .dynamic_cast_ref::<gtk::Box>()
          .ok_or(Error::GtkReparentFailed)?;
        container.pack_start(&self.webview, true, true, 0);
      }
      GtkParentKind::Fixed => {
        let container = container
          .dynamic_cast_ref::<gtk::Fixed>()
          .ok_or(Error::GtkReparentFailed)?;
        self.is_in_fixed_parent.set(true);
        container.put(&self.webview, 0, 0);
      }
      GtkParentKind::Generic => {
        container.add(&self.webview);
      }
    }

    let expected_container: &gtk::Container = container.as_ref();
    let expected_parent: &gtk::Widget = expected_container.upcast_ref();
    if self.webview.parent().as_ref() == Some(expected_parent) {
      return Ok(());
    }

    let actual_is_fixed = self
      .webview
      .parent()
      .as_ref()
      .and_then(|parent| parent.dynamic_cast_ref::<gtk::Fixed>())
      .is_some();
    self.is_in_fixed_parent.set(actual_is_fixed);
    Err(Error::GtkReparentFailed)
  }
}

#[derive(Clone, Copy)]
enum GtkParentKind {
  Box,
  Fixed,
  Generic,
}

#[cfg(test)]
mod native_reparent_tests {
  use super::*;
  use crate::{WebViewBuilder, WebViewBuilderExtUnix as _};
  use gtk::subclass::prelude::*;

  mod test_fixed {
    use super::*;
    use gtk::glib;

    mod imp {
      use super::*;

      #[derive(Default)]
      pub struct TestFixed;

      #[glib::object_subclass]
      impl ObjectSubclass for TestFixed {
        const NAME: &'static str = "WryNativeReparentTestFixed";
        type Type = super::TestFixed;
        type ParentType = gtk::Fixed;
      }

      impl ObjectImpl for TestFixed {}
      impl WidgetImpl for TestFixed {}
      impl ContainerImpl for TestFixed {}
      impl FixedImpl for TestFixed {}
    }

    glib::wrapper! {
      pub struct TestFixed(ObjectSubclass<imp::TestFixed>)
          @extends gtk::Fixed, gtk::Container, gtk::Widget;
    }

    impl TestFixed {
      pub(super) fn new() -> Self {
        glib::Object::new()
      }
    }
  }

  pub(super) fn assert_native_invariants() {
    let mut context = WebContext::new_ephemeral().expect("secure ephemeral WebContext");
    let first_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let view = WebViewBuilder::new_with_web_context(&mut context)
      .build_gtk(&first_box)
      .expect("WebView in GtkBox");
    assert!(!view.webview.is_in_fixed_parent.get());

    let fixed = test_fixed::TestFixed::new();
    view
      .webview
      .reparent(&fixed)
      .expect("reparent into GtkFixed subclass");
    assert!(view.webview.is_in_fixed_parent.get());
    view
      .set_bounds(Rect {
        position: dpi::LogicalPosition::new(41, 43).into(),
        size: dpi::LogicalSize::new(211, 127).into(),
      })
      .expect("fixed-parent bounds");
    let allocation = view.webview.webview.allocation();
    assert_eq!(
      (
        allocation.x(),
        allocation.y(),
        allocation.width(),
        allocation.height()
      ),
      (41, 43, 211, 127)
    );

    let second_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    view
      .webview
      .reparent(&second_box)
      .expect("reparent back into GtkBox");
    assert!(!view.webview.is_in_fixed_parent.get());
    view
      .webview
      .reparent(&fixed)
      .expect("reuse GtkFixed subclass");
    assert!(view.webview.is_in_fixed_parent.get());
  }
}

#[cfg(test)]
pub(crate) fn assert_native_gtk_reparent_invariants() {
  native_reparent_tests::assert_native_invariants();
}

pub fn platform_webview_version() -> Result<String> {
  let (major, minor, patch) = unsafe {
    (
      webkit_get_major_version(),
      webkit_get_minor_version(),
      webkit_get_micro_version(),
    )
  };
  Ok(format!("{major}.{minor}.{patch}"))
}

#[cfg(feature = "x11")]
fn scale_factor_from_x11(xlib: &Xlib, display: *mut _XDisplay, parent: c_ulong) -> f64 {
  const BASE_DPI: f64 = 96.0;
  if display.is_null() || parent == 0 {
    return 1.0;
  }
  let mut attrs = std::mem::MaybeUninit::<XWindowAttributes>::zeroed();
  if unsafe { (xlib.XGetWindowAttributes)(display, parent, attrs.as_mut_ptr()) } == 0 {
    return 1.0;
  }
  let attrs = unsafe { attrs.assume_init() };
  if attrs.screen.is_null() {
    return 1.0;
  }
  let screen = unsafe { &*attrs.screen };
  if screen.mwidth <= 0 || screen.width <= 0 {
    return 1.0;
  }
  let scale_factor = screen.width as f64 * 25.4 / screen.mwidth as f64 / BASE_DPI;
  if scale_factor.is_finite() && scale_factor > 0.0 {
    scale_factor
  } else {
    1.0
  }
}

fn is_valid_cookie_name(name: &str) -> bool {
  !name.is_empty()
    && name.bytes().all(|byte| {
      byte.is_ascii()
        && !byte.is_ascii_control()
        && !matches!(
          byte,
          b' '
            | b'\t'
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'@'
            | b','
            | b';'
            | b':'
            | b'\\'
            | b'"'
            | b'/'
            | b'['
            | b']'
            | b'?'
            | b'='
            | b'{'
            | b'}'
        )
    })
}

mod ffi {
  use gtk::{
    gdk,
    gio::{
      self,
      ffi::{GAsyncReadyCallback, GCancellable},
      prelude::*,
      Cancellable,
    },
    glib::{
      self,
      translate::{FromGlibPtrContainer, ToGlibPtr},
    },
  };
  use webkit2gtk::CookieManager;
  use webkit2gtk_sys::WebKitCookieManager;

  pub trait CookieManageExt: IsA<CookieManager> + 'static {
    fn all_cookies<P: FnOnce(std::result::Result<Vec<soup::Cookie>, glib::Error>) + 'static>(
      &self,
      cancellable: Option<&impl IsA<Cancellable>>,
      callback: P,
    ) -> crate::Result<()> {
      let main_context = glib::MainContext::ref_thread_default();
      let is_main_context_owner = main_context.is_owner();
      let has_acquired_main_context = (!is_main_context_owner)
        .then(|| main_context.acquire().ok())
        .flatten();
      if !is_main_context_owner && has_acquired_main_context.is_none() {
        return Err(crate::Error::NotMainThread);
      }

      let user_data: Box<glib::thread_guard::ThreadGuard<P>> =
        Box::new(glib::thread_guard::ThreadGuard::new(callback));
      unsafe extern "C" fn cookies_trampoline<
        P: FnOnce(std::result::Result<Vec<soup::Cookie>, glib::Error>) + 'static,
      >(
        _source_object: *mut glib::gobject_ffi::GObject,
        res: *mut gdk::gio::ffi::GAsyncResult,
        user_data: glib::ffi::gpointer,
      ) {
        if user_data.is_null() {
          return;
        }
        let mut error = std::ptr::null_mut();
        let result = if _source_object.is_null() || res.is_null() {
          Err(glib::Error::new(
            glib::FileError::Failed,
            "WebKitGTK returned an incomplete all-cookies callback",
          ))
        } else {
          let ret =
            webkit_cookie_manager_get_all_cookies_finish(_source_object as *mut _, res, &mut error);
          if error.is_null() {
            Ok(FromGlibPtrContainer::from_glib_full(ret))
          } else {
            Err(glib::translate::from_glib_full(error))
          }
        };
        let callback: Box<glib::thread_guard::ThreadGuard<P>> = Box::from_raw(user_data as *mut _);
        let callback: P = callback.into_inner();
        callback(result);
      }
      let callback = cookies_trampoline::<P>;

      unsafe {
        webkit_cookie_manager_get_all_cookies(
          self.as_ref().to_glib_none().0,
          cancellable.map(|p| p.as_ref()).to_glib_none().0,
          Some(callback),
          Box::into_raw(user_data) as *mut _,
        );
      }
      Ok(())
    }
  }

  impl CookieManageExt for CookieManager {}

  extern "C" {
    pub fn webkit_cookie_manager_get_all_cookies(
      cookie_manager: *mut webkit2gtk_sys::WebKitCookieManager,
      cancellable: *mut GCancellable,
      callback: GAsyncReadyCallback,
      user_data: glib::ffi::gpointer,
    );

    pub fn webkit_cookie_manager_get_all_cookies_finish(
      cookie_manager: *mut WebKitCookieManager,
      result: *mut gio::ffi::GAsyncResult,
      error: *mut *mut glib::ffi::GError,
    ) -> *mut glib::ffi::GList;
  }
}
