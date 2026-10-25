// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Unix platform extensions for [`WebContext`](super::WebContext).

use crate::{
  native_admission::{
    InFlightAdmission, CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT, CUSTOM_PROTOCOL_OVERFLOW_STATUS,
  },
  native_bounds::{
    bounded_utf8_c_string, CustomProtocolRequestBudget, CUSTOM_PROTOCOL_METHOD_LIMIT,
    PAGE_URL_LIMIT,
  },
  DownloadPolicy, Error, RequestAsyncResponder,
};
use gtk::glib::{self, MainContext, ObjectExt, ObjectType};
use http::{
  header::CONTENT_TYPE, HeaderName, HeaderValue, Request, Response as HttpResponse, StatusCode,
};
use soup::{MessageHeaders, MessageHeadersType};
use std::{
  borrow::Cow,
  cell::{Cell, RefCell},
  env::current_dir,
  path::{Path, PathBuf},
  rc::Rc,
};
use webkit2gtk::{
  ApplicationInfo, AutomationSessionExt, CookiePersistentStorage, DownloadExt, SecurityManagerExt,
  URIRequest, URIRequestExt, URISchemeRequest, URISchemeRequestExt, URISchemeResponse,
  URISchemeResponseExt, WebContext, WebContextExt as Webkit2gtkContextExt, WebView, WebViewExt,
};
use webkit2gtk_sys::{
  webkit_uri_request_get_uri, webkit_uri_scheme_request_get_http_method,
  webkit_uri_scheme_request_get_uri,
};

fn bounded_request_uri(request: &URIRequest) -> Option<String> {
  // SAFETY: WebKit owns the returned pointer while the request is alive. The
  // helper scans only through the configured limit plus its terminator byte.
  unsafe { bounded_utf8_c_string(webkit_uri_request_get_uri(request.as_ptr()), PAGE_URL_LIMIT) }
}

fn bounded_scheme_request_uri(request: &URISchemeRequest) -> Option<String> {
  // SAFETY: identical borrowed-pointer contract to `bounded_request_uri`.
  unsafe {
    bounded_utf8_c_string(
      webkit_uri_scheme_request_get_uri(request.as_ptr()),
      PAGE_URL_LIMIT,
    )
  }
}

fn finish_scheme_request_with_empty_status(request: &URISchemeRequest, status: StatusCode) {
  let input = gtk::gio::MemoryInputStream::new();
  let response = URISchemeResponse::new(&input, 0);
  response.set_status(status.as_u16() as u32, None);
  request.finish_with_response(&response);
}

#[derive(Debug)]
pub struct WebContextImpl {
  context: WebContext,
  automation: bool,
  app_info: Option<ApplicationInfo>,
  custom_protocol_admission: InFlightAdmission,
}

impl WebContextImpl {
  pub fn new(data_directory: Option<&Path>) -> Self {
    use webkit2gtk::{CookieManagerExt, WebsiteDataManager, WebsiteDataManagerExt};
    let Some(data_directory) = data_directory else {
      return Self::create_ephemeral();
    };
    // Zephium security patch: this property is construct-only and defaults to
    // false in WebKit2GTK 4.1. Every persistent context must opt into
    // a fresh Web process when the top-level site changes.
    let mut context_builder =
      WebContext::builder().process_swap_on_cross_site_navigation_enabled(true);
    let data_manager = WebsiteDataManager::builder()
      // TODO: Consider taking a cache_directory so this can be in XDG_CACHE_HOME.
      .base_cache_directory(data_directory.to_string_lossy())
      .base_data_directory(data_directory.to_string_lossy())
      .build();
    if let Some(cookie_manager) = data_manager.cookie_manager() {
      cookie_manager.set_persistent_storage(
        &data_directory.join("cookies").to_string_lossy(),
        CookiePersistentStorage::Text,
      );
    }
    context_builder = context_builder.website_data_manager(&data_manager);
    let context = context_builder.build();

    Self::create_context(context)
  }

  pub fn new_ephemeral() -> crate::Result<Self> {
    let context = Self::create_ephemeral();
    context.validate_security()?;
    Ok(context)
  }

  fn create_ephemeral() -> Self {
    // `WebContext::new_ephemeral()` offers no builder hook for construct-only
    // process policy. Recreate its documented shape with an ephemeral data
    // manager and the mandatory process-swap property in one construction.
    let data_manager = webkit2gtk::WebsiteDataManager::new_ephemeral();
    let context = WebContext::builder()
      .process_swap_on_cross_site_navigation_enabled(true)
      .website_data_manager(&data_manager)
      .build();

    Self::create_context(context)
  }

  pub fn create_context(context: WebContext) -> Self {
    // Zephium security patch: enable the Web-process sandbox while the
    // context cannot yet own a WebView or have launched a Web process. A
    // post-build toggle is too late to be a reliable construction invariant.
    if !context.is_sandbox_enabled() && std::env::var_os("ZEPHIUM_ALLOW_SYSTEM_WEBKITGTK").is_none() {
      context.set_sandbox_enabled(true);
    }
    let automation = false;
    context.set_automation_allowed(automation);

    // e.g. wry 0.9.4
    let app_info = ApplicationInfo::new();
    app_info.set_name(env!("CARGO_PKG_NAME"));
    app_info.set_version(
      env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or_default(),
      env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or_default(),
      env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or_default(),
    );

    Self {
      context,
      automation,
      app_info: Some(app_info),
      custom_protocol_admission: InFlightAdmission::new(CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT),
    }
  }

  pub(crate) fn validate_security(&self) -> crate::Result<()> {
    validate_context_security(
      self.context.is_sandbox_enabled(),
      self
        .context
        .is_process_swap_on_cross_site_navigation_enabled(),
    )
  }

  pub fn set_allows_automation(&mut self, flag: bool) {
    self.automation = flag;
    self.context.set_automation_allowed(flag);
  }

  pub fn set_web_extensions_directory(&mut self, path: &Path) {
    self
      .context
      .set_web_extensions_directory(&path.to_string_lossy());
  }
}

impl Drop for WebContextImpl {
  fn drop(&mut self) {
    self.custom_protocol_admission.seal_and_drain();
  }
}

fn validate_context_security(sandbox: bool, process_swap: bool) -> crate::Result<()> {
  if !sandbox && std::env::var_os("ZEPHIUM_ALLOW_SYSTEM_WEBKITGTK").is_none() {
    return Err(Error::GtkSandboxUnavailable);
  }
  if !process_swap {
    return Err(Error::GtkProcessSwapUnavailable);
  }
  Ok(())
}

#[cfg(test)]
mod download_filename_tests {
  use super::*;

  #[test]
  fn mandatory_context_postconditions_are_fallible() {
    assert!(validate_context_security(true, true).is_ok());
    assert!(matches!(
      validate_context_security(false, true),
      Err(Error::GtkSandboxUnavailable)
    ));
    assert!(matches!(
      validate_context_security(true, false),
      Err(Error::GtkProcessSwapUnavailable)
    ));
  }
}

/// [`WebContext`](super::WebContext) items that only matter on unix.
pub trait WebContextExt {
  /// The GTK [`WebContext`] of all webviews in the context.
  fn context(&self) -> &WebContext;

  /// Register a custom protocol to the web context.
  fn register_uri_scheme<F>(&mut self, name: &str, handler: F) -> crate::Result<()>
  where
    F: Fn(crate::WebViewId, Request<Vec<u8>>, RequestAsyncResponder) + 'static;

  /// Loads a URI for a [`WebView`].
  fn load_uri(&self, webview: WebView, url: String, headers: Option<http::HeaderMap>);

  /// If the context allows automation.
  ///
  /// **Note:** `libwebkit2gtk` only allows 1 automation context at a time.
  fn allows_automation(&self) -> bool;

  fn register_automation(&mut self, webview: WebView);

  fn register_download_handler(
    &mut self,
    policy: DownloadPolicy,
    download_started_callback: Option<Box<dyn FnMut(String, &mut PathBuf) -> bool>>,
    download_completed_callback: Option<Rc<dyn Fn(String, Option<PathBuf>, bool) + 'static>>,
  );
}

impl WebContextExt for super::WebContext {
  fn context(&self) -> &WebContext {
    &self.os.context
  }

  fn register_uri_scheme<F>(&mut self, name: &str, handler: F) -> crate::Result<()>
  where
    F: Fn(crate::WebViewId, Request<Vec<u8>>, RequestAsyncResponder) + 'static,
  {
    self.register_custom_protocol(name.to_owned())?;

    // Enable secure context
    self
      .os
      .context
      .security_manager()
      .ok_or(Error::MissingManager)?
      .register_uri_scheme_as_secure(name);

    let protocol_admission = self.os.custom_protocol_admission.clone();
    self.os.context.register_uri_scheme(name, move |request| {
      #[cfg(feature = "tracing")]
      let span = tracing::info_span!(parent: None, "wry::custom_protocol::handle", uri = tracing::field::Empty).entered();

      if let Some(uri) = bounded_scheme_request_uri(&request) {

        let Some(permit) = protocol_admission.try_acquire() else {
          finish_scheme_request_with_empty_status(&request, CUSTOM_PROTOCOL_OVERFLOW_STATUS);
          return;
        };

        #[cfg(feature = "tracing")]
        span.record("uri", uri.as_str());

        #[allow(unused_mut)]
        let mut http_request = Request::builder().uri(uri.as_str()).method("GET");
        let mut budget = CustomProtocolRequestBudget::default();
        let mut request_within_budget = true;

        // Set request http headers
        if let Some(headers) = request.http_headers() {
          if let Some(map) = http_request.headers_mut() {
            headers.foreach(|k, v| {
              if !request_within_budget || !budget.admit_header(k.len(), v.len()) {
                request_within_budget = false;
                return;
              }
              if let Ok(name) = HeaderName::from_bytes(k.as_bytes()) {
                if let Ok(value) = HeaderValue::from_bytes(v.as_bytes()) {
                  map.insert(name, value);
                }
              }
            });
          }
        }
        if !request_within_budget {
          request.finish_error(&mut gtk::glib::Error::new(
            glib::UriError::Failed,
            "Custom protocol request headers exceed the allocation limit.",
          ));
          return;
        }

        // Set request http method
        // Use WebKit's borrowed pointer directly: the safe binding turns it
        // into a `GString` before callers can inspect its native length.
        let method = unsafe { webkit_uri_scheme_request_get_http_method(request.as_ptr()) };
        if !method.is_null() {
          // SAFETY: WebKit owns this NUL-terminated pointer for the duration of
          // the live request; the helper scans at most the configured bound.
          let Some(method) = (unsafe {
            bounded_utf8_c_string(method, CUSTOM_PROTOCOL_METHOD_LIMIT)
          }) else {
            request.finish_error(&mut gtk::glib::Error::new(
              glib::UriError::Failed,
              "Custom protocol request method exceeds the allocation limit.",
            ));
            return;
          };
          http_request = http_request.method(method.as_str());
        }

        #[allow(unused_mut)]
        let mut body = Vec::new();
        #[cfg(feature = "linux-body")]
        {
          use gtk::{gdk::prelude::InputStreamExtManual, gio::Cancellable};

          // Set request http body
          let cancellable: Option<&Cancellable> = None;
          if let Some(stream) = request.http_body() {
            let mut buffer = [0u8; 1_024];
            while let Ok(count) = stream.read(&mut buffer, cancellable) {
              if count == 0 {
                break;
              }
              if !budget.admit_body_chunk(count) {
                request_within_budget = false;
                break;
              }
              body.extend_from_slice(&buffer[..count]);
            }
          }
        }
        if !request_within_budget {
          request.finish_error(&mut gtk::glib::Error::new(
            glib::UriError::Failed,
            "Custom protocol request body exceeds the allocation limit.",
          ));
          return;
        }

        let http_request = match http_request.body(body) {
          Ok(req) => req,
          Err(_) => {
            request.finish_error(&mut gtk::glib::Error::new(
              glib::UriError::Failed,
              "Internal server error: could not create request.",
            ));
            return;
          }
        };

        let main_context = MainContext::ref_thread_default();
        #[allow(deprecated)]
        let (response_sender, response_receiver) =
          MainContext::channel::<HttpResponse<Cow<'static, [u8]>>>(glib::Priority::DEFAULT);
        let request_ = request.clone();
        response_receiver.attach(Some(&main_context), move |http_response| {
          // The source is removed after this single response. Keeping the
          // permit in the main-context closure covers the full native request,
          // including a response queued from a worker thread.
          let _permit = &permit;
          let buffer = http_response.body();
          let input = gtk::gio::MemoryInputStream::from_bytes(&gtk::glib::Bytes::from(buffer));
          let content_type = http_response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|h| h.to_str().ok());

          let response = URISchemeResponse::new(&input, buffer.len() as i64);
          response.set_status(http_response.status().as_u16() as u32, None);
          if let Some(content_type) = content_type {
            response.set_content_type(content_type);
          }

          let headers = MessageHeaders::new(MessageHeadersType::Response);
          for (name, value) in http_response.headers() {
            if let Ok(value) = value.to_str() {
              headers.append(name.as_str(), value);
            }
          }
          response.set_http_headers(headers);
          request_.finish_with_response(&response);
          glib::ControlFlow::Break
        });
        let responder: Box<dyn FnOnce(HttpResponse<Cow<'static, [u8]>>) + Send> =
          Box::new(move |http_response| {
            let _ = response_sender.send(http_response);
          });

        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!("wry::custom_protocol::call_handler").entered();

        let webview_id = request
          .web_view()
          .and_then(|w| unsafe { w.data::<String>(super::WEBVIEW_ID) })
          .map(|id| unsafe { id.as_ref().clone() })
          .unwrap_or_default();

        handler(
          &webview_id,
          http_request,
          RequestAsyncResponder {
            responder: Some(responder),
          },
        );
      } else {
        request.finish_error(&mut glib::Error::new(
          glib::FileError::Exist,
          "Could not get uri.",
        ));
      }
    });

    Ok(())
  }

  fn load_uri(&self, webview: WebView, uri: String, headers: Option<http::HeaderMap>) {
    if let Some(headers) = headers {
      let req = URIRequest::builder().uri(&uri).build();

      if let Some(ref mut req_headers) = req.http_headers() {
        for (header, value) in headers.iter() {
          req_headers.append(
            header.to_string().as_str(),
            value.to_str().unwrap_or_default(),
          );
        }
      }

      webview.load_request(&req);
    } else {
      webview.load_uri(&uri);
    }
  }

  fn allows_automation(&self) -> bool {
    self.os.automation
  }

  fn register_automation(&mut self, webview: WebView) {
    if let (true, Some(app_info)) = (self.os.automation, self.os.app_info.take()) {
      self.os.context.connect_automation_started(move |_, auto| {
        let webview = webview.clone();
        auto.set_application_info(&app_info);

        // We do **NOT** support arbitrarily creating new webviews.
        // To support this in the future, we would need a way to specify the
        // default WindowBuilder to use to create the window it will use, and
        // possibly "default" webview attributes. Difficulty comes in for controlling
        // the owned Window that would need to be used.
        //
        // Instead, we just pass the first created webview.
        auto.connect_create_web_view(None, move |_| webview.clone());
      });
    }
  }

  fn register_download_handler(
    &mut self,
    policy: DownloadPolicy,
    download_started_handler: Option<Box<dyn FnMut(String, &mut PathBuf) -> bool>>,
    download_completed_handler: Option<Rc<dyn Fn(String, Option<PathBuf>, bool) + 'static>>,
  ) {
    let context = &self.os.context;

    if policy.inspect_metadata(|| ()).is_none() {
      // This signal is installed before the WebView's first navigation. Do not
      // request URI, filename, destination, or completion metadata: the native
      // object is cancelled at the first construction-time interception point.
      context.connect_download_started(|_context, download| download.cancel());
      return;
    }

    let download_started_handler = Rc::new(RefCell::new(download_started_handler));
    context.connect_download_started(move |_context, download| {
      let failed = Rc::new(Cell::new(false));
      let download_started_handler = download_started_handler.clone();
      download.connect_decide_destination(move |download, suggested_filename| {
        let Some(uri) = download
          .request()
          .and_then(|request| bounded_request_uri(&request))
        else {
          download.cancel();
          return true;
        };
        let Ok(mut handler_slot) = download_started_handler.try_borrow_mut() else {
          // A re-entrant page callback must not panic the process or fall back
          // to WebKit's native download destination UI.
          download.cancel();
          return true;
        };
        let Some(download_started_handler) = handler_slot.as_mut() else {
          // Downloads are unbrokered without an explicit embedder callback.
          download.cancel();
          return true;
        };

        let filename = sanitize_download_filename(suggested_filename);
        let download_directory =
          dirs::download_dir().unwrap_or_else(|| current_dir().unwrap_or_default());
        let Some(mut download_destination) =
          available_download_path(&download_directory, &filename)
        else {
          download.cancel();
          return true;
        };

        if download_started_handler(uri, &mut download_destination) {
          download.set_destination(&download_destination.to_string_lossy());
        } else {
          download.cancel();
        }
        true
      });

      download.connect_failed({
        let failed = failed.clone();
        move |_, _error| {
          failed.set(true);
        }
      });

      if let Some(download_completed_handler) = download_completed_handler.clone() {
        download.connect_finished({
          let failed = failed.clone();
          move |download| {
            if let Some(uri) = download
              .request()
              .and_then(|request| bounded_request_uri(&request))
            {
              let failed = failed.get();
              download_completed_handler(
                uri,
                (!failed)
                  .then(|| download.destination().map(PathBuf::from))
                  .flatten(),
                !failed,
              )
            }
          }
        });
      }
    });
  }
}

const MAX_DOWNLOAD_COLLISIONS: u32 = 1_000;
const MAX_DOWNLOAD_FILENAME_CHARS: usize = 128;

fn sanitize_download_filename(suggested: &str) -> String {
  let mut filename: String = suggested
    .chars()
    .filter(|character| {
      !character.is_control()
        && !matches!(
          character,
          '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'
        )
    })
    .take(MAX_DOWNLOAD_FILENAME_CHARS)
    .collect();
  filename = filename.trim().trim_matches('.').to_owned();
  if filename.is_empty() || filename == "." || filename == ".." {
    "download".to_owned()
  } else {
    filename
  }
}

fn available_download_path(directory: &Path, filename: &str) -> Option<PathBuf> {
  let initial = directory.join(filename);
  if !initial.exists() {
    return Some(initial);
  }

  let filename_path = Path::new(filename);
  let stem = filename_path
    .file_stem()
    .and_then(|value| value.to_str())
    .filter(|value| !value.is_empty())
    .unwrap_or("download");
  let extension = filename_path
    .extension()
    .and_then(|value| value.to_str())
    .map(|value| format!(".{value}"))
    .unwrap_or_default();
  (1..=MAX_DOWNLOAD_COLLISIONS)
    .map(|counter| directory.join(format!("{stem} ({counter}){extension}")))
    .find(|candidate| !candidate.exists())
}

#[cfg(test)]
mod tests {
  use super::sanitize_download_filename;

  #[test]
  fn download_filename_cannot_escape_the_broker_directory() {
    assert_eq!(sanitize_download_filename("../../.ssh/id_rsa"), "sshid_rsa");
    assert_eq!(
      sanitize_download_filename("..\\..\\secret.txt"),
      "secret.txt"
    );
    assert_eq!(sanitize_download_filename("..."), "download");
  }
}
