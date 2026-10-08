// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! <p align="center"><img height="100" src="https://raw.githubusercontent.com/tauri-apps/wry/refs/heads/dev/.github/splash.png" alt="WRY Webview Rendering library" /></p>
//!
//! [![](https://img.shields.io/crates/v/wry?style=flat-square)](https://crates.io/crates/wry) [![](https://img.shields.io/docsrs/wry?style=flat-square)](https://docs.rs/wry/)
//! [![License](https://img.shields.io/badge/License-MIT%20or%20Apache%202-green.svg)](https://opencollective.com/tauri)
//! [![Chat Server](https://img.shields.io/badge/chat-discord-7289da.svg)](https://discord.gg/SpmNs4S)
//! [![website](https://img.shields.io/badge/website-tauri.app-purple.svg)](https://tauri.app)
//! [![https://good-labs.github.io/greater-good-affirmation/assets/images/badge.svg](https://good-labs.github.io/greater-good-affirmation/assets/images/badge.svg)](https://good-labs.github.io/greater-good-affirmation)
//! [![support](https://img.shields.io/badge/sponsor-Open%20Collective-blue.svg)](https://opencollective.com/tauri)
//!
//! Wry is a cross-platform WebView rendering library.
//!
//! The webview requires a running event loop and a window type that implements [`HasWindowHandle`],
//! or a gtk container widget if you need to support X11 and Wayland.
//! You can use a windowing library like [`tao`] or [`winit`].
//!
//! ## Examples
//!
//! This example leverages the [`HasWindowHandle`] and supports Windows, macOS, iOS, Android and Linux (X11 Only).
//! See the following example using [`winit`]:
//!
//! ```no_run
//! # use wry::{WebViewBuilder, raw_window_handle};
//! # use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, EventLoop}, window::{Window, WindowId}};
//! #[derive(Default)]
//! struct App {
//!   window: Option<Window>,
//!   webview: Option<wry::WebView>,
//! }
//!
//! impl ApplicationHandler for App {
//!   fn resumed(&mut self, event_loop: &ActiveEventLoop) {
//!     let window = event_loop.create_window(Window::default_attributes()).unwrap();
//!     let webview = WebViewBuilder::new()
//!       .with_url("https://tauri.app")
//!       .build(&window)
//!       .unwrap();
//!
//!     self.window = Some(window);
//!     self.webview = Some(webview);
//!   }
//!
//!   fn window_event(&mut self, _event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {}
//! }
//!
//! let event_loop = EventLoop::new().unwrap();
//! let mut app = App::default();
//! event_loop.run_app(&mut app).unwrap();
//! ```
//!
//! If you also want to support Wayland too, then we recommend you use [`WebViewBuilderExtUnix::new_gtk`] on Linux.
//! See the following example using [`tao`]:
//!
//! ```no_run
//! # use wry::WebViewBuilder;
//! # use tao::{window::WindowBuilder, event_loop::EventLoop};
//! # #[cfg(target_os = "linux")]
//! # use tao::platform::unix::WindowExtUnix;
//! # #[cfg(target_os = "linux")]
//! # use wry::WebViewBuilderExtUnix;
//! let event_loop = EventLoop::new();
//! let window = WindowBuilder::new().build(&event_loop).unwrap();
//!
//! let builder = WebViewBuilder::new().with_url("https://tauri.app");
//!
//! #[cfg(not(target_os = "linux"))]
//! let webview = builder.build(&window).unwrap();
//! #[cfg(target_os = "linux")]
//! let webview = builder.build_gtk(window.gtk_window()).unwrap();
//! ```
//!
//! ## Child webviews
//!
//! You can use [`WebViewBuilder::build_as_child`] to create the webview as a child inside another window. This is supported on
//! macOS, Windows and Linux (X11 Only).
//!
//! ```no_run
//! # use wry::{WebViewBuilder, raw_window_handle, Rect, dpi::*};
//! # use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, EventLoop}, window::{Window, WindowId}};
//! #[derive(Default)]
//! struct App {
//!   window: Option<Window>,
//!   webview: Option<wry::WebView>,
//! }
//!
//! impl ApplicationHandler for App {
//!   fn resumed(&mut self, event_loop: &ActiveEventLoop) {
//!     let window = event_loop.create_window(Window::default_attributes()).unwrap();
//!     let webview = WebViewBuilder::new()
//!       .with_url("https://tauri.app")
//!       .with_bounds(Rect {
//!         position: LogicalPosition::new(100, 100).into(),
//!         size: LogicalSize::new(200, 200).into(),
//!       })
//!       .build_as_child(&window)
//!       .unwrap();
//!
//!     self.window = Some(window);
//!     self.webview = Some(webview);
//!   }
//!
//!   fn window_event(&mut self, _event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {}
//! }
//!
//! let event_loop = EventLoop::new().unwrap();
//! let mut app = App::default();
//! event_loop.run_app(&mut app).unwrap();
//! ```
//!
//! If you want to support X11 and Wayland at the same time, we recommend using
//! [`WebViewExtUnix::new_gtk`] or [`WebViewBuilderExtUnix::new_gtk`] with [`gtk::Fixed`].
//!
//! ```no_run
//! # use wry::{WebViewBuilder, raw_window_handle, Rect, dpi::*};
//! # use tao::{window::WindowBuilder, event_loop::EventLoop};
//! # #[cfg(target_os = "linux")]
//! # use wry::WebViewBuilderExtUnix;
//! # #[cfg(target_os = "linux")]
//! # use tao::platform::unix::WindowExtUnix;
//! let event_loop = EventLoop::new();
//! let window = WindowBuilder::new().build(&event_loop).unwrap();
//!
//! let builder = WebViewBuilder::new()
//!   .with_url("https://tauri.app")
//!   .with_bounds(Rect {
//!     position: LogicalPosition::new(100, 100).into(),
//!     size: LogicalSize::new(200, 200).into(),
//!   });
//!
//! #[cfg(not(target_os = "linux"))]
//! let webview = builder.build_as_child(&window).unwrap();
//! #[cfg(target_os = "linux")]
//! let webview = {
//!   # use gtk::prelude::*;
//!   let vbox = window.default_vbox().unwrap(); // tao adds a gtk::Box by default
//!   let fixed = gtk::Fixed::new();
//!   fixed.show_all();
//!   vbox.pack_start(&fixed, true, true, 0);
//!   builder.build_gtk(&fixed).unwrap()
//! };
//! ```
//!
//! ## Platform Considerations
//!
//! Here is the underlying web engine each platform uses, and some dependencies you might need to install.
//!
//! ### Linux
//!
//! [WebKitGTK](https://webkitgtk.org/) is used to provide webviews on Linux which requires GTK,
//! so if the windowing library doesn't support GTK (as in [`winit`])
//! you'll need to call [`gtk::init`] before creating the webview and then call [`gtk::main_iteration_do`] alongside
//! your windowing library event loop.
//!
//! ```no_run
//! # use wry::{WebView, WebViewBuilder};
//! # use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, EventLoop}, window::{Window, WindowId}};
//! #[derive(Default)]
//! struct App {
//!   webview_window: Option<(Window, WebView)>,
//! }
//!
//! impl ApplicationHandler for App {
//!   fn resumed(&mut self, event_loop: &ActiveEventLoop) {
//!     let window = event_loop.create_window(Window::default_attributes()).unwrap();
//!     let webview = WebViewBuilder::new()
//!       .with_url("https://tauri.app")
//!       .build(&window)
//!       .unwrap();
//!
//!     self.webview_window = Some((window, webview));
//!   }
//!
//!   fn window_event(&mut self, _event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {}
//!
//!   // Advance GTK event loop <!----- IMPORTANT
//!   fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
//!     #[cfg(target_os = "linux")]
//!     while gtk::events_pending() {
//!       gtk::main_iteration_do(false);
//!     }
//!   }
//! }
//!
//! let event_loop = EventLoop::new().unwrap();
//! let mut app = App::default();
//! event_loop.run_app(&mut app).unwrap();
//! ```
//!
//! #### Linux Dependencies
//!
//! ##### Arch Linux / Manjaro:
//!
//! ```bash
//! sudo pacman -S webkit2gtk-4.1
//! ```
//!
//! ##### Debian / Ubuntu:
//!
//! ```bash
//! sudo apt install libwebkit2gtk-4.1-dev
//! ```
//!
//! ##### Fedora
//!
//! ```bash
//! sudo dnf install gtk3-devel webkit2gtk4.1-devel
//! ```
//!
//! ##### Nix & NixOS
//!
//! ```nix
//! # shell.nix
//!
//! let
//!    # Unstable Channel | Rolling Release
//!    pkgs = import (fetchTarball("channel:nixpkgs-unstable")) { };
//!    packages = with pkgs; [
//!      pkg-config
//!      webkitgtk_4_1
//!    ];
//!  in
//!  pkgs.mkShell {
//!    buildInputs = packages;
//!  }
//! ```
//!
//! ```sh
//! nix-shell shell.nix
//! ```
//!
//! ##### GUIX
//!
//! ```scheme
//! ;; manifest.scm
//!
//! (specifications->manifest
//!   '("pkg-config"                ; Helper tool used when compiling
//!     "webkitgtk"                 ; Web content engine fot GTK+
//!  ))
//! ```
//!
//! ```bash
//! guix shell -m manifest.scm
//! ```
//!
//! ### macOS
//!
//! WebKit is native on macOS so everything should be fine.
//!
//! If you are cross-compiling for macOS using [osxcross](https://github.com/tpoechtrager/osxcross) and encounter a runtime panic like `Class with name WKWebViewConfiguration could not be found` it's possible that `WebKit.framework` has not been linked correctly, to fix this set the `RUSTFLAGS` environment variable:
//!
//! ```bash
//! RUSTFLAGS="-l framework=WebKit" cargo build --target=x86_64-apple-darwin --release
//! ```
//!
//! ### Windows
//!
//! WebView2 provided by Microsoft Edge Chromium is used. So wry supports Windows 7, 8, 10 and 11.
//!
//! ### Android
//!
//! In order for `wry` to be able to create webviews on Android, there are a few requirements that your application needs to uphold:
//!
//! 1. You need to set a few environment variables that will be used to generate the necessary kotlin
//!    files that you need to include in your Android application for wry to function properly.
//!    - `WRY_ANDROID_PACKAGE`: which is the reversed domain name of your android project and the app name in snake_case, for example, `com.wry.example.wry_app`
//!    - `WRY_ANDROID_LIBRARY`: for example, if your cargo project has a lib name `wry_app`, it will generate `libwry_app.so` so you set this env var to `wry_app`
//!    - `WRY_ANDROID_KOTLIN_FILES_OUT_DIR`: for example, `path/to/app/src/main/kotlin/com/wry/example`
//! 2. Your main Android Activity needs to inherit `AppCompatActivity`, preferably it should use the generated `WryActivity` or inherit it.
//! 3. Your Rust app needs to call `wry::android_setup` function to setup the necessary logic to be able to create webviews later on.
//! 4. Your Rust app needs to call `wry::android_binding!` macro to setup the JNI functions that will be called by `WryActivity` and various other places.
//!
//! It is recommended to use the [`tao`](https://docs.rs/tao/latest/tao/) crate as it provides maximum compatibility with `wry`.
//!
//! ```
//! #[cfg(target_os = "android")]
//! {
//!   tao::android_binding!(
//!       com_example,
//!       wry_app,
//!       WryActivity,
//!       wry::android_setup, // pass the wry::android_setup function to tao which will be invoked when the event loop is created
//!       _start_app
//!   );
//!   wry::android_binding!(com_example, ttt);
//! }
//! ```
//!
//! If this feels overwhelming, you can just use the preconfigured template from [`cargo-mobile2`](https://github.com/tauri-apps/cargo-mobile2).
//!
//! For more information, check out [MOBILE.md](https://github.com/tauri-apps/wry/blob/dev/MOBILE.md).
//!
//! ## Feature flags
//!
//! Wry uses a set of feature flags to toggle several advanced features.
//!
//! - `os-webview` (default): Enables the default WebView framework on the platform. This must be enabled
//!   for the crate to work. This feature was added in preparation of other ports like cef and servo.
//! - `protocol` (default): Enables [`WebViewBuilder::with_custom_protocol`] to define custom URL scheme for handling tasks like
//!   loading assets.
//! - `devtools`: Enables devtools on release builds. Devtools are always enabled in debug builds.
//!   On **macOS**, enabling devtools, requires calling private APIs so you should not enable this flag in release
//!   build if your app needs to publish to App Store.
//! - `transparent`: Transparent background on **macOS** requires calling private functions.
//!   Avoid this in release build if your app needs to publish to App Store.
//! - `fullscreen`: Fullscreen video and other media on **macOS** requires calling private functions.
//!   Avoid this in release build if your app needs to publish to App Store.
//! - `linux-body`: Enables body support of custom protocol request on Linux. Requires
//!   WebKit2GTK v2.40 or above.
//! - `tracing`: enables [`tracing`] for `evaluate_script`, `ipc_handler`, and `custom_protocols`.
//!
//! ## Partners
//!
//! <table>
//!   <tbody>
//!     <tr>
//!       <td align="center" valign="middle">
//!         <a href="https://crabnebula.dev" target="_blank">
//!           <img src=".github/sponsors/crabnebula.svg" alt="CrabNebula" width="283">
//!         </a>
//!       </td>
//!     </tr>
//!   </tbody>
//! </table>
//!
//! For the complete list of sponsors please visit our [website](https://tauri.app#sponsors) and [Open Collective](https://opencollective.com/tauri).
//!
//! ## License
//!
//! Apache-2.0/MIT
//!
//! [`tao`]: https://docs.rs/tao
//! [`winit`]: https://docs.rs/winit
//! [`tracing`]: https://docs.rs/tracing

#![allow(clippy::new_without_default)]
#![allow(clippy::default_constructed_unit_structs)]
#![allow(clippy::type_complexity)]
#![cfg_attr(docsrs, feature(doc_cfg))]

// #[cfg(any(target_os = "macos", target_os = "ios"))]
// #[macro_use]
// extern crate objc;

#[cfg(any(target_os = "windows", target_os = "android"))]
mod custom_protocol_workaround;
mod error;
#[cfg(target_os = "macos")]
mod file_upload;
#[cfg(any(target_os = "android", test))]
mod inject_initialization_scripts;
mod native_admission;
mod native_bounds;
#[cfg(any(target_os = "windows", test))]
mod native_cleanup;
mod permissions;
#[cfg(target_os = "macos")]
pub use file_upload::{FileUploadRequest, FileUploadResponder};
mod proxy;
#[cfg(any(target_os = "macos", target_os = "android", target_os = "ios"))]
mod util;
mod web_context;

#[cfg(target_os = "android")]
pub(crate) mod android;
#[cfg(target_os = "android")]
pub use crate::android::android_setup;
#[cfg(target_os = "android")]
pub mod prelude {
  pub use crate::android::{binding::*, dispatch, find_class, Context};
  pub use tao_macros::{android_fn, generate_package_name};
}
#[cfg(target_os = "android")]
pub use android::JniHandle;
#[cfg(target_os = "android")]
use android::*;

#[cfg(gtk)]
pub(crate) mod webkitgtk;
/// Re-exported [raw-window-handle](https://docs.rs/raw-window-handle/latest/raw_window_handle/) crate.
pub use raw_window_handle;
use raw_window_handle::HasWindowHandle;
#[cfg(gtk)]
use webkitgtk::*;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindow;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use objc2_web_kit::WKUserContentController;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod wkwebview;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use wkwebview::*;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use wkwebview::{PrintMargin, PrintOptions, WryWebView};

#[cfg(target_os = "windows")]
pub(crate) mod webview2;
#[cfg(all(target_os = "windows", feature = "windows-cleanup-qualification"))]
pub use self::webview2::fail_next_webview2_controller_close_for_qualification;
#[cfg(target_os = "windows")]
use self::webview2::*;

#[cfg(target_os = "windows")]
pub use self::webview2::{
  pending_webview2_cleanup_debts, webview2_cleanup_overflowed, ScrollBarStyle, WebView2CleanupDebt,
  WebView2CleanupFailure, WebView2CleanupStep, WebView2ConstructionCleanupIncident,
};
#[cfg(target_os = "windows")]
use webview2_com::Microsoft::Web::WebView2::Win32::{
  ICoreWebView2, ICoreWebView2ContextMenuRequestedEventArgs, ICoreWebView2Controller,
  ICoreWebView2DownloadStartingEventArgs, ICoreWebView2Environment,
};

use std::{borrow::Cow, collections::HashMap, path::PathBuf, rc::Rc};

use http::{Request, Response};

pub use cookie;
pub use dpi;
pub use error::*;
pub use http;
pub use permissions::{
  PermissionKind, PermissionOrigin, PermissionRequest, PermissionRequestDisposition,
  PermissionRequestId, PermissionRequestKind, PermissionResponse,
};
pub use proxy::{ProxyConfig, ProxyEndpoint};
pub use web_context::WebContext;

#[cfg(target_os = "ios")]
pub type InputAccessoryViewBuilder =
  dyn Fn(&objc2_ui_kit::UIView) -> Option<Retained<objc2_ui_kit::UIView>>;

/// A rectangular region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
  /// Rect position.
  pub position: dpi::Position,
  /// Rect size.
  pub size: dpi::Size,
}

impl Default for Rect {
  fn default() -> Self {
    Self {
      position: dpi::LogicalPosition::new(0, 0).into(),
      size: dpi::LogicalSize::new(0, 0).into(),
    }
  }
}

/// Resolves a custom protocol [`Request`] asynchronously.
///
/// See [`WebViewBuilder::with_asynchronous_custom_protocol`] for more information.
pub struct RequestAsyncResponder {
  pub(crate) responder: Option<Box<dyn FnOnce(Response<Cow<'static, [u8]>>) + Send>>,
}

impl RequestAsyncResponder {
  /// Resolves the request with the given response.
  pub fn respond<T: Into<Cow<'static, [u8]>>>(mut self, response: Response<T>) {
    let (parts, body) = response.into_parts();
    if let Some(responder) = self.responder.take() {
      responder(Response::from_parts(parts, body.into()));
    }
  }
}

impl Drop for RequestAsyncResponder {
  fn drop(&mut self) {
    let Some(responder) = self.responder.take() else {
      return;
    };
    // A forgotten asynchronous responder must not leave an engine request and
    // its native deferral pending forever. Complete it once with a bounded,
    // non-privileged failure response.
    let mut response: Response<Cow<'static, [u8]>> = Response::new(Cow::Borrowed(&[] as &[u8]));
    *response.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
    responder(response);
  }
}

/// Response for the new window request handler.
///
/// See [`WebViewBuilder::with_new_window_req_handler`].
pub enum NewWindowResponse {
  /// Allow the window to be opened with the default implementation.
  Allow,
  /// Allow the window to be opened, with the given platform webview instance.
  ///
  /// ## Platform-specific:
  ///
  /// **Linux**: The webview must be related to the caller webview. See [`WebViewBuilderExtUnix::with_related_view`].
  /// **Windows**: The webview must use the same environment as the caller webview. See [`WebViewBuilderExtWindows::with_environment`].
  /// **macOS**: The webview must use the same configuration as the caller webview. See [`WebViewBuilderExtMacos::with_webview_configuration`].
  #[cfg(not(any(target_os = "android", target_os = "ios")))]
  Create {
    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd",
    ))]
    webview: webkit2gtk::WebView,
    #[cfg(windows)]
    webview: ICoreWebView2,
    #[cfg(target_os = "macos")]
    webview: Retained<objc2_web_kit::WKWebView>,
  },
  /// A host-owned Windows child requiring post-attachment policy registration.
  /// The callback must close the child when attachment or policy setup fails.
  #[cfg(windows)]
  CreateGuarded {
    webview: ICoreWebView2,
    attached: Box<dyn FnOnce(bool)>,
  },
  /// Deny the window from being opened.
  Deny,
}

/// Host policy for page-driven native close requests.
///
/// Web content must not be allowed to destroy Wry's child container behind
/// an embedder that still owns and tracks the controller. The secure default
/// is therefore [`Self::Ignore`]. Embedders may opt into the legacy behavior
/// only when their lifecycle model reconciles the native destruction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageClosePolicy {
  /// Keep the native WebView and its container alive.
  #[default]
  Ignore,
  /// Destroy Wry's child container when the page requests closure.
  DestroyContainer,
}

/// Construction-time policy for downloads initiated by web content.
///
/// [`DownloadPolicy::DenyWithoutMetadata`] is a native-boundary security mode:
/// the platform handler cancels before reading or allocating the page-derived
/// URL, suggested filename, or destination path. It takes precedence over any
/// download callbacks regardless of builder call order; select
/// [`DownloadPolicy::UseHandlers`] explicitly to re-enable those callbacks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DownloadPolicy {
  /// Preserve Wry's normal callback-controlled download behavior.
  #[default]
  UseHandlers,
  /// Cancel every download without materializing page-controlled metadata.
  DenyWithoutMetadata,
}

impl DownloadPolicy {
  pub(crate) fn inspect_metadata<T>(self, inspect: impl FnOnce() -> T) -> Option<T> {
    match self {
      Self::UseHandlers => Some(inspect()),
      Self::DenyWithoutMetadata => None,
    }
  }
}

/// Information about the webview that initiated a new window request.
#[derive(Debug)]
pub struct NewWindowOpener {
  /// The instance of the webview that initiated the new window request.
  ///
  /// This must be set as the related view of the new webview. See [`WebViewBuilderExtUnix::with_related_view`].
  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
  ))]
  pub webview: webkit2gtk::WebView,
  /// The instance of the webview that initiated the new window request.
  #[cfg(windows)]
  pub webview: ICoreWebView2,
  /// The environment of the webview that initiated the new window request.
  ///
  /// The target webview environment **MUST** match the environment of the opener webview. See [`WebViewBuilderExtWindows::with_environment`].
  #[cfg(windows)]
  pub environment: ICoreWebView2Environment,
  /// The instance of the webview that initiated the new window request.
  #[cfg(target_os = "macos")]
  pub webview: Retained<objc2_web_kit::WKWebView>,
  /// Configuration of the target webview.
  ///
  /// This **MUST** be used when creating the target webview. See [`WebViewBuilderExtMacos::with_webview_configuration`].
  #[cfg(target_os = "macos")]
  pub target_configuration: Retained<objc2_web_kit::WKWebViewConfiguration>,
}

/// Window features of a window requested to open.
#[non_exhaustive]
#[derive(Debug)]
pub struct NewWindowFeatures {
  /// Native admission evidence. This is never supplied by page IPC.
  pub user_initiated: bool,
  /// Requested UI disposition; it is not an authorization signal.
  pub foreground: bool,
  /// Specifies the size of the content area
  /// as defined by the user's operating system where the new window will be generated.
  pub size: Option<dpi::LogicalSize<f64>>,
  /// Specifies the position of the window relative to the work area
  /// as defined by the user's operating system where the new window will be generated.
  pub position: Option<dpi::LogicalPosition<f64>>,
  /// Information about the webview opener containing data that must be used when creating the new webview.
  pub opener: NewWindowOpener,
}

/// An id for a webview
pub type WebViewId<'a> = &'a str;

// WebViewAttributes is not stable enough to be pub.
struct WebViewAttributes<'a> {
  /// An id that will be passed when this webview makes requests in certain callbacks.
  pub id: Option<WebViewId<'a>>,

  /// Web context to be shared with this webview.
  #[allow(unused)]
  pub context: Option<&'a mut WebContext>,

  /// Whether the WebView should have a custom user-agent.
  pub user_agent: Option<String>,

  /// Whether the WebView window should be visible.
  pub visible: bool,

  /// Whether the WebView should be transparent.
  ///
  /// ## Platform-specific:
  ///
  /// **Windows 7**: Not supported.
  pub transparent: bool,

  /// Specify the webview background color. This will be ignored if `transparent` is set to `true`.
  ///
  /// The color uses the RGBA format.
  ///
  /// ## Platform-specific:
  ///
  /// - **macOS**: Disables the default white WKWebView background via the `drawsBackground` KVC key
  ///   (same as the `transparent` feature) and sets `underPageBackgroundColor` (macOS 12+) for overscroll areas.
  /// - **Windows**:
  ///   - On Windows 7, transparency is not supported and the alpha value will be ignored.
  ///   - On Windows higher than 7: translucent colors are not supported so any alpha value other than `0` will be replaced by `255`
  pub background_color: Option<RGBA>,

  /// Whether load the provided URL to [`WebView`].
  ///
  /// ## Note
  ///
  /// Data URLs are not supported, use [`html`](Self::html) option instead.
  pub url: Option<String>,

  /// Headers used when loading the requested [`url`](Self::url).
  pub headers: Option<http::HeaderMap>,

  /// Whether page zooming by hotkeys or gestures is enabled
  ///
  /// ## Platform-specific
  ///
  /// - Windows: Setting to `false` can't disable pinch zoom on WebView2 Runtime version before 91.0.865.0,
  ///   see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10865-prerelease>
  ///
  /// - **macOS / Linux / Android / iOS**: Unsupported
  pub zoom_hotkeys_enabled: bool,

  /// Whether load the provided html string to [`WebView`].
  /// This will be ignored if the `url` is provided.
  ///
  /// # Warning
  ///
  /// The Page loaded from html string will have `null` origin.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows:** the string can not be larger than 2 MB (2 * 1024 * 1024 bytes) in total size
  pub html: Option<String>,

  /// A list of initialization javascript scripts to run when loading new pages.
  /// When webview load a new page, this initialization code will be executed.
  /// It is guaranteed that code is executed before `window.onload`.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: scripts are always injected into sub frames.
  /// - **Android:** When [addDocumentStartJavaScript] is not supported,
  ///   we prepend them to each HTML head (implementation only supported on custom protocol URLs).
  ///   For remote URLs, we use [onPageStarted] which is not guaranteed to run before other scripts.
  ///
  /// [addDocumentStartJavaScript]: https://developer.android.com/reference/androidx/webkit/WebViewCompat#addDocumentStartJavaScript(android.webkit.WebView,java.lang.String,java.util.Set%3Cjava.lang.String%3E)
  /// [onPageStarted]: https://developer.android.com/reference/android/webkit/WebViewClient#onPageStarted(android.webkit.WebView,%20java.lang.String,%20android.graphics.Bitmap)
  pub initialization_scripts: Vec<InitializationScript>,

  /// A list of custom loading protocols with pairs of scheme uri string and a handling
  /// closure.
  ///
  /// The closure takes an Id ([WebViewId]), [Request] and [RequestAsyncResponder] as arguments and returns a [Response].
  ///
  /// # Note
  ///
  /// If using a shared [WebContext], make sure custom protocols were not already registered on that web context on Linux.
  ///
  /// # Warning
  ///
  /// Pages loaded from custom protocol will have different Origin on different platforms. And
  /// servers which enforce CORS will need to add exact same Origin header in `Access-Control-Allow-Origin`
  /// if you wish to send requests with native `fetch` and `XmlHttpRequest` APIs. Here are the
  /// different Origin headers across platforms:
  ///
  /// - macOS, iOS and Linux: `<scheme_name>://<path>` (so it will be `wry://path/to/page`).
  /// - Windows and Android: `http://<scheme_name>.<path>` by default (so it will be `http://wry.path/to/page`). To use `https` instead of `http`, use [`WebViewBuilderExtWindows::with_https_scheme`] and [`WebViewBuilderExtAndroid::with_https_scheme`].
  ///
  /// # Reading assets on mobile
  ///
  /// - Android: Android has `assets` and `resource` path finder to
  ///   locate your files in those directories. For more information, see [Loading in-app content](https://developer.android.com/guide/webapps/load-local-content) page.
  /// - iOS: To get the path of your assets, you can call [`CFBundle::resources_path`](https://docs.rs/core-foundation/latest/core_foundation/bundle/struct.CFBundle.html#method.resources_path). So url like `wry://assets/index.html` could get the html file in assets directory.
  pub custom_protocols:
    HashMap<String, Box<dyn Fn(WebViewId, Request<Vec<u8>>, RequestAsyncResponder) + Send + Sync>>,

  /// The IPC handler to receive the message from Javascript on webview
  /// using `window.ipc.postMessage("insert_message_here")` to host Rust code.
  pub ipc_handler: Option<Box<dyn Fn(Request<String>)>>,

  /// A handler closure to process incoming [`DragDropEvent`] of the webview.
  ///
  /// ## Blocking OS Default Behavior
  ///
  /// Return `true` in the callback to block the OS' default behavior.
  ///
  /// Note, that if you do block this behavior, it won't be possible to drop files on `<input type="file">` forms.
  /// Also note, that it's not possible to manually set the value of a `<input type="file">` via JavaScript for security reasons.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows:** This will disable the HTML Drag and Drop APIs like `draggable="true"`,
  ///   since we replace the drag drop handler of WebView 2 on Windows.
  ///   `handler`'s return value is ignored on Windows.
  /// - **Android / iOS:** Unsupported.
  pub drag_drop_handler: Option<Box<dyn Fn(DragDropEvent) -> bool>>,

  /// A navigation handler to decide if incoming url is allowed to navigate.
  ///
  /// The closure take a `String` parameter as url and returns a `bool` to determine whether the navigation should happen.
  /// `true` allows to navigate and `false` does not.
  pub navigation_handler: Option<Box<dyn Fn(String) -> bool>>,

  /// Apple-only navigation policy callback with native action provenance.
  ///
  /// When set, this callback replaces [`Self::navigation_handler`] for Apple
  /// navigation actions. It lets security-sensitive embedders distinguish a
  /// browser history traversal from page-driven links, forms, and reloads.
  #[cfg(any(target_os = "macos", target_os = "ios"))]
  pub apple_navigation_action_handler: Option<Box<dyn Fn(String, AppleNavigationAction) -> bool>>,
  /// Accepted main-frame navigation attempts, before a document commits.
  /// The callback is observational and must not grant page authority.
  #[cfg(target_os = "macos")]
  pub main_frame_navigation_attempt_handler: Option<Box<dyn Fn(String)>>,

  /// A download started handler to manage incoming downloads.
  ///
  /// The closure takes two parameters, the first is a `String` representing the url being downloaded from and the
  /// second is a mutable `PathBuf` reference that (possibly) represents where the file will be downloaded to. The latter
  /// parameter can be used to set the download location by assigning a new path to it, the assigned path _must_ be
  /// absolute. The closure returns a `bool` to allow or deny the download.
  ///
  /// [`Self::default()`] sets a handler allowing all downloads to match browser behavior.
  pub download_started_handler: Option<Box<dyn FnMut(String, &mut PathBuf) -> bool + 'static>>,

  /// A download completion handler to manage downloads that have finished.
  ///
  /// The closure is fired when the download completes, whether it was successful or not.
  /// The closure takes a `String` representing the URL of the original download request, an `Option<PathBuf>`
  /// potentially representing the filesystem path the file was downloaded to, and a `bool` indicating if the download
  /// succeeded. A value of `None` being passed instead of a `PathBuf` does not necessarily indicate that the download
  /// did not succeed, and may instead indicate some other failure, always check the third parameter if you need to
  /// know if the download succeeded.
  ///
  /// ## Platform-specific:
  ///
  /// - **macOS**: The second parameter indicating the path the file was saved to, is always empty,
  ///   due to API limitations.
  pub download_completed_handler: Option<Rc<dyn Fn(String, Option<PathBuf>, bool) + 'static>>,

  /// Construction-time download admission policy. Denial takes precedence
  /// over both download callbacks.
  pub download_policy: DownloadPolicy,

  /// A new window request handler to decide if incoming url is allowed to be opened.
  ///
  /// A new window is requested to be opened by the [window.open] API.
  ///
  /// The closure take the URL to open and the window features object and returns [`NewWindowResponse`] to determine whether the window should open.
  ///
  /// [window.open]: https://developer.mozilla.org/en-US/docs/Web/API/Window/open
  pub new_window_req_handler: Option<Box<dyn Fn(String, NewWindowFeatures) -> NewWindowResponse>>,

  /// Policy for page-driven native close requests. The secure default keeps
  /// lifecycle ownership with the embedder.
  pub page_close_policy: PageClosePolicy,
  #[cfg(any(target_os = "macos", target_os = "windows"))]
  pub page_close_handler: Option<Box<dyn Fn()>>,

  /// Enables clipboard access for the page rendered on **Linux** and **Windows**.
  ///
  /// macOS doesn't provide such method and is always enabled by default. But your app will still need to add menu
  /// item accelerators to use the clipboard shortcuts.
  pub clipboard: bool,

  /// Enable web inspector which is usually called browser devtools.
  ///
  /// Note this only enables devtools to the webview. To open it, you can call
  /// [`WebView::open_devtools`], or right click the page and open it from the context menu.
  ///
  /// ## Platform-specific
  ///
  /// - macOS: This will call private functions on **macOS**. It is enabled in **debug** builds,
  ///   but requires `devtools` feature flag to actually enable it in **release** builds.
  /// - Android: Open `chrome://inspect/#devices` in Chrome to get the devtools window. Wry's `WebView` devtools API isn't supported on Android.
  /// - iOS: Open Safari > Develop > [Your Device Name] > [Your WebView] to get the devtools window.
  pub devtools: bool,

  /// Whether clicking an inactive window also clicks through to the webview. Default is `false`.
  ///
  /// ## Platform-specific
  ///
  /// This configuration only impacts macOS.
  pub accept_first_mouse: bool,

  /// Indicates whether horizontal swipe gestures trigger backward and forward page navigation.
  ///
  /// ## Platform-specific:
  ///
  /// - Windows: Setting to `false` does nothing on WebView2 Runtime version before 92.0.902.0,
  ///   see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10902-prerelease>
  ///
  /// - **Android / iOS:** Unsupported.
  pub back_forward_navigation_gestures: bool,

  /// Set a handler closure to process the change of the webview's document title.
  pub document_title_changed_handler: Option<Box<dyn Fn(String)>>,

  /// Run the WebView with incognito mode. On WebKitGTK, a supplied WebContext
  /// must be explicitly ephemeral; pathless contexts are ephemeral by
  /// default in this fork.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows**: Requires WebView2 Runtime version 101.0.1210.39 or higher, does nothing on older versions,
  ///   see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10121039>
  /// - **Android:** Unsupported yet.
  /// - **macOS / iOS**: Uses the nonPersistent DataStore.
  pub incognito: bool,

  /// Whether all media can be played without user interaction.
  pub autoplay: bool,

  /// Whether page content may enter WebKit's native fullscreen media surface.
  ///
  /// ## Platform-specific
  ///
  /// - **macOS:** WebKit's public element fullscreen preference, set for
  ///   every view. Defaults to on only with the `fullscreen` feature; an
  ///   embedder that enables it should observe `fullscreenState` and exit
  ///   fullscreen whenever the view stops being the one on screen.
  /// - **iOS:** Requires the `fullscreen` feature and uses WebKit private
  ///   preferences.
  /// - Other platforms: Unsupported and ignored.
  pub fullscreen_enabled: bool,

  /// Whether page content may enter WebKit's native picture-in-picture media
  /// surface.
  ///
  /// ## Platform-specific
  ///
  /// - **macOS:** Uses a WebKit private preference. The secure default is
  ///   disabled.
  /// - Other platforms: Unsupported and ignored.
  pub picture_in_picture_enabled: bool,

  /// Set a handler closure to process page load events.
  pub on_page_load_handler: Option<Box<dyn Fn(PageLoadEvent, String)>>,

  /// Set a handler closure to process identity-bearing main-frame navigation events.
  ///
  /// Unlike [`Self::on_page_load_handler`], this preserves the native
  /// navigation sequence across redirects, overlapping loads, and failures.
  /// The identifier is scoped to this WebView and is opaque to the embedder.
  pub navigation_event_handler: Option<Box<dyn Fn(NavigationEvent)>>,

  /// Why a main-frame navigation failed, reported before its `Failed` event.
  pub navigation_failure_handler: Option<Box<dyn Fn(NavigationId, NavigationFailure)>>,

  /// Hide the native presentation surface synchronously at each main-frame
  /// commit, before delivering its identity event. Hardened embedders can
  /// then update trusted chrome and explicitly reveal the exact document.
  pub navigation_presentation_guard: Option<Box<dyn Fn()>>,

  /// Set a proxy configuration for the webview. Supports HTTP CONNECT and SOCKSv5 proxies
  ///
  /// - **macOS**: Requires macOS 14.0+ and the `mac-proxy` feature flag to be enabled.
  /// - **Android / iOS:** Not supported.
  pub proxy_config: Option<ProxyConfig>,

  /// Whether the webview should be focused when created.
  ///
  /// ## Platform-specific:
  ///
  /// - **macOS:** `false` suppresses construction-time application activation.
  ///   Child WebViews additionally preserve existing first-responder ownership;
  ///   non-child construction still installs and focuses its content view. Use
  ///   [`WebView::focus`] when exact child first-responder focus is required.
  /// - **Android / iOS:** Unsupported.
  pub focused: bool,

  /// The webview bounds. Defaults to `x: 0, y: 0, width: 200, height: 200`.
  /// This is only effective if the webview was created by [`WebViewBuilder::new_as_child`]
  /// or on Linux, if was created by [`WebViewExtUnix::new_gtk`] or [`WebViewBuilderExtUnix::new_gtk`] with [`gtk::Fixed`].
  pub bounds: Option<Rect>,

  /// Whether background throttling should be disabled.
  ///
  /// By default, browsers throttle timers and even unload the whole tab (view) to free resources after roughly 5 minutes when
  /// a view became minimized or hidden. This will permanently suspend all tasks until the documents visibility state
  /// changes back from hidden to visible by bringing the view back to the foreground.
  ///
  /// ## Platform-specific
  ///
  /// - **Linux / Windows / Android**: Unsupported. Workarounds like a pending WebLock transaction might suffice.
  /// - **iOS**: Supported since version 17.0+.
  /// - **macOS**: Supported since version 14.0+.
  ///
  /// see <https://github.com/tauri-apps/tauri/issues/5250#issuecomment-2569380578>
  pub background_throttling: Option<BackgroundThrottlingPolicy>,

  /// Whether JavaScript should be disabled.
  pub javascript_disabled: bool,

  /// A handler to intercept permission requests from the webview.
  ///
  /// The handler receives the [`PermissionKind`] and should return
  /// the desired [`PermissionResponse`].
  ///
  /// > [!NOTE]
  /// > This handler only triggers for new permission requests. If the user has already
  /// > allowed or denied a permission persistently within the webview, the browser
  /// > will use the saved preference instead of calling this handler.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows**: Fully supported via WebView2's PermissionRequested event.
  /// - **macOS / iOS**: Camera and microphone via WKUIDelegate's public media
  ///   capture callback (macOS 12+ / iOS 15+). This is not a deny-all capability
  ///   boundary: display capture, geolocation and WebAuthn are not routed here.
  /// - **Linux**: Fully supported via WebKitGTK's permission-request signal.
  /// - **Android**: Supported via JNI bridge for geolocation, microphone, camera,
  ///   protected media, and MIDI requests. Android runtime permissions may still
  ///   trigger native OS prompts before access is granted.
  ///
  /// ## Example
  ///
  /// ```no_run
  /// # use wry::{WebViewBuilder, PermissionKind, PermissionResponse};
  /// let webview = WebViewBuilder::new()
  ///     .with_permission_handler(|kind| {
  ///         match kind {
  ///             PermissionKind::Microphone => PermissionResponse::Allow,
  ///             PermissionKind::Camera => PermissionResponse::Allow,
  ///             _ => PermissionResponse::Default,
  ///         }
  ///     });
  /// ```
  pub permission_handler: Option<Box<dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync>>,
  /// Controls the WebView's browser-level general autofill behavior.
  ///
  /// **This option does not disable password or credit card autofill.**
  ///
  /// When enabled, the WebView may automatically populate form fields using
  /// previously stored data such as addresses or contact information.
  ///
  /// If not specified, this is `true` by default.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported. On Windows, WebView2's autofill feature (called
  ///   "Suggestions") may not honor `autocomplete="off"` attributes on input
  ///   elements in some cases. When this option is `false`, that autofill
  ///   behavior will be disabled.
  /// - **macOS / Linux / Android / iOS**: Unsupported and ignored.
  pub general_autofill_enabled: bool,
}

impl Default for WebViewAttributes<'_> {
  fn default() -> Self {
    Self {
      id: Default::default(),
      context: None,
      user_agent: None,
      visible: true,
      transparent: false,
      background_color: None,
      url: None,
      headers: None,
      html: None,
      initialization_scripts: Default::default(),
      custom_protocols: Default::default(),
      ipc_handler: None,
      drag_drop_handler: None,
      navigation_handler: None,
      #[cfg(any(target_os = "macos", target_os = "ios"))]
      apple_navigation_action_handler: None,
      #[cfg(target_os = "macos")]
      main_frame_navigation_attempt_handler: None,
      download_started_handler: Some(Box::new(|_, _| true)),
      download_completed_handler: None,
      download_policy: DownloadPolicy::UseHandlers,
      new_window_req_handler: None,
      page_close_policy: PageClosePolicy::Ignore,
      #[cfg(any(target_os = "macos", target_os = "windows"))]
      page_close_handler: None,
      clipboard: false,
      #[cfg(debug_assertions)]
      devtools: true,
      #[cfg(not(debug_assertions))]
      devtools: false,
      zoom_hotkeys_enabled: false,
      accept_first_mouse: false,
      back_forward_navigation_gestures: false,
      document_title_changed_handler: None,
      incognito: false,
      autoplay: true,
      fullscreen_enabled: cfg!(feature = "fullscreen"),
      picture_in_picture_enabled: false,
      on_page_load_handler: None,
      navigation_event_handler: None,
      navigation_failure_handler: None,
      navigation_presentation_guard: None,
      proxy_config: None,
      focused: true,
      bounds: Some(Rect {
        position: dpi::LogicalPosition::new(0, 0).into(),
        size: dpi::LogicalSize::new(200, 200).into(),
      }),
      background_throttling: None,
      javascript_disabled: false,
      permission_handler: None,
      general_autofill_enabled: true,
    }
  }
}

impl WebViewAttributes<'_> {
  /// Whether native construction must begin behind the presentation barrier.
  ///
  /// WebKitGTK keeps a guarded child unmapped through construction, then lets
  /// the embedder's presentation stage perform its first map at validated
  /// offscreen geometry. Other backends use their native hidden state.
  #[cfg(any(gtk, target_os = "macos", test))]
  fn guards_initial_presentation(&self) -> bool {
    self.navigation_presentation_guard.is_some()
  }

  #[cfg(any(gtk, test))]
  fn maps_during_initial_construction(&self) -> bool {
    self.visible && !self.guards_initial_presentation()
  }

  #[cfg(any(gtk, target_os = "macos", test))]
  fn focuses_during_initial_construction(&self) -> bool {
    self.focused && !self.guards_initial_presentation()
  }
}

/// Builder type of [`WebView`].
///
/// [`WebViewBuilder`] / [`WebView`] are the basic building blocks to construct WebView contents and
/// scripts for those who prefer to control fine grained window creation and event handling.
/// [`WebViewBuilder`] provides ability to setup initialization before web engine starts.
pub struct WebViewBuilder<'a> {
  attrs: WebViewAttributes<'a>,
  platform_specific: PlatformSpecificWebViewAttributes,
  /// Records errors before the [`WebViewBuilder::build`] is called
  error: crate::Result<()>,
}

impl<'a> WebViewBuilder<'a> {
  /// Create a new [`WebViewBuilder`].
  pub fn new() -> Self {
    Self {
      attrs: WebViewAttributes::default(),
      #[allow(clippy::default_constructed_unit_structs)]
      platform_specific: PlatformSpecificWebViewAttributes::default(),
      error: Ok(()),
    }
  }

  /// Create a new [`WebViewBuilder`] with a web context that can be shared with multiple [`WebView`]s.
  pub fn new_with_web_context(web_context: &'a mut WebContext) -> Self {
    let attrs = WebViewAttributes {
      context: Some(web_context),
      ..Default::default()
    };

    Self {
      attrs,
      #[allow(clippy::default_constructed_unit_structs)]
      platform_specific: PlatformSpecificWebViewAttributes::default(),
      error: Ok(()),
    }
  }

  /// Set an id that will be passed when this webview makes requests in certain callbacks.
  pub fn with_id(mut self, id: WebViewId<'a>) -> Self {
    self.attrs.id = Some(id);
    self
  }

  /// Indicates whether horizontal swipe gestures trigger backward and forward page navigation.
  ///
  /// ## Platform-specific:
  ///
  /// - **Android / iOS:** Unsupported.
  pub fn with_back_forward_navigation_gestures(mut self, gesture: bool) -> Self {
    self.attrs.back_forward_navigation_gestures = gesture;
    self
  }

  /// Sets whether the WebView should be transparent.
  ///
  /// ## Platform-specific:
  ///
  /// **Windows 7**: Not supported.
  pub fn with_transparent(mut self, transparent: bool) -> Self {
    self.attrs.transparent = transparent;
    self
  }

  /// Specify the webview background color. This will be ignored if `transparent` is set to `true`.
  ///
  /// The color uses the RGBA format.
  ///
  /// ## Platform-specific:
  ///
  /// - **macOS**: Disables the default white WKWebView background via the `drawsBackground` KVC key
  ///   (same as the `transparent` feature) and sets `underPageBackgroundColor` (macOS 12+) for overscroll areas.
  /// - **Windows**:
  ///   - on Windows 7, transparency is not supported and the alpha value will be ignored.
  ///   - on Windows higher than 7: translucent colors are not supported so any alpha value other than `0` will be replaced by `255`
  pub fn with_background_color(mut self, background_color: RGBA) -> Self {
    self.attrs.background_color = Some(background_color);
    self
  }

  /// Sets whether the WebView should be visible or not.
  pub fn with_visible(mut self, visible: bool) -> Self {
    self.attrs.visible = visible;
    self
  }

  /// Sets whether all media can be played without user interaction.
  pub fn with_autoplay(mut self, autoplay: bool) -> Self {
    self.attrs.autoplay = autoplay;
    self
  }

  /// Enables or disables page-triggered native fullscreen media surfaces.
  ///
  /// This is currently implemented only on Apple platforms. Set it per view:
  /// feature unification can otherwise change the default for every view
  /// at compile time.
  pub fn with_fullscreen_enabled(mut self, enabled: bool) -> Self {
    self.attrs.fullscreen_enabled = enabled;
    self
  }

  /// Enables or disables page-triggered native picture-in-picture surfaces.
  ///
  /// This is currently implemented only on macOS. It is disabled by default.
  pub fn with_picture_in_picture_enabled(mut self, enabled: bool) -> Self {
    self.attrs.picture_in_picture_enabled = enabled;
    self
  }

  /// Initialize javascript code when loading new pages. When webview load a new page, this
  /// initialization code will be executed. It is guaranteed that code is executed before
  /// `window.onload`.
  ///
  /// ## Example
  /// ```ignore
  /// let webview = WebViewBuilder::new()
  ///   .with_initialization_script("console.log('Running inside main frame only')")
  ///   .with_url("https://tauri.app")
  ///   .build(&window)
  ///   .unwrap();
  /// ```
  ///
  /// ## Platform-specific
  ///
  ///- **Windows:** scripts are always added to subframes.
  /// - **Android:** When [addDocumentStartJavaScript] is not supported,
  ///   we prepend them to each HTML head (implementation only supported on custom protocol URLs).
  ///   For remote URLs, we use [onPageStarted] which is not guaranteed to run before other scripts.
  ///
  /// [addDocumentStartJavaScript]: https://developer.android.com/reference/androidx/webkit/WebViewCompat#addDocumentStartJavaScript(android.webkit.WebView,java.lang.String,java.util.Set%3Cjava.lang.String%3E)
  /// [onPageStarted]: https://developer.android.com/reference/android/webkit/WebViewClient#onPageStarted(android.webkit.WebView,%20java.lang.String,%20android.graphics.Bitmap)
  pub fn with_initialization_script<S: Into<String>>(self, js: S) -> Self {
    self.with_initialization_script_for_main_only(js, true)
  }

  /// Same as [`with_initialization_script`](Self::with_initialization_script) but with option to inject into main frame only or sub frames.
  ///
  /// ## Example
  /// ```ignore
  /// let webview = WebViewBuilder::new()
  ///   .with_initialization_script_for_main_only("console.log('Running inside main frame only')", true)
  ///   .with_initialization_script_for_main_only("console.log('Running  main frame and sub frames')", false)
  ///   .with_url("https://tauri.app")
  ///   .build(&window)
  ///   .unwrap();
  /// ```
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows:** scripts are always added to subframes regardless of the `for_main_frame_only` option.
  /// - **Android**: When [addDocumentStartJavaScript] is not supported, scripts are always injected into main frame only.
  ///
  /// [addDocumentStartJavaScript]: https://developer.android.com/reference/androidx/webkit/WebViewCompat#addDocumentStartJavaScript(android.webkit.WebView,java.lang.String,java.util.Set%3Cjava.lang.String%3E)
  pub fn with_initialization_script_for_main_only<S: Into<String>>(
    mut self,
    js: S,
    for_main_frame_only: bool,
  ) -> Self {
    let script = js.into();
    if !script.is_empty() {
      self
        .attrs
        .initialization_scripts
        .push(InitializationScript {
          script,
          for_main_frame_only,
        });
    }
    self
  }

  /// Register custom loading protocols with pairs of scheme uri string and a handling
  /// closure.
  ///
  /// The closure takes a [Request] and returns a [Response]
  ///
  /// When registering a custom protocol with the same name, only the last registered one will be used.
  ///
  /// # Warning
  ///
  /// Pages loaded from custom protocol will have different Origin on different platforms. And
  /// servers which enforce CORS will need to add exact same Origin header in `Access-Control-Allow-Origin`
  /// if you wish to send requests with native `fetch` and `XmlHttpRequest` APIs. Here are the
  /// different Origin headers across platforms:
  ///
  /// - macOS, iOS and Linux: `<scheme_name>://<path>` (so it will be `wry://path/to/page).
  /// - Windows and Android: `http://<scheme_name>.<path>` by default (so it will be `http://wry.path/to/page`). To use `https` instead of `http`, use [`WebViewBuilderExtWindows::with_https_scheme`] and [`WebViewBuilderExtAndroid::with_https_scheme`].
  ///
  /// # Reading assets on mobile
  ///
  /// - Android: For loading content from the `assets` folder (which is copied to the Andorid apk) please
  ///   use the function [`with_asset_loader`] from [`WebViewBuilderExtAndroid`] instead.
  ///   This function on Android can only be used to serve assets you can embed in the binary or are
  ///   elsewhere in Android (provided the app has appropriate access), but not from the `assets`
  ///   folder which lives within the apk. For the cases where this can be used, it works the same as in macOS and Linux.
  /// - iOS: To get the path of your assets, you can call [`CFBundle::resources_path`](https://docs.rs/core-foundation/latest/core_foundation/bundle/struct.CFBundle.html#method.resources_path). So url like `wry://assets/index.html` could get the html file in assets directory.
  #[cfg(feature = "protocol")]
  pub fn with_custom_protocol<F>(mut self, name: String, handler: F) -> Self
  where
    F: Fn(WebViewId, Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> + Send + Sync + 'static,
  {
    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd",
    ))]
    if let Some(context) = &mut self.attrs.context {
      if context.is_custom_protocol_registered(&name) {
        let err = Err(crate::Error::DuplicateCustomProtocol(name));
        self.error = self.error.and(err);
        return self;
      }
    }

    if self.attrs.custom_protocols.contains_key(&name) {
      let err = Err(crate::Error::DuplicateCustomProtocol(name));
      self.error = self.error.and(err);
      return self;
    }

    self.attrs.custom_protocols.insert(
      name,
      Box::new(move |id, request, responder| {
        let http_response = handler(id, request);
        responder.respond(http_response);
      }),
    );
    self
  }

  /// Same as [`Self::with_custom_protocol`] but with an asynchronous responder.
  ///
  /// When registering a custom protocol with the same name, only the last registered one will be used.
  ///
  /// # Warning
  ///
  /// Pages loaded from custom protocol will have different Origin on different platforms. And
  /// servers which enforce CORS will need to add exact same Origin header in `Access-Control-Allow-Origin`
  /// if you wish to send requests with native `fetch` and `XmlHttpRequest` APIs. Here are the
  /// different Origin headers across platforms:
  ///
  /// - macOS, iOS and Linux: `<scheme_name>://<path>` (so it will be `wry://path/to/page).
  /// - Windows and Android: `http://<scheme_name>.<path>` by default (so it will be `http://wry.path/to/page`). To use `https` instead of `http`, use [`WebViewBuilderExtWindows::with_https_scheme`] and [`WebViewBuilderExtAndroid::with_https_scheme`].
  ///
  /// # Examples
  ///
  /// ```no_run
  /// use wry::{WebViewBuilder, raw_window_handle};
  /// WebViewBuilder::new()
  ///   .with_asynchronous_custom_protocol("wry".into(), |_webview_id, request, responder| {
  ///     // here you can use a tokio task, thread pool or anything
  ///     // to do heavy computation to resolve your request
  ///     // e.g. downloading files, opening the camera...
  ///     std::thread::spawn(move || {
  ///       std::thread::sleep(std::time::Duration::from_secs(2));
  ///       responder.respond(http::Response::builder().body(Vec::new()).unwrap());
  ///     });
  ///   });
  /// ```
  #[cfg(feature = "protocol")]
  pub fn with_asynchronous_custom_protocol<F>(mut self, name: String, handler: F) -> Self
  where
    F: Fn(WebViewId, Request<Vec<u8>>, RequestAsyncResponder) + Send + Sync + 'static,
  {
    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd",
    ))]
    if let Some(context) = &mut self.attrs.context {
      if context.is_custom_protocol_registered(&name) {
        let err = Err(crate::Error::DuplicateCustomProtocol(name));
        self.error = self.error.and(err);
        return self;
      }
    }

    if self.attrs.custom_protocols.contains_key(&name) {
      let err = Err(crate::Error::DuplicateCustomProtocol(name));
      self.error = self.error.and(err);
      return self;
    }

    self.attrs.custom_protocols.insert(name, Box::new(handler));
    self
  }

  /// Set the IPC handler to receive the message from Javascript on webview
  /// using `window.ipc.postMessage("insert_message_here")` to host Rust code.
  ///
  /// ## Platform-specific
  ///
  /// - **Linux / Android**: The request URL is not supported on iframes and the main frame URL is used instead.
  pub fn with_ipc_handler<F>(mut self, handler: F) -> Self
  where
    F: Fn(Request<String>) + 'static,
  {
    self.attrs.ipc_handler = Some(Box::new(handler));
    self
  }

  /// A handler closure to process incoming [`DragDropEvent`] of the webview.
  ///
  /// ## Blocking OS Default Behavior
  ///
  /// Return `true` in the callback to block the OS' default behavior.
  ///
  /// Note, that if you do block this behavior, it won't be possible to drop files on `<input type="file">` forms.
  /// Also note, that it's not possible to manually set the value of a `<input type="file">` via JavaScript for security reasons.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows:** This will disable the HTML Drag and Drop APIs like `draggable="true"`,
  ///   since we replace the drag drop handler of WebView 2 on Windows.
  ///   `handler`'s return value is ignored on Windows.
  /// - **Android / iOS:** Unsupported.
  pub fn with_drag_drop_handler<F>(mut self, handler: F) -> Self
  where
    F: Fn(DragDropEvent) -> bool + 'static,
  {
    self.attrs.drag_drop_handler = Some(Box::new(handler));
    self
  }

  /// Load the provided URL with given headers when the builder calling [`WebViewBuilder::build`] to create the [`WebView`].
  /// The provided URL must be valid.
  ///
  /// ## Note
  ///
  /// Data URLs are not supported, use [`html`](Self::with_html) option instead.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows and Android:** if the URL's scheme is a registered custom protocol,
  ///   a work around is used that changes the URL this navigates to
  ///   from `{protocol}://localhost/abc` to `{http_or_https}://{protocol}.localhost/abc`
  pub fn with_url_and_headers(mut self, url: impl Into<String>, headers: http::HeaderMap) -> Self {
    self.attrs.url = Some(url.into());
    self.attrs.headers = Some(headers);
    self
  }

  /// Load the provided URL when the builder calling [`WebViewBuilder::build`] to create the [`WebView`].
  /// The provided URL must be valid.
  ///
  /// ## Note
  ///
  /// Data URLs are not supported, use [`html`](Self::with_html) option instead.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows and Android:** if the URL's scheme is a registered custom protocol,
  ///   a work around is used that changes the URL this navigates to
  ///   from `{protocol}://localhost/abc` to `{http_or_https}://{protocol}.localhost/abc`
  pub fn with_url(mut self, url: impl Into<String>) -> Self {
    self.attrs.url = Some(url.into());
    self.attrs.headers = None;
    self
  }

  /// Set headers used when loading the requested [`url`](Self::with_url).
  pub fn with_headers(mut self, headers: http::HeaderMap) -> Self {
    self.attrs.headers = Some(headers);
    self
  }

  /// Load the provided HTML string when the builder calling [`WebViewBuilder::build`] to create the [`WebView`].
  /// This will be ignored if `url` is provided.
  ///
  /// # Warning
  ///
  /// The Page loaded from html string will have `null` origin.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows:** the string can not be larger than 2 MB (2 * 1024 * 1024 bytes) in total size
  pub fn with_html(mut self, html: impl Into<String>) -> Self {
    self.attrs.html = Some(html.into());
    self
  }

  /// Set a custom [user-agent](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/User-Agent) for the WebView.
  ///
  /// ## Platform-specific
  ///
  /// - Windows: Requires WebView2 Runtime version 86.0.616.0 or higher, does nothing on older versions,
  ///   see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10790-prerelease>
  pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
    self.attrs.user_agent = Some(user_agent.into());
    self
  }

  /// Enable or disable web inspector which is usually called devtools.
  ///
  /// Note this only enables devtools to the webview. To open it, you can call
  /// [`WebView::open_devtools`], or right click the page and open it from the context menu.
  ///
  /// ## Platform-specific
  ///
  /// - macOS: This will call private functions on **macOS**. It is enabled in **debug** builds,
  ///   but requires `devtools` feature flag to actually enable it in **release** builds.
  /// - Android: Open `chrome://inspect/#devices` in Chrome to get the devtools window. Wry's `WebView` devtools API isn't supported on Android.
  /// - iOS: Open Safari > Develop > [Your Device Name] > [Your WebView] to get the devtools window.
  pub fn with_devtools(mut self, devtools: bool) -> Self {
    self.attrs.devtools = devtools;
    self
  }

  /// Whether page zooming by hotkeys or gestures is enabled
  ///
  /// ## Platform-specific
  ///
  /// - Windows: Setting to `false` can't disable pinch zoom on WebView2 Runtime version before 91.0.865.0,
  ///   see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10865-prerelease>
  ///
  /// - **macOS / Linux / Android / iOS**: Unsupported
  pub fn with_hotkeys_zoom(mut self, zoom: bool) -> Self {
    self.attrs.zoom_hotkeys_enabled = zoom;
    self
  }

  /// Set a navigation handler to decide if incoming url is allowed to navigate.
  ///
  /// The closure take a `String` parameter as url and returns a `bool` to determine whether the navigation should happen.
  /// `true` allows to navigate and `false` does not.
  pub fn with_navigation_handler(mut self, callback: impl Fn(String) -> bool + 'static) -> Self {
    self.attrs.navigation_handler = Some(Box::new(callback));
    self
  }

  /// Set an Apple navigation policy handler that also receives immutable
  /// native action provenance. This replaces the URL-only callback on macOS
  /// and iOS and is unavailable on other platforms.
  #[cfg(any(target_os = "macos", target_os = "ios"))]
  pub fn with_apple_navigation_action_handler(
    mut self,
    callback: impl Fn(String, AppleNavigationAction) -> bool + 'static,
  ) -> Self {
    self.attrs.apple_navigation_action_handler = Some(Box::new(callback));
    self
  }

  /// Observe an admitted macOS main-frame request before its document commits.
  /// This does not change navigation policy or report a committed page URL.
  #[cfg(target_os = "macos")]
  pub fn with_main_frame_navigation_attempt_handler(
    mut self,
    callback: impl Fn(String) + 'static,
  ) -> Self {
    self.attrs.main_frame_navigation_attempt_handler = Some(Box::new(callback));
    self
  }

  /// Set a handler to intercept permission requests from the webview.
  ///
  /// The handler receives the [`PermissionKind`] and should return
  /// the desired [`PermissionResponse`].
  ///
  /// > [!NOTE]
  /// > This handler only triggers for new permission requests. If the user has already
  /// > allowed or denied a permission persistently within the webview, the browser
  /// > will use the saved preference instead of calling this handler.
  ///
  /// ## Platform-specific:
  ///
  /// - **Windows**: Fully supported via WebView2's PermissionRequested event.
  /// - **macOS / iOS**: Camera and microphone via WKUIDelegate's public media
  ///   capture callback (macOS 12+ / iOS 15+). This is not a deny-all capability
  ///   boundary: display capture, geolocation and WebAuthn are not routed here.
  /// - **Linux**: Fully supported via WebKitGTK's permission-request signal.
  /// - **Android**: Supported via JNI bridge for geolocation, microphone, camera,
  ///   protected media, and MIDI requests. Android runtime permissions may still
  ///   trigger native OS prompts before access is granted.
  ///
  /// ## Example
  ///
  /// ```no_run
  /// # use wry::{WebViewBuilder, PermissionKind, PermissionResponse};
  /// let webview = WebViewBuilder::new()
  ///     .with_permission_handler(|kind| {
  ///         match kind {
  ///             PermissionKind::Microphone => PermissionResponse::Allow,
  ///             PermissionKind::Camera => PermissionResponse::Allow,
  ///             _ => PermissionResponse::Default,
  ///         }
  ///     });
  /// ```
  pub fn with_permission_handler<F>(mut self, handler: F) -> Self
  where
    F: Fn(PermissionKind) -> PermissionResponse + Send + Sync + 'static,
  {
    self.attrs.permission_handler = Some(Box::new(handler));
    #[cfg(target_os = "macos")]
    {
      self.platform_specific.permission_request_handler = None;
    }
    self
  }

  /// Set a download started handler to manage incoming downloads.
  ///
  /// The closure takes two parameters, the first is a `String` representing the url being downloaded from and the
  /// second is a mutable `PathBuf` reference that (possibly) represents where the file will be downloaded to. The latter
  /// parameter can be used to set the download location by assigning a new path to it, the assigned path _must_ be
  /// absolute. The closure returns a `bool` to allow or deny the download.
  ///
  /// By default a handler that allows all downloads is set to match browser behavior.
  pub fn with_download_started_handler(
    mut self,
    download_started_handler: impl FnMut(String, &mut PathBuf) -> bool + 'static,
  ) -> Self {
    self.attrs.download_started_handler = Some(Box::new(download_started_handler));
    self
  }

  /// Sets the construction-time download policy.
  ///
  /// `DenyWithoutMetadata` is stronger than returning `false` from a started
  /// callback: cancellation happens before Wry requests or copies the URL,
  /// filename, or destination from the native engine. The deny policy remains
  /// authoritative even if download callbacks are configured before or after
  /// this method. Call this again with `UseHandlers` to re-enable callbacks.
  pub fn with_download_policy(mut self, policy: DownloadPolicy) -> Self {
    self.attrs.download_policy = policy;
    self
  }

  /// Sets a download completion handler to manage downloads that have finished.
  ///
  /// The closure is fired when the download completes, whether it was successful or not.
  /// The closure takes a `String` representing the URL of the original download request, an `Option<PathBuf>`
  /// potentially representing the filesystem path the file was downloaded to, and a `bool` indicating if the download
  /// succeeded. A value of `None` being passed instead of a `PathBuf` does not necessarily indicate that the download
  /// did not succeed, and may instead indicate some other failure, always check the third parameter if you need to
  /// know if the download succeeded.
  ///
  /// ## Platform-specific:
  ///
  /// - **macOS**: The second parameter indicating the path the file was saved to, is always empty,
  ///   due to API limitations.
  pub fn with_download_completed_handler(
    mut self,
    download_completed_handler: impl Fn(String, Option<PathBuf>, bool) + 'static,
  ) -> Self {
    self.attrs.download_completed_handler = Some(Rc::new(download_completed_handler));
    self
  }

  /// Enables clipboard access for the page rendered on **Linux** and **Windows**.
  ///
  /// macOS doesn't provide such method and is always enabled by default. But your app will still need to add menu
  /// item accelerators to use the clipboard shortcuts.
  pub fn with_clipboard(mut self, clipboard: bool) -> Self {
    self.attrs.clipboard = clipboard;
    self
  }

  /// Set a new window request handler to decide if incoming url is allowed to be opened.
  ///
  /// A new window is requested to be opened by the [window.open] API.
  ///
  /// The closure take the URL to open and the window features object and returns [`NewWindowResponse`] to determine whether the window should open.
  ///
  /// [window.open]: https://developer.mozilla.org/en-US/docs/Web/API/Window/open
  pub fn with_new_window_req_handler(
    mut self,
    callback: impl Fn(String, NewWindowFeatures) -> NewWindowResponse + 'static,
  ) -> Self {
    self.attrs.new_window_req_handler = Some(Box::new(callback));
    self
  }

  /// Select how a page-driven native close request affects Wry's child
  /// container. The default is [`PageClosePolicy::Ignore`].
  ///
  /// [`PageClosePolicy::DestroyContainer`] is appropriate only when the
  /// embedder also reconciles its native and logical-view ownership;
  /// otherwise it can retain a stale host entry.
  pub fn with_page_close_policy(mut self, policy: PageClosePolicy) -> Self {
    self.attrs.page_close_policy = policy;
    self
  }

  /// Route native page-close requests to the host without destroying its
  /// controller behind the logical tab lifecycle.
  #[cfg(any(target_os = "macos", target_os = "windows"))]
  pub fn with_page_close_handler(mut self, handler: impl Fn() + 'static) -> Self {
    self.attrs.page_close_handler = Some(Box::new(handler));
    self
  }

  /// Sets whether clicking an inactive window also clicks through to the webview. Default is `false`.
  ///
  /// ## Platform-specific
  ///
  /// This configuration only impacts macOS.
  pub fn with_accept_first_mouse(mut self, accept_first_mouse: bool) -> Self {
    self.attrs.accept_first_mouse = accept_first_mouse;
    self
  }

  /// Set a handler closure to process the change of the webview's document title.
  pub fn with_document_title_changed_handler(
    mut self,
    callback: impl Fn(String) + 'static,
  ) -> Self {
    self.attrs.document_title_changed_handler = Some(Box::new(callback));
    self
  }

  /// Run the WebView with incognito mode. On WebKitGTK, a supplied WebContext
  /// must be explicitly ephemeral; pathless contexts are ephemeral by
  /// default in this fork.
  ///
  /// ## Platform-specific:
  ///
  /// - Windows: Requires WebView2 Runtime version 101.0.1210.39 or higher, does nothing on older versions,
  ///   see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10121039>
  /// - **Android:** Unsupported yet.
  pub fn with_incognito(mut self, incognito: bool) -> Self {
    self.attrs.incognito = incognito;
    self
  }

  /// Set a handler to process page loading events.
  pub fn with_on_page_load_handler(
    mut self,
    handler: impl Fn(PageLoadEvent, String) + 'static,
  ) -> Self {
    self.attrs.on_page_load_handler = Some(Box::new(handler));
    self
  }

  /// Set a handler for identity-bearing main-frame navigation events.
  ///
  /// The same [`NavigationId`] is reported for every phase of one navigation,
  /// including every server redirect. Different navigations may overlap, so
  /// consumers must correlate by identifier rather than URL or callback
  /// order. A `Committed` event is the authoritative point at which rendered
  /// main-frame content may be attributed to its reported URL.
  ///
  /// Supported on Windows, macOS, iOS, and Linux. Android currently ignores
  /// this handler.
  pub fn with_navigation_event_handler(
    mut self,
    handler: impl Fn(NavigationEvent) + 'static,
  ) -> Self {
    self.attrs.navigation_event_handler = Some(Box::new(handler));
    self
  }

  /// Reports why a main-frame navigation failed, keyed by the same
  /// [`NavigationId`] its [`NavigationEventPhase::Failed`] event carries, and
  /// always before that event. Only the category is reported, never the
  /// native error text.
  ///
  /// Supported on macOS and Windows. On Windows, WebView2's own error page
  /// is hidden and never reported as a commit. Other platforms ignore it.
  pub fn with_navigation_failure_handler(
    mut self,
    handler: impl Fn(NavigationId, NavigationFailure) + 'static,
  ) -> Self {
    self.attrs.navigation_failure_handler = Some(Box::new(handler));
    self
  }

  /// Hide each newly committed main-frame document before its identity event
  /// reaches the embedder. `guard` runs before the native hide primitive, so
  /// it should synchronously revoke the embedder's reveal permit without
  /// entering native UI code. The embedder must call [`WebView::set_visible`]
  /// only after it has acknowledged the matching trusted-chrome state.
  ///
  /// On Linux, this fork additionally revokes GTK mapping, opacity, and input.
  /// A guarded view starts unmapped, and `set_visible(true)` alone deliberately
  /// cannot restore every revoked surface. The embedder must own a native GTK
  /// presentation stage that maps offscreen first, then restores paint and
  /// input only after exact navigation attribution.
  pub fn with_navigation_presentation_guard(mut self, guard: impl Fn() + 'static) -> Self {
    self.attrs.navigation_presentation_guard = Some(Box::new(guard));
    self
  }

  /// Set a proxy configuration for the webview. Supports HTTP CONNECT and SOCKSv5 proxies
  ///
  /// - **macOS**: Requires macOS 14.0+ and the `mac-proxy` feature flag to be enabled.
  /// - **Android / iOS:** Not supported.
  pub fn with_proxy_config(mut self, configuration: ProxyConfig) -> Self {
    self.attrs.proxy_config = Some(configuration);
    self
  }

  /// Set whether the webview should be focused when created.
  ///
  /// ## Platform-specific:
  ///
  /// - **macOS:** `false` suppresses construction-time application activation.
  ///   Child WebViews additionally preserve existing first-responder ownership;
  ///   non-child construction still installs and focuses its content view. Use
  ///   [`WebView::focus`] when exact child first-responder focus is required.
  /// - **Android / iOS:** Unsupported.
  pub fn with_focused(mut self, focused: bool) -> Self {
    self.attrs.focused = focused;
    self
  }

  /// Specify the webview position relative to its parent if it will be created as a child
  /// or if created using [`WebViewBuilderExtUnix::new_gtk`] with [`gtk::Fixed`].
  ///
  /// Defaults to `x: 0, y: 0, width: 200, height: 200`.
  pub fn with_bounds(mut self, bounds: Rect) -> Self {
    self.attrs.bounds = Some(bounds);
    self
  }

  /// Set whether background throttling should be disabled.
  ///
  /// By default, browsers throttle timers and even unload the whole tab (view) to free resources after roughly 5 minutes when
  /// a view became minimized or hidden. This will permanently suspend all tasks until the documents visibility state
  /// changes back from hidden to visible by bringing the view back to the foreground.
  ///
  /// ## Platform-specific
  ///
  /// - **Linux / Windows / Android**: Unsupported. Workarounds like a pending WebLock transaction might suffice.
  /// - **iOS**: Supported since version 17.0+.
  /// - **macOS**: Supported since version 14.0+.
  ///
  /// see <https://github.com/tauri-apps/tauri/issues/5250#issuecomment-2569380578>
  pub fn with_background_throttling(mut self, policy: BackgroundThrottlingPolicy) -> Self {
    self.attrs.background_throttling = Some(policy);
    self
  }

  /// Whether JavaScript should be disabled.
  pub fn with_javascript_disabled(mut self) -> Self {
    self.attrs.javascript_disabled = true;
    self
  }

  /// Controls the WebView's browser-level general autofill behavior.
  ///
  /// **This option does not disable password or credit card autofill.**
  ///
  /// When enabled, the WebView may automatically populate form fields using
  /// previously stored data such as addresses or contact information.
  ///
  /// If not specified, this is `true` by default.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported. On Windows, WebView2's autofill feature (called
  ///   "Suggestions") may not honor `autocomplete="off"` attributes on input
  ///   elements in some cases. When this option is `false`, that autofill
  ///   behavior will be disabled.
  /// - **macOS / Linux / Android / iOS**: Unsupported and ignored.
  pub fn with_general_autofill_enabled(mut self, enabled: bool) -> Self {
    self.attrs.general_autofill_enabled = enabled;
    self
  }

  /// Consume the builder and create the [`WebView`] from a type that implements [`HasWindowHandle`].
  ///
  /// # Platform-specific:
  ///
  /// - **Linux**: Only X11 is supported, if you want to support Wayland too, use [`WebViewBuilderExtUnix::new_gtk`].
  ///
  ///   Although this method only needs an X11 window handle, we use webkit2gtk, so you still need to initialize gtk
  ///   by callling [`gtk::init`] and advance its loop alongside your event loop using [`gtk::main_iteration_do`].
  ///   Checkout the [Platform Considerations](https://docs.rs/wry/latest/wry/#platform-considerations) section in the crate root documentation.
  /// - **Windows**: The webview will auto-resize when the passed handle is resized.
  /// - **Linux (X11)**: Unlike macOS and Windows, the webview will not auto-resize and you'll need to call [`WebView::set_bounds`] manually.
  ///
  /// # Panics:
  ///
  /// - Panics if the provided handle was not supported or invalid.
  /// - Panics on Linux, if [`gtk::init`] was not called in this thread.
  pub fn build<W: HasWindowHandle>(self, window: &'a W) -> Result<WebView> {
    self.error?;

    InnerWebView::new(window, self.attrs, self.platform_specific).map(|webview| WebView { webview })
  }

  /// Consume the builder and create the [`WebView`] as a child window inside the provided [`HasWindowHandle`].
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: This will create the webview as a child window of the `parent` window.
  /// - **macOS**: This will create the webview as a `NSView` subview of the `parent` window's
  ///   content view.
  /// - **Linux**: This will create the webview as a child window of the `parent` window. Only X11
  ///   is supported. This method won't work on Wayland.
  ///
  ///   Although this methods only needs an X11 window handle, you use webkit2gtk, so you still need to initialize gtk
  ///   by callling [`gtk::init`] and advance its loop alongside your event loop using [`gtk::main_iteration_do`].
  ///   Checkout the [Platform Considerations](https://docs.rs/wry/latest/wry/#platform-considerations) section in the crate root documentation.
  ///
  ///   If you want to support child webviews on X11 and Wayland at the same time,
  ///   we recommend using [`WebViewBuilderExtUnix::new_gtk`] with [`gtk::Fixed`].
  /// - **Android/iOS:** Unsupported.
  ///
  /// # Panics:
  ///
  /// - Panics if the provided handle was not support or invalid.
  /// - Panics on Linux, if [`gtk::init`] was not called in this thread.
  pub fn build_as_child<W: HasWindowHandle>(self, window: &'a W) -> Result<WebView> {
    self.error?;

    InnerWebView::new_as_child(window, self.attrs, self.platform_specific)
      .map(|webview| WebView { webview })
  }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) struct PlatformSpecificWebViewAttributes {
  data_store_identifier: Option<[u8; 16]>,
  traffic_light_inset: Option<dpi::Position>,
  allow_link_preview: bool,
  on_web_content_process_terminate_handler: Option<Box<dyn Fn()>>,
  #[cfg(target_os = "ios")]
  input_accessory_view_builder: Option<Box<InputAccessoryViewBuilder>>,
  #[cfg(target_os = "ios")]
  limit_navigations_to_app_bound_domains: bool,
  #[cfg(target_os = "macos")]
  webview_configuration: Option<Retained<objc2_web_kit::WKWebViewConfiguration>>,
  #[cfg(target_os = "macos")]
  permission_request_handler:
    Option<Box<dyn Fn(PermissionRequest) -> PermissionRequestDisposition>>,
  #[cfg(target_os = "macos")]
  file_upload_handler: Option<Box<file_upload::FileUploadHandler>>,
  #[cfg(target_os = "macos")]
  native_download_handler: Option<Box<dyn Fn(&objc2_web_kit::WKDownload)>>,
  #[cfg(target_os = "macos")]
  context_menu_handler: Option<
    Box<
      dyn Fn(
        &objc2_app_kit::NSEvent,
        Option<Retained<objc2_app_kit::NSMenu>>,
      ) -> Option<Retained<objc2_app_kit::NSMenu>>,
    >,
  >,
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl Default for PlatformSpecificWebViewAttributes {
  fn default() -> Self {
    Self {
      data_store_identifier: None,
      traffic_light_inset: None,
      // platform default for this is true
      allow_link_preview: true,
      on_web_content_process_terminate_handler: None,
      #[cfg(target_os = "ios")]
      input_accessory_view_builder: None,
      #[cfg(target_os = "ios")]
      limit_navigations_to_app_bound_domains: false,
      #[cfg(target_os = "macos")]
      webview_configuration: None,
      #[cfg(target_os = "macos")]
      permission_request_handler: None,
      #[cfg(target_os = "macos")]
      file_upload_handler: None,
      #[cfg(target_os = "macos")]
      native_download_handler: None,
      #[cfg(target_os = "macos")]
      context_menu_handler: None,
    }
  }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub trait WebViewBuilderExtDarwin {
  /// Initialize the WebView with a custom data store identifier.
  /// Can be used as a replacement for data_directory not being available in WKWebView.
  ///
  /// - **macOS / iOS**: Available on macOS >= 14 and iOS >= 17
  ///
  /// Note: Enable incognito mode to use the `nonPersistent` DataStore.
  fn with_data_store_identifier(self, identifier: [u8; 16]) -> Self;
  /// Move the window controls to the specified position.
  /// Normally this is handled by the Window but because `WebViewBuilder::build()` overwrites the window's NSView the controls will flicker on resizing.
  /// Note: This method has no effects if the WebView is injected via `WebViewBuilder::build_as_child();` and there should be no flickers.
  /// Warning: Do not use this if your chosen window library does not support traffic light insets.
  /// Warning: Only use this in **decorated** windows with a **hidden titlebar**!
  fn with_traffic_light_inset<P: Into<dpi::Position>>(self, position: P) -> Self;
  /// Whether to show a link preview when long pressing on links. Available on macOS and iOS only.
  ///
  /// Default is true.
  ///
  /// See <https://developer.apple.com/documentation/webkit/wkwebview/allowslinkpreview>.
  fn with_allow_link_preview(self, allow_link_preview: bool) -> Self;
  /// Set a handler closure to respond to web content process termination. Available on macOS and iOS only.
  fn with_on_web_content_process_terminate_handler(self, handler: impl Fn() + 'static) -> Self;
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl WebViewBuilderExtDarwin for WebViewBuilder<'_> {
  fn with_data_store_identifier(mut self, identifier: [u8; 16]) -> Self {
    self.platform_specific.data_store_identifier = Some(identifier);
    self
  }

  fn with_traffic_light_inset<P: Into<dpi::Position>>(mut self, position: P) -> Self {
    self.platform_specific.traffic_light_inset = Some(position.into());
    self
  }

  fn with_allow_link_preview(mut self, allow_link_preview: bool) -> Self {
    self.platform_specific.allow_link_preview = allow_link_preview;
    self
  }

  fn with_on_web_content_process_terminate_handler(mut self, handler: impl Fn() + 'static) -> Self {
    self
      .platform_specific
      .on_web_content_process_terminate_handler = Some(Box::new(handler));
    self
  }
}

#[cfg(target_os = "macos")]
pub trait WebViewBuilderExtMacos {
  /// Set the webview configuration that must be used to create the new webview.
  fn with_webview_configuration(
    self,
    configuration: Retained<objc2_web_kit::WKWebViewConfiguration>,
  ) -> Self;
  /// Installs an origin-labelled permission broker capable of deferring the
  /// native completion without blocking WebKit's main thread.
  ///
  /// At most a small fixed cohort is retained per WebView. Overflow,
  /// malformed native origin metadata, handler panic, teardown, and unknown
  /// settlement ids all fail closed. Calling this replaces the simpler
  /// kind-only permission handler. The closure and later settlement both run
  /// on WebKit's owning main thread.
  fn with_permission_request_handler(
    self,
    handler: impl Fn(PermissionRequest) -> PermissionRequestDisposition + 'static,
  ) -> Self;
  /// Transfers native download delegation to the embedder. The callback must
  /// retain its delegate/operation and cancel on any admission failure.
  /// Requires `DownloadPolicy::UseHandlers`; a deny policy always wins.
  fn with_native_download_handler(
    self,
    handler: impl Fn(&objc2_web_kit::WKDownload) + 'static,
  ) -> Self;
  /// Installs a main-thread native file upload broker. Without it uploads are
  /// denied before inspecting request metadata. The responder cancels on drop;
  /// the embedder must cancel retained responders on navigation and teardown.
  /// WebKit owns user activation and the original input/frame association.
  fn with_file_upload_handler(
    self,
    handler: impl Fn(&objc2_web_kit::WKWebView, FileUploadRequest, FileUploadResponder) + 'static,
  ) -> Self;
  /// Merges browser-owned items into the native page context menu without
  /// serializing page metadata through Rust. The callback receives WebKit's
  /// already-created default menu and may replace or suppress it.
  fn with_context_menu_handler(
    self,
    handler: impl Fn(
        &objc2_app_kit::NSEvent,
        Option<Retained<objc2_app_kit::NSMenu>>,
      ) -> Option<Retained<objc2_app_kit::NSMenu>>
      + 'static,
  ) -> Self;
}

#[cfg(target_os = "macos")]
impl WebViewBuilderExtMacos for WebViewBuilder<'_> {
  fn with_webview_configuration(
    mut self,
    configuration: Retained<objc2_web_kit::WKWebViewConfiguration>,
  ) -> Self {
    self
      .platform_specific
      .webview_configuration
      .replace(configuration);
    self
  }

  fn with_permission_request_handler(
    mut self,
    handler: impl Fn(PermissionRequest) -> PermissionRequestDisposition + 'static,
  ) -> Self {
    self.attrs.permission_handler = None;
    self.platform_specific.permission_request_handler = Some(Box::new(handler));
    self
  }

  fn with_native_download_handler(
    mut self,
    handler: impl Fn(&objc2_web_kit::WKDownload) + 'static,
  ) -> Self {
    self.platform_specific.native_download_handler = Some(Box::new(handler));
    self
  }

  fn with_file_upload_handler(
    mut self,
    handler: impl Fn(&objc2_web_kit::WKWebView, FileUploadRequest, FileUploadResponder) + 'static,
  ) -> Self {
    self.platform_specific.file_upload_handler = Some(Box::new(handler));
    self
  }

  fn with_context_menu_handler(
    mut self,
    handler: impl Fn(
        &objc2_app_kit::NSEvent,
        Option<Retained<objc2_app_kit::NSMenu>>,
      ) -> Option<Retained<objc2_app_kit::NSMenu>>
      + 'static,
  ) -> Self {
    self.platform_specific.context_menu_handler = Some(Box::new(handler));
    self
  }
}

#[cfg(target_os = "ios")]
pub trait WebViewBuilderExtIos {
  /// Allows overriding the the keyboard accessory view on iOS.
  /// Returning `None` effectively removes the view.
  ///
  /// The closure parameter is the webview instance.
  ///
  /// The accessory view is the view that appears above the keyboard when a text input element is focused.
  /// It usually displays a view with "Done", "Next" buttons.
  fn with_input_accessory_view_builder<
    F: Fn(&objc2_ui_kit::UIView) -> Option<Retained<objc2_ui_kit::UIView>> + 'static,
  >(
    self,
    builder: F,
  ) -> Self;
  /// Whether to limit navigations to App-Bound Domains. This is necessary
  /// to enable Service Workers on iOS.
  ///
  /// Note: If you set limit_navigations to true
  /// make sure to add the following to Info.plist in the iOS project:
  /// ```xml
  /// <plist>
  /// <dict>
  /// 	<key>WKAppBoundDomains</key>
  /// 	<array>
  /// 		<string>localhost</string>
  /// 	</array>
  /// </dict>
  /// </plist>
  /// ```
  /// You should also add any additional domains which your app requests assets from.
  /// Assets served through custom protocols like Tauri's IPC are added to the
  /// list automatically. Available on iOS only.
  ///
  /// Default is false.
  ///
  /// See https://webkit.org/blog/10882/app-bound-domains/ and
  /// https://developer.apple.com/documentation/webkit/wkwebviewconfiguration/limitsnavigationstoappbounddomains
  fn with_limit_navigations_to_app_bound_domains(self, limit_navigations: bool) -> Self;
}

#[cfg(target_os = "ios")]
impl WebViewBuilderExtIos for WebViewBuilder<'_> {
  fn with_input_accessory_view_builder<
    F: Fn(&objc2_ui_kit::UIView) -> Option<Retained<objc2_ui_kit::UIView>> + 'static,
  >(
    mut self,
    builder: F,
  ) -> Self {
    self
      .platform_specific
      .input_accessory_view_builder
      .replace(Box::new(builder));
    self
  }
  fn with_limit_navigations_to_app_bound_domains(mut self, limit_navigations: bool) -> Self {
    self
      .platform_specific
      .limit_navigations_to_app_bound_domains = limit_navigations;
    self
  }
}

#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct PlatformSpecificWebViewAttributes {
  additional_browser_args: Option<String>,
  browser_accelerator_keys: bool,
  theme: Option<Theme>,
  use_https: bool,
  scroll_bar_style: ScrollBarStyle,
  browser_extensions_enabled: bool,
  browser_extension_startup_gate: Option<
    std::sync::Arc<
      dyn Fn(&ICoreWebView2Environment, &ICoreWebView2) -> windows_core::Result<()> + 'static,
    >,
  >,
  extension_path: Option<PathBuf>,
  default_context_menus: bool,
  environment: Option<ICoreWebView2Environment>,
  environment_created_handler: Option<std::sync::Arc<dyn Fn(&ICoreWebView2Environment) + 'static>>,
  profile_name: Option<String>,
  native_context_menu_handler: Option<
    Rc<dyn Fn(&ICoreWebView2Controller, &ICoreWebView2ContextMenuRequestedEventArgs) -> bool>,
  >,
  native_download_handler: Option<
    std::sync::Arc<dyn Fn(&ICoreWebView2Controller, &ICoreWebView2DownloadStartingEventArgs)>,
  >,
}

#[cfg(windows)]
impl Default for PlatformSpecificWebViewAttributes {
  fn default() -> Self {
    Self {
      additional_browser_args: None,
      browser_accelerator_keys: true, // This is WebView2's default behavior
      default_context_menus: true,    // This is WebView2's default behavior
      theme: None,
      use_https: false, // To match macOS & Linux behavior in the context of mixed content.
      scroll_bar_style: ScrollBarStyle::default(),
      browser_extensions_enabled: false,
      browser_extension_startup_gate: None,
      extension_path: None,
      environment: None,
      environment_created_handler: None,
      profile_name: None,
      native_context_menu_handler: None,
      native_download_handler: None,
    }
  }
}

#[cfg(windows)]
pub trait WebViewBuilderExtWindows {
  /// Opt in to a host-filtered native context menu. The event is handled by
  /// default; only a true return permits displaying the filtered native items.
  fn with_native_context_menu_handler(
    self,
    handler: impl Fn(&ICoreWebView2Controller, &ICoreWebView2ContextMenuRequestedEventArgs) -> bool
      + 'static,
  ) -> Self;
  /// Hands the original native download event to the host. Cancellation is
  /// set before invocation; the host must retain a deferral to resolve later.
  /// Explicit download denial always takes precedence over this opt-in hook.
  fn with_native_download_handler(
    self,
    handler: impl Fn(&ICoreWebView2Controller, &ICoreWebView2DownloadStartingEventArgs) + 'static,
  ) -> Self;

  /// Pass additional args to WebView2 upon creating the webview.
  ///
  /// ## Warning
  ///
  /// - Webview instances with different browser arguments must also have different [data directories](WebContext::new).
  /// - By default wry passes `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`
  ///   `--autoplay-policy=no-user-gesture-required` if autoplay is enabled
  ///   and `--proxy-server=<scheme>://<host>:<port>` if a proxy is set.
  ///   so if you use this method, you have to add these arguments yourself if you want to keep the same behavior.
  fn with_additional_browser_args<S: Into<String>>(self, additional_args: S) -> Self;

  /// Determines whether browser-specific accelerator keys are enabled. When this setting is set to
  /// `false`, it disables all accelerator keys that access features specific to a web browser.
  /// The default value is `true`. See the following link to know more details.
  ///
  /// Setting to `false` does nothing on WebView2 Runtime version before 92.0.902.0,
  /// see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10824-prerelease>
  ///
  /// <https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/winrt/microsoft_web_webview2_core/corewebview2settings#arebrowseracceleratorkeysenabled>
  fn with_browser_accelerator_keys(self, enabled: bool) -> Self;

  /// Determines whether the webview's default context menus are enabled. When this setting is set to `false`,
  /// it disables all context menus on the webview - menus on the window's native decorations for example are not affected.
  ///
  /// The default value is `true` (context menus are enabled).
  ///
  /// <https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/winrt/microsoft_web_webview2_core/corewebview2settings#aredefaultcontextmenusenabled>
  fn with_default_context_menus(self, enabled: bool) -> Self;

  /// Specifies the theme of webview2. This affects things like `prefers-color-scheme`.
  ///
  /// Defaults to [`Theme::Auto`] which will follow the OS defaults.
  ///
  /// Requires WebView2 Runtime version 101.0.1210.39 or higher, does nothing on older versions,
  /// see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10121039>
  fn with_theme(self, theme: Theme) -> Self;

  /// Determines whether the custom protocols should use `https://<scheme>.path/to/page` instead of the default `http://<scheme>.path/to/page`.
  ///
  /// Using a `http` scheme will allow mixed content when trying to fetch `http` endpoints
  /// and is therefore less secure but will match the behavior of the `<scheme>://path/to/page` protocols used on macOS and Linux.
  ///
  /// The default value is `false`.
  fn with_https_scheme(self, enabled: bool) -> Self;

  /// Specifies the native scrollbar style to use with webview2.
  /// CSS styles that modify the scrollbar are applied on top of the native appearance configured here.
  ///
  /// Defaults to [`ScrollBarStyle::Default`] which is the browser default used by Microsoft Edge.
  ///
  /// Requires WebView2 Runtime version 125.0.2535.41 or higher, does nothing on older versions,
  /// see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/?tabs=dotnetcsharp#10253541>
  ///
  /// ## Warning
  ///
  /// Webview instances with different scroll bar styles must also have different [data directories](WebContext::new).
  fn with_scroll_bar_style(self, style: ScrollBarStyle) -> Self;

  /// Retains the WebView2 extension-enablement setting for source compatibility.
  ///
  /// By default, extensions are disabled.
  ///
  /// Requires WebView2 Runtime version 120.0.2210.55 or higher, does nothing on older versions,
  /// see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10221055>
  ///
  /// ## Warning
  ///
  /// On Windows, construction with this setting enabled directly fails with
  /// [`Error::WebView2ExtensionsStartupFenceUnavailable`] before Wry
  /// initializes COM or creates a native child window. Trusted embedders must
  /// instead use [`Self::with_browser_extension_startup_gate`], so the exact
  /// environment/profile is authenticated before WebView initialization.
  ///
  /// Webview instances with different browser extensions enabled settings must also have different [data directories](WebContext::new).
  fn with_browser_extensions_enabled(self, enabled: bool) -> Self;

  /// Enables browser extensions only behind an embedder-owned startup gate.
  ///
  /// Wry invokes this gate after it has created the exact controller and
  /// obtained its profile-bound `ICoreWebView2`, but before WebView
  /// initialization, script installation, or initial navigation. Returning an
  /// error aborts construction through Wry's owned controller-cleanup path.
  /// The callback must authenticate and retain the exact environment/profile
  /// authority; merely returning `Ok(())` is not a safe product policy.
  ///
  /// A supplied environment must have been created through the same admitted
  /// mode. WebView2 exposes no environment-side readback for this option, so
  /// the embedder remains responsible for binding that fact to the environment
  /// it reuses.
  fn with_browser_extension_startup_gate(
    self,
    gate: impl Fn(&ICoreWebView2Environment, &ICoreWebView2) -> windows_core::Result<()> + 'static,
  ) -> Self;

  /// Retains an unpacked-extension path for source compatibility.
  ///
  /// On Windows, construction with a configured path fails with
  /// [`Error::WebView2ExtensionPathUnsupported`] before Wry initializes COM or
  /// creates a native child window. Extension installation must instead be
  /// performed by an authenticated, owner-aware native host. Enabling browser
  /// extensions does not bypass this safety boundary.
  ///
  /// The separate Unix builder API continues to configure WebKitGTK's web
  /// process extension directory.
  fn with_extensions_path(self, path: impl Into<PathBuf>) -> Self;

  /// Set the environment for the webview.
  /// Useful if you need to share the same environment, for instance when using the [`WebViewBuilder::with_new_window_req_handler`].
  ///
  /// A supplied COM environment has opaque extension-enablement state and
  /// bypasses Wry's environment-option construction. Extension-enabled reuse
  /// must therefore carry the same authenticated startup gate and an
  /// embedder-owned binding to the mode used for the original environment.
  fn with_environment(self, environment: ICoreWebView2Environment) -> Self;

  /// Observe the exact environment selected for this construction before Wry
  /// starts creating a controller. This is intended for embedders that must
  /// retain process-lifecycle and storage obligations even when a later
  /// controller or WebView initialization step fails.
  ///
  /// The callback is invoked for both newly-created and explicitly supplied
  /// environments. It must not call back into WebView construction.
  fn with_environment_created_handler(
    self,
    handler: impl Fn(&ICoreWebView2Environment) + 'static,
  ) -> Self;

  /// Set the WebView2 profile name for this webview. Webviews with different
  /// profile names within the same environment have isolated cookies, storage,
  /// IndexedDB, cache, and other site data, while sharing the runtime.
  ///
  /// When `None` (the default), the webview uses the unnamed default profile.
  ///
  /// See <https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/multi-profile-support>
  /// for the underlying WebView2 multi-profile feature.
  ///
  /// Profile names must follow the WebView2 naming rules (alphanumeric, `.`,
  /// `_`, `-`, ` `, up to 64 chars, not starting/ending with `.` or ` `).
  fn with_profile_name<S: Into<String>>(self, name: S) -> Self;
}

#[cfg(windows)]
impl WebViewBuilderExtWindows for WebViewBuilder<'_> {
  fn with_native_context_menu_handler(
    mut self,
    handler: impl Fn(&ICoreWebView2Controller, &ICoreWebView2ContextMenuRequestedEventArgs) -> bool
      + 'static,
  ) -> Self {
    self.platform_specific.native_context_menu_handler = Some(Rc::new(handler));
    self.platform_specific.default_context_menus = true;
    self
  }
  fn with_native_download_handler(
    mut self,
    handler: impl Fn(&ICoreWebView2Controller, &ICoreWebView2DownloadStartingEventArgs) + 'static,
  ) -> Self {
    self.platform_specific.native_download_handler = Some(std::sync::Arc::new(handler));
    self
  }

  fn with_additional_browser_args<S: Into<String>>(mut self, additional_args: S) -> Self {
    self.platform_specific.additional_browser_args = Some(additional_args.into());
    self
  }

  fn with_browser_accelerator_keys(mut self, enabled: bool) -> Self {
    self.platform_specific.browser_accelerator_keys = enabled;
    self
  }

  fn with_default_context_menus(mut self, enabled: bool) -> Self {
    self.platform_specific.default_context_menus = enabled;
    self
  }

  fn with_theme(mut self, theme: Theme) -> Self {
    self.platform_specific.theme = Some(theme);
    self
  }

  fn with_https_scheme(mut self, enabled: bool) -> Self {
    self.platform_specific.use_https = enabled;
    self
  }

  fn with_scroll_bar_style(mut self, style: ScrollBarStyle) -> Self {
    self.platform_specific.scroll_bar_style = style;
    self
  }

  fn with_browser_extensions_enabled(mut self, enabled: bool) -> Self {
    self.platform_specific.browser_extensions_enabled = enabled;
    self
  }

  fn with_browser_extension_startup_gate(
    mut self,
    gate: impl Fn(&ICoreWebView2Environment, &ICoreWebView2) -> windows_core::Result<()> + 'static,
  ) -> Self {
    self.platform_specific.browser_extensions_enabled = true;
    self.platform_specific.browser_extension_startup_gate = Some(std::sync::Arc::new(gate));
    self
  }

  fn with_extensions_path(mut self, path: impl Into<PathBuf>) -> Self {
    self.platform_specific.extension_path = Some(path.into());
    self
  }

  fn with_environment(mut self, environment: ICoreWebView2Environment) -> Self {
    self.platform_specific.environment.replace(environment);
    self
  }

  fn with_environment_created_handler(
    mut self,
    handler: impl Fn(&ICoreWebView2Environment) + 'static,
  ) -> Self {
    self.platform_specific.environment_created_handler = Some(std::sync::Arc::new(handler));
    self
  }

  fn with_profile_name<S: Into<String>>(mut self, name: S) -> Self {
    self.platform_specific.profile_name = Some(name.into());
    self
  }
}

#[cfg(target_os = "android")]
#[derive(Default)]
pub(crate) struct PlatformSpecificWebViewAttributes {
  on_webview_created: Option<
    std::sync::Arc<
      dyn Fn(prelude::Context) -> std::result::Result<(), jni::errors::Error>
        + Send
        + Sync
        + 'static,
    >,
  >,
  with_asset_loader: bool,
  asset_loader_domain: Option<String>,
  https_scheme: bool,
}

#[cfg(target_os = "android")]
pub trait WebViewBuilderExtAndroid {
  fn on_webview_created<
    F: Fn(prelude::Context<'_, '_>) -> std::result::Result<(), jni::errors::Error>
      + Send
      + Sync
      + 'static,
  >(
    self,
    f: F,
  ) -> Self;

  /// Use [WebViewAssetLoader](https://developer.android.com/reference/kotlin/androidx/webkit/WebViewAssetLoader)
  /// to load assets from Android's `asset` folder when using `with_url` as `<protocol>://assets/` (e.g.:
  /// `wry://assets/index.html`). Note that this registers a custom protocol with the provided
  /// String, similar to [`with_custom_protocol`], but also sets the WebViewAssetLoader with the
  /// necessary domain (which is fixed as `<protocol>.assets`). This cannot be used in conjunction
  /// to `with_custom_protocol` for Android, as it changes the way in which requests are handled.
  #[cfg(feature = "protocol")]
  fn with_asset_loader(self, protocol: String) -> Self;

  /// Determines whether the custom protocols should use `https://<scheme>.localhost` instead of the default `http://<scheme>.localhost`.
  ///
  /// Using a `http` scheme will allow mixed content when trying to fetch `http` endpoints
  /// and is therefore less secure but will match the behavior of the `<scheme>://localhost` protocols used on macOS and Linux.
  ///
  /// The default value is `false`.
  fn with_https_scheme(self, enabled: bool) -> Self;
}

#[cfg(target_os = "android")]
impl WebViewBuilderExtAndroid for WebViewBuilder<'_> {
  fn on_webview_created<
    F: Fn(prelude::Context<'_, '_>) -> std::result::Result<(), jni::errors::Error>
      + Send
      + Sync
      + 'static,
  >(
    mut self,
    f: F,
  ) -> Self {
    self.platform_specific.on_webview_created = Some(std::sync::Arc::new(f));
    self
  }

  #[cfg(feature = "protocol")]
  fn with_asset_loader(mut self, protocol: String) -> Self {
    // register custom protocol with empty Response return,
    // this is necessary due to the need of fixing a domain
    // in WebViewAssetLoader.
    self.attrs.custom_protocols.insert(
      protocol.clone(),
      Box::new(|_, _, api| {
        api.respond(Response::new(Vec::new()));
      }),
    );
    self.platform_specific.with_asset_loader = true;
    self.platform_specific.asset_loader_domain = Some(format!("{}.assets", protocol));
    self
  }

  fn with_https_scheme(mut self, enabled: bool) -> Self {
    self.platform_specific.https_scheme = enabled;
    self
  }
}

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd",
))]
#[derive(Default)]
pub(crate) struct PlatformSpecificWebViewAttributes {
  extension_path: Option<PathBuf>,
  related_view: Option<webkit2gtk::WebView>,
}

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd",
))]
pub trait WebViewBuilderExtUnix<'a> {
  /// Consume the builder and create the webview inside a GTK container widget, such as GTK window.
  ///
  /// - If the container is [`gtk::Box`], it is added using [`Box::pack_start(webview, true, true, 0)`](gtk::prelude::BoxExt::pack_start).
  /// - If the container is [`gtk::Fixed`], its [size request](gtk::prelude::WidgetExt::set_size_request) will be set using the (width, height) bounds passed in
  ///   and will be added to the container using [`Fixed::put`](gtk::prelude::FixedExt::put) using the (x, y) bounds passed in.
  /// - For all other containers, it will be added using [`gtk::prelude::ContainerExt::add`]
  ///
  /// # Panics:
  ///
  /// - Panics if [`gtk::init`] was not called in this thread.
  fn build_gtk<W>(self, widget: &'a W) -> Result<WebView>
  where
    W: gtk::prelude::IsA<gtk::Container>;

  /// Set the path from which to load extensions from.
  fn with_extensions_path(self, path: impl Into<PathBuf>) -> Self;

  /// Creates a new webview sharing the same web process with the provided webview.
  /// Useful if you need to link a webview to another, for instance when using the [`WebViewBuilder::with_new_window_req_handler`].
  fn with_related_view(self, webview: webkit2gtk::WebView) -> Self;
}

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd",
))]
impl<'a> WebViewBuilderExtUnix<'a> for WebViewBuilder<'a> {
  fn build_gtk<W>(self, widget: &'a W) -> Result<WebView>
  where
    W: gtk::prelude::IsA<gtk::Container>,
  {
    self.error?;

    InnerWebView::new_gtk(widget, self.attrs, self.platform_specific)
      .map(|webview| WebView { webview })
  }

  fn with_extensions_path(mut self, path: impl Into<PathBuf>) -> Self {
    self.platform_specific.extension_path = Some(path.into());
    self
  }

  fn with_related_view(mut self, webview: webkit2gtk::WebView) -> Self {
    self.platform_specific.related_view.replace(webview);
    self
  }
}

/// The fundamental type to present a [`WebView`].
///
/// [`WebViewBuilder`] / [`WebView`] are the basic building blocks to construct WebView contents and
/// scripts for those who prefer to control fine grained window creation and event handling.
/// [`WebView`] presents the actual WebView window and let you still able to perform actions on it.
pub struct WebView {
  webview: InnerWebView,
}

impl WebView {
  /// Returns the id of this webview.
  pub fn id(&self) -> WebViewId<'_> {
    self.webview.id()
  }

  /// Get the current url of the webview
  pub fn url(&self) -> Result<String> {
    self.webview.url()
  }

  /// Get the current native top-level document title, bounded before it is
  /// copied into Rust. `None` means the engine has no usable title yet (or the
  /// untrusted value exceeded the native allocation limit).
  pub fn document_title(&self) -> Result<Option<String>> {
    self.webview.document_title()
  }

  /// Evaluate and run javascript code.
  pub fn evaluate_script(&self, js: &str) -> Result<()> {
    self
      .webview
      .eval(js, None::<Box<dyn Fn(String) + Send + 'static>>)
  }

  /// Evaluate and run javascript code with callback function. The evaluation result will be
  /// serialized into a JSON string and passed to the callback function.
  ///
  /// Exception is ignored because of the limitation on windows. You can catch it yourself and return as string as a workaround.
  pub fn evaluate_script_with_callback(
    &self,
    js: &str,
    callback: impl Fn(String) + Send + 'static,
  ) -> Result<()> {
    self.webview.eval(js, Some(callback))
  }

  /// Launch print modal for the webview content.
  pub fn print(&self) -> Result<()> {
    self.webview.print()
  }

  /// Get a list of cookies for specific url.
  pub fn cookies_for_url(&self, url: &str) -> Result<Vec<cookie::Cookie<'static>>> {
    self.webview.cookies_for_url(url)
  }

  /// Get the list of cookies.
  ///
  /// ## Platform-specific
  ///
  /// - **Android**: Unsupported, always returns an empty [`Vec`].
  pub fn cookies(&self) -> Result<Vec<cookie::Cookie<'static>>> {
    self.webview.cookies()
  }

  /// Set a cookie for the webview.
  ///
  /// ## Platform-specific
  ///
  /// - **Android**: Not supported.
  pub fn set_cookie(&self, cookie: &cookie::Cookie<'_>) -> Result<()> {
    self.webview.set_cookie(cookie)
  }

  /// Delete a cookie for the webview.
  ///
  /// ## Platform-specific
  ///
  /// - **Android**: Not supported.
  pub fn delete_cookie(&self, cookie: &cookie::Cookie<'_>) -> Result<()> {
    self.webview.delete_cookie(cookie)
  }

  /// Open the web inspector which is usually called dev tool.
  ///
  /// ## Platform-specific
  ///
  /// - **Android / iOS:** Not supported.
  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn open_devtools(&self) {
    self.webview.open_devtools()
  }

  /// Close the web inspector which is usually called dev tool.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows / Android / iOS:** Not supported.
  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn close_devtools(&self) {
    self.webview.close_devtools()
  }

  /// Gets the devtool window's current visibility state.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows / Android / iOS:** Not supported.
  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn is_devtools_open(&self) -> bool {
    self.webview.is_devtools_open()
  }

  /// Set the webview zoom level
  ///
  /// ## Platform-specific:
  ///
  /// - **Android**: Not supported.
  /// - **macOS**: available on macOS 11+ only.
  /// - **iOS**: available on iOS 14+ only.
  pub fn zoom(&self, scale_factor: f64) -> Result<()> {
    self.webview.zoom(scale_factor)
  }

  /// Specify the webview background color.
  ///
  /// The color uses the RGBA format.
  ///
  /// ## Platform-specific:
  ///
  /// - **macOS**: Disables the default white WKWebView background via the `drawsBackground` KVC key
  ///   (same as the `transparent` feature) and sets `underPageBackgroundColor` (macOS 12+) for overscroll areas.
  /// - **Windows**:
  ///   - On Windows 7, transparency is not supported and the alpha value will be ignored.
  ///   - On Windows higher than 7: translucent colors are not supported so any alpha value other than `0` will be replaced by `255`
  pub fn set_background_color(&self, background_color: RGBA) -> Result<()> {
    self.webview.set_background_color(background_color)
  }

  /// Navigate to the specified url
  pub fn load_url(&self, url: &str) -> Result<()> {
    self.webview.load_url(url)
  }

  /// Reloads the current page.
  pub fn reload(&self) -> crate::Result<()> {
    self.webview.reload()
  }

  /// Go to the next page.
  pub fn go_forward(&self) -> Result<()> {
    self.webview.go_forward()
  }

  /// Go to the previous page.
  pub fn go_back(&self) -> Result<()> {
    self.webview.go_back()
  }

  pub fn can_go_forward(&self) -> Result<bool> {
    self.webview.can_go_forward()
  }

  pub fn can_go_back(&self) -> Result<bool> {
    self.webview.can_go_back()
  }

  /// Navigate to the specified url using the specified headers
  pub fn load_url_with_headers(&self, url: &str, headers: http::HeaderMap) -> Result<()> {
    self.webview.load_url_with_headers(url, headers)
  }

  /// Load html content into the webview
  pub fn load_html(&self, html: &str) -> Result<()> {
    self.webview.load_html(html)
  }

  /// Clear all browsing data
  pub fn clear_all_browsing_data(&self) -> Result<()> {
    self.webview.clear_all_browsing_data()
  }

  pub fn bounds(&self) -> Result<Rect> {
    self.webview.bounds()
  }

  /// Set the webview bounds.
  ///
  /// This is only effective if the webview was created as a child
  /// or created using [`WebViewBuilderExtUnix::new_gtk`] with [`gtk::Fixed`].
  pub fn set_bounds(&self, bounds: Rect) -> Result<()> {
    self.webview.set_bounds(bounds)
  }

  /// Shows or hides the webview.
  pub fn set_visible(&self, visible: bool) -> Result<()> {
    self.webview.set_visible(visible)
  }

  /// Try moving focus to the webview.
  pub fn focus(&self) -> Result<()> {
    self.webview.focus()
  }

  /// Try moving focus away from the webview back to the parent window.
  ///
  /// ## Platform-specific:
  ///
  /// - **Android**: Not implemented.
  pub fn focus_parent(&self) -> Result<()> {
    self.webview.focus_parent()
  }
}

/// An event describing drag and drop operations on the webview.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum DragDropEvent {
  /// A drag operation has entered the webview.
  Enter {
    /// List of paths that are being dragged onto the webview.
    paths: Vec<PathBuf>,
    /// Position of the drag operation, relative to the webview top-left corner.
    position: (i32, i32),
  },
  /// A drag operation is moving over the window.
  Over {
    /// Position of the drag operation, relative to the webview top-left corner.
    position: (i32, i32),
  },
  /// The file(s) have been dropped onto the window.
  Drop {
    /// List of paths that are being dropped onto the window.
    paths: Vec<PathBuf>,
    /// Position of the drag operation, relative to the webview top-left corner.
    position: (i32, i32),
  },
  /// The drag operation has been cancelled or left the window.
  Leave,
}

/// Get WebView/Webkit version on current platform.
#[cfg(feature = "os-webview")]
#[cfg_attr(docsrs, doc(cfg(feature = "os-webview")))]
pub fn webview_version() -> Result<String> {
  platform_webview_version()
}

/// The [memory usage target level][1]. There are two levels 'Low' and 'Normal' and the default
/// level is 'Normal'. When the application is going inactive, setting the level to 'Low' can
/// significantly reduce the application's memory consumption.
///
/// [1]: https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2memoryusagetargetlevel
#[cfg(target_os = "windows")]
#[non_exhaustive]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryUsageLevel {
  /// The 'Normal' memory usage. Applications should set this level when they are becoming active.
  #[default]
  Normal,
  /// The 'Low' memory usage. Applications can reduce memory comsumption by setting this level when
  /// they are becoming inactive.
  Low,
}

/// Additional methods on `WebView` that are specific to Windows.
#[cfg(target_os = "windows")]
pub trait WebViewExtWindows {
  /// Returns the WebView2 controller.
  fn controller(&self) -> ICoreWebView2Controller;

  /// Webview environment.
  fn environment(&self) -> ICoreWebView2Environment;

  /// Webview instance.
  fn webview(&self) -> ICoreWebView2;

  /// Changes the webview2 theme.
  ///
  /// Requires WebView2 Runtime version 101.0.1210.39 or higher, returns error on older versions,
  /// see <https://learn.microsoft.com/en-us/microsoft-edge/webview2/release-notes/archive?tabs=dotnetcsharp#10121039>
  fn set_theme(&self, theme: Theme) -> Result<()>;

  /// Sets the [memory usage target level][1].
  ///
  /// When to best use this mode depends on the app in question. Most commonly it's called when
  /// the app's visiblity state changes.
  ///
  /// Please read the [guide for WebView2][2] for more details.
  ///
  /// This method uses a WebView2 API added in Runtime version 114.0.1823.32. When it is used in
  /// an older Runtime version, it does nothing.
  ///
  /// [1]: https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2memoryusagetargetlevel
  /// [2]: https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2.memoryusagetargetlevel?view=webview2-dotnet-1.0.2088.41#remarks
  fn set_memory_usage_level(&self, level: MemoryUsageLevel) -> Result<()>;

  /// Attaches this webview to the given HWND and removes it from the current one.
  fn reparent(&self, hwnd: isize) -> Result<()>;

  /// Returns the child HWND hosting this webview.
  fn hwnd(&self) -> windows::Win32::Foundation::HWND;

  /// Explicitly closes the controller, removes Wry's parent subclass, and
  /// destroys its container HWND. A failed native step is returned as an
  /// owned retryable debt instead of being hidden by `Drop`.
  fn close(&mut self) -> std::result::Result<(), WebView2CleanupDebt>;
}

#[cfg(target_os = "windows")]
impl WebViewExtWindows for WebView {
  fn controller(&self) -> ICoreWebView2Controller {
    self.webview.controller.clone()
  }

  fn environment(&self) -> ICoreWebView2Environment {
    self.webview.env.clone()
  }

  fn webview(&self) -> ICoreWebView2 {
    self.webview.webview.clone()
  }

  fn set_theme(&self, theme: Theme) -> Result<()> {
    self.webview.set_theme(theme)
  }

  fn set_memory_usage_level(&self, level: MemoryUsageLevel) -> Result<()> {
    self.webview.set_memory_usage_level(level)
  }

  fn reparent(&self, hwnd: isize) -> Result<()> {
    self.webview.reparent(hwnd)
  }

  /// Returns the child HWND hosting this webview.
  fn hwnd(&self) -> windows::Win32::Foundation::HWND {
    self.webview.hwnd()
  }

  fn close(&mut self) -> std::result::Result<(), WebView2CleanupDebt> {
    self.webview.close_explicit()
  }
}

/// Additional methods on `WebView` that are specific to Linux.
#[cfg(gtk)]
pub trait WebViewExtUnix: Sized {
  /// Create the webview inside a GTK container widget, such as GTK window.
  ///
  /// - If the container is [`gtk::Box`], it is added using [`Box::pack_start(webview, true, true, 0)`](gtk::prelude::BoxExt::pack_start).
  /// - If the container is [`gtk::Fixed`], its [size request](gtk::prelude::WidgetExt::set_size_request) will be set using the (width, height) bounds passed in
  ///   and will be added to the container using [`Fixed::put`](gtk::prelude::FixedExt::put) using the (x, y) bounds passed in.
  /// - For all other containers, it will be added using [`gtk::prelude::ContainerExt::add`]
  ///
  /// # Panics:
  ///
  /// - Panics if [`gtk::init`] was not called in this thread.
  fn new_gtk<W>(widget: &W) -> Result<Self>
  where
    W: gtk::prelude::IsA<gtk::Container>;

  /// Returns Webkit2gtk Webview handle
  fn webview(&self) -> webkit2gtk::WebView;

  /// Attaches this webview to the given Widget and removes it from the current one.
  fn reparent<W>(&self, widget: &W) -> Result<()>
  where
    W: gtk::prelude::IsA<gtk::Container>;
}

#[cfg(gtk)]
impl WebViewExtUnix for WebView {
  fn new_gtk<W>(widget: &W) -> Result<Self>
  where
    W: gtk::prelude::IsA<gtk::Container>,
  {
    WebViewBuilder::new().build_gtk(widget)
  }

  fn webview(&self) -> webkit2gtk::WebView {
    self.webview.webview.clone()
  }

  fn reparent<W>(&self, widget: &W) -> Result<()>
  where
    W: gtk::prelude::IsA<gtk::Container>,
  {
    self.webview.reparent(widget)
  }
}

/// Additional methods on `WebView` that are specific to macOS or iOS.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub trait WebViewExtDarwin {
  /// Prints with extra options
  fn print_with_options(&self, options: &PrintOptions) -> Result<()>;
  /// Fetches all Data Store Identifiers of this application
  ///
  /// Needs to run on main thread and needs an event loop to run.
  fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(cb: F) -> Result<()>;
  /// Deletes a Data Store by an identifier.
  ///
  /// You must drop any WebView instances using the data store before you call this method.
  ///
  /// Needs to run on main thread and needs an event loop to run.
  fn remove_data_store<F: FnOnce(Result<()>) + Send + 'static>(uuid: &[u8; 16], cb: F);
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl WebViewExtDarwin for WebView {
  fn print_with_options(&self, options: &PrintOptions) -> Result<()> {
    self.webview.print_with_options(options)
  }

  fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(cb: F) -> Result<()> {
    wkwebview::InnerWebView::fetch_data_store_identifiers(cb)
  }

  fn remove_data_store<F: FnOnce(Result<()>) + Send + 'static>(uuid: &[u8; 16], cb: F) {
    wkwebview::InnerWebView::remove_data_store(uuid, cb)
  }
}

/// Additional methods on `WebView` that are specific to macOS.
#[cfg(target_os = "macos")]
pub trait WebViewExtMacOS {
  /// Observes only the exact host-owned subscription rule list. Returns false
  /// if the optional private WebKit action class/getter is unavailable.
  fn set_content_block_counter(
    &self,
    identifier: &str,
    aggregate: std::sync::Arc<(
      std::sync::atomic::AtomicU64,
      std::sync::atomic::AtomicBool,
      std::sync::atomic::AtomicBool,
    )>,
  ) -> bool;
  /// Transfers the plain per-view counter without inspecting request metadata.
  fn collect_content_block_counter(&self, reset: bool);

  /// Returns WKWebView handle
  fn webview(&self) -> Retained<WryWebView>;
  /// Returns WKWebView manager [(userContentController)](https://developer.apple.com/documentation/webkit/wkscriptmessagehandler/1396222-usercontentcontroller) handle
  fn manager(&self) -> Retained<WKUserContentController>;
  /// Returns the last NSWindow to which construction or reparenting
  /// successfully attached this WKWebView. Wry retains the handle so it stays
  /// valid while AppKit temporarily detaches the view during teardown.
  fn ns_window(&self) -> Retained<NSWindow>;
  /// Attaches this webview to the given NSWindow and removes it from the current one.
  fn reparent(&self, window: *mut NSWindow) -> Result<()>;
  /// Prints with extra options
  fn print_with_options(&self, options: &PrintOptions) -> Result<()>;
  /// Move the window controls to the specified position.
  /// Normally this is handled by the Window but because `WebViewBuilder::build()` overwrites the window's NSView the controls will flicker on resizing.
  /// Note: This method has no effects if the WebView is injected via `WebViewBuilder::build_as_child();` and there should be no flickers.
  /// Warning: Do not use this if your chosen window library does not support traffic light insets.
  /// Warning: Only use this in **decorated** windows with a **hidden titlebar**!
  fn set_traffic_light_inset<P: Into<dpi::Position>>(&self, position: P) -> Result<()>;
  /// Resolves one exact deferred permission request owned by this WebView.
  ///
  /// Returns `true` only when the request was still pending. `Default` is
  /// treated as denial; native prompting is never re-enabled by settlement.
  fn resolve_permission_request(
    &self,
    request: PermissionRequestId,
    response: PermissionResponse,
  ) -> bool;
}

#[cfg(target_os = "macos")]
impl WebViewExtMacOS for WebView {
  fn set_content_block_counter(
    &self,
    identifier: &str,
    aggregate: std::sync::Arc<(
      std::sync::atomic::AtomicU64,
      std::sync::atomic::AtomicBool,
      std::sync::atomic::AtomicBool,
    )>,
  ) -> bool {
    use objc2::{msg_send, sel, DefinedClass};
    let Some(class) = objc2::runtime::AnyClass::get(c"_WKContentRuleListAction") else {
      aggregate
        .2
        .store(false, std::sync::atomic::Ordering::Relaxed);
      return false;
    };
    let available: bool =
      unsafe { msg_send![class, instancesRespondToSelector: sel!(blockedLoad)] };
    if !available {
      aggregate
        .2
        .store(false, std::sync::atomic::Ordering::Relaxed);
      return false;
    }
    let mut slot = self
      .webview
      .navigation_policy_delegate
      .ivars()
      .blocked_loads
      .borrow_mut();
    let identifier = objc2_foundation::NSString::from_str(identifier);
    if slot.as_ref().is_some_and(|c| {
      c.identifier.isEqualToString(&identifier) && std::sync::Arc::ptr_eq(&c.aggregate, &aggregate)
    }) {
      return true;
    }
    let previous_identifier = slot
      .as_ref()
      .filter(|c| std::sync::Arc::ptr_eq(&c.aggregate, &aggregate))
      .map(|c| c.identifier.clone());
    *slot = Some(crate::wkwebview::BlockedLoadCounter {
      identifier,
      previous_identifier,
      count: Default::default(),
      aggregate,
    });
    drop(slot);
    // WebKit caches optional delegate capabilities when this property is set.
    unsafe {
      self
        .webview
        .webview
        .setNavigationDelegate(Some(objc2::runtime::ProtocolObject::from_ref(
          &*self.webview.navigation_policy_delegate,
        )));
    }
    true
  }
  fn collect_content_block_counter(&self, reset: bool) {
    use objc2::DefinedClass;
    if let Some(counter) = self
      .webview
      .navigation_policy_delegate
      .ivars()
      .blocked_loads
      .borrow()
      .as_ref()
    {
      counter.flush(reset);
    }
  }

  fn webview(&self) -> Retained<WryWebView> {
    self.webview.webview.clone()
  }

  fn manager(&self) -> Retained<WKUserContentController> {
    self.webview.manager.clone()
  }

  fn ns_window(&self) -> Retained<NSWindow> {
    self.webview.ns_window.get()
  }

  fn reparent(&self, window: *mut NSWindow) -> Result<()> {
    self.webview.reparent(window)
  }

  fn print_with_options(&self, options: &PrintOptions) -> Result<()> {
    self.webview.print_with_options(options)
  }

  fn set_traffic_light_inset<P: Into<dpi::Position>>(&self, position: P) -> Result<()> {
    self.webview.set_traffic_light_inset(position.into())
  }

  fn resolve_permission_request(
    &self,
    request: PermissionRequestId,
    response: PermissionResponse,
  ) -> bool {
    self.webview.resolve_permission_request(request, response)
  }
}

/// Additional methods on `WebView` that are specific to iOS.
#[cfg(target_os = "ios")]
pub trait WebViewExtIOS {
  /// Returns WKWebView handle
  fn webview(&self) -> Retained<WryWebView>;
  /// Returns WKWebView manager [(userContentController)](https://developer.apple.com/documentation/webkit/wkscriptmessagehandler/1396222-usercontentcontroller) handle
  fn manager(&self) -> Retained<WKUserContentController>;
}

#[cfg(target_os = "ios")]
impl WebViewExtIOS for WebView {
  fn webview(&self) -> Retained<WryWebView> {
    self.webview.webview.clone()
  }

  fn manager(&self) -> Retained<WKUserContentController> {
    self.webview.manager.clone()
  }
}

#[cfg(target_os = "android")]
/// Additional methods on `WebView` that are specific to Android
pub trait WebViewExtAndroid {
  fn handle(&self) -> JniHandle;
}

#[cfg(target_os = "android")]
impl WebViewExtAndroid for WebView {
  fn handle(&self) -> JniHandle {
    JniHandle {
      activity_id: self.webview.activity_id,
    }
  }
}

/// WebView theme.
#[derive(Debug, Clone, Copy)]
pub enum Theme {
  /// Dark
  Dark,
  /// Light
  Light,
  /// System preference
  Auto,
}

/// Type alias for a color in the RGBA format.
///
/// Each value can be 0..255 inclusive.
pub type RGBA = (u8, u8, u8, u8);

/// Type of of page loading event
pub enum PageLoadEvent {
  /// Indicates that the content of the page has started loading
  Started,
  /// Indicates that the page content has finished loading
  Finished,
}

/// Opaque identity of one main-frame navigation within a WebView.
///
/// Values are meaningful only for correlating callbacks emitted by the same
/// WebView. They must not be persisted or compared across WebViews.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NavigationId(u64);

impl NavigationId {
  /// Construct an opaque navigation identity from a platform value.
  ///
  /// This is public for embedders implementing custom platform adapters and
  /// for deterministic state-machine tests. Ordinary consumers should use
  /// identities received through [`NavigationEvent`].
  pub const fn from_raw(raw: u64) -> Self {
    Self(raw)
  }
}

/// A phase in one identity-bearing main-frame navigation sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationEventPhase {
  /// A new provisional main-frame navigation started.
  Started,
  /// The same provisional navigation received a server redirect.
  ///
  /// Some native APIs expose the redirect phase and identity but not its
  /// destination URL. In that case [`NavigationEvent::url`] repeats the last
  /// safely attributed provisional URL; `Committed` always carries the
  /// authoritative final response URL.
  Redirected,
  /// Main-frame content began arriving and the URL became authoritative.
  Committed,
  /// The committed navigation completed successfully.
  Finished,
  /// Native policy or user cancellation ended navigation without a document.
  /// This is not a successful commit or a successful download.
  Cancelled,
  /// The navigation terminated with an error.
  Failed,
}

/// Why a main-frame navigation failed, as a category safe to show a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationFailure {
  /// The device has no network connection.
  Offline,
  /// The host name did not resolve.
  HostNotFound,
  /// The host was found but refused or dropped the connection.
  Unreachable,
  /// The server did not answer in time.
  TimedOut,
  /// A secure connection could not be established or verified.
  Insecure,
  /// Any other failure.
  Other,
}

/// Identity-bearing observation of a main-frame navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationEvent {
  /// Opaque per-WebView navigation identity.
  pub id: NavigationId,
  /// Native navigation phase.
  pub phase: NavigationEventPhase,
  /// Bounded URL safely attributed to this native navigation phase. See
  /// [`NavigationEventPhase::Redirected`] for the platform fallback.
  pub url: String,
}

/// Native cause of an Apple WebKit navigation policy decision.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppleNavigationType {
  /// A user or page activated a link.
  LinkActivated,
  /// A form submitted for the first time.
  FormSubmitted,
  /// WebKit requested a back/forward-list traversal.
  BackForward,
  /// The current document was reloaded.
  Reload,
  /// A form submission was replayed.
  FormResubmitted,
  /// WebKit reported another navigation cause.
  Other,
}

/// Bounded native provenance accompanying an Apple navigation policy request.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppleNavigationAction {
  /// Native navigation cause.
  pub navigation_type: AppleNavigationType,
  /// Whether the request method is exactly `GET`.
  pub is_get: bool,
  /// Whether WebKit identified the target as the main frame. `None` denotes a
  /// target-less/new-window action and must not be treated as main-frame work.
  pub target_is_main_frame: Option<bool>,
}

/// Background throttling policy
#[derive(Debug, Clone)]
pub enum BackgroundThrottlingPolicy {
  /// A policy where background throttling is disabled
  Disabled,
  /// A policy where a web view that's not in a window fully suspends tasks.
  Suspend,
  /// A policy where a web view that's not in a window limits processing, but does not fully suspend tasks.
  Throttle,
}

/// An initialization script
#[derive(Debug, Clone)]
pub struct InitializationScript {
  /// The script to run
  pub script: String,
  /// Whether the script should be injected to main frame only.
  ///
  /// When set to false, the script is also injected to subframes.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: scripts are always injected into subframes regardless of this option.
  ///   This will be the case until Webview2 implements a proper API to inject a script only on the main frame.
  /// - **Android**: When [addDocumentStartJavaScript] is not supported, scripts are always injected into main frame only.
  ///
  /// [addDocumentStartJavaScript]: https://developer.android.com/reference/androidx/webkit/WebViewCompat#addDocumentStartJavaScript(android.webkit.WebView,java.lang.String,java.util.Set%3Cjava.lang.String%3E)
  pub for_main_frame_only: bool,
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::{Arc, Mutex};

  #[cfg(target_os = "macos")]
  #[test]
  fn macos_permission_handlers_are_mutually_exclusive_and_last_writer_wins() {
    let brokered = WebViewBuilder::new()
      .with_permission_handler(|_| PermissionResponse::Allow)
      .with_permission_request_handler(|_| PermissionRequestDisposition::Defer);
    assert!(brokered.attrs.permission_handler.is_none());
    assert!(brokered
      .platform_specific
      .permission_request_handler
      .is_some());

    let deny_only = brokered.with_permission_handler(|_| PermissionResponse::Deny);
    assert!(deny_only.attrs.permission_handler.is_some());
    assert!(deny_only
      .platform_specific
      .permission_request_handler
      .is_none());
  }

  #[test]
  fn async_responder_completes_explicit_response_exactly_once() {
    let completions = Arc::new(Mutex::new(Vec::new()));
    let recorded = completions.clone();
    let responder = RequestAsyncResponder {
      responder: Some(Box::new(move |response| {
        if let Ok(mut completions) = recorded.lock() {
          completions.push((response.status(), response.into_body().into_owned()));
        }
      })),
    };

    responder.respond(Response::new(Vec::from(&b"ok"[..])));

    let completions = completions.lock().expect("completion lock poisoned");
    assert_eq!(
      completions.as_slice(),
      &[(http::StatusCode::OK, b"ok".to_vec())]
    );
  }

  #[test]
  fn dropped_async_responder_completes_with_internal_error() {
    let completions = Arc::new(Mutex::new(Vec::new()));
    let recorded = completions.clone();
    let responder = RequestAsyncResponder {
      responder: Some(Box::new(move |response| {
        if let Ok(mut completions) = recorded.lock() {
          completions.push(response.status());
        }
      })),
    };

    drop(responder);

    let completions = completions.lock().expect("completion lock poisoned");
    assert_eq!(
      completions.as_slice(),
      &[http::StatusCode::INTERNAL_SERVER_ERROR]
    );
  }

  #[test]
  fn download_deny_policy_never_materializes_or_delivers_metadata() {
    let materialized = std::cell::Cell::new(false);
    let delivered = DownloadPolicy::DenyWithoutMetadata.inspect_metadata(|| {
      materialized.set(true);
      ("page URL".to_owned(), std::path::PathBuf::from("page-name"))
    });

    assert!(delivered.is_none());
    assert!(!materialized.get());
  }

  #[test]
  fn download_metadata_policy_is_independent_of_callback_builder_order() {
    let before = WebViewBuilder::new()
      .with_download_started_handler(|_, _| true)
      .with_download_policy(DownloadPolicy::DenyWithoutMetadata);
    let after = WebViewBuilder::new()
      .with_download_policy(DownloadPolicy::DenyWithoutMetadata)
      .with_download_started_handler(|_, _| true);

    assert_eq!(
      before.attrs.download_policy,
      DownloadPolicy::DenyWithoutMetadata
    );
    assert_eq!(
      after.attrs.download_policy,
      DownloadPolicy::DenyWithoutMetadata
    );
  }

  #[test]
  fn page_close_cannot_destroy_the_native_container_without_explicit_opt_in() {
    let default = WebViewBuilder::new();
    assert_eq!(default.attrs.page_close_policy, PageClosePolicy::Ignore);

    let opted_in = WebViewBuilder::new().with_page_close_policy(PageClosePolicy::DestroyContainer);
    assert_eq!(
      opted_in.attrs.page_close_policy,
      PageClosePolicy::DestroyContainer
    );
  }

  #[test]
  fn native_media_surfaces_are_independently_configurable_per_view() {
    let denied = WebViewBuilder::new()
      .with_fullscreen_enabled(false)
      .with_picture_in_picture_enabled(false);
    assert!(!denied.attrs.fullscreen_enabled);
    assert!(!denied.attrs.picture_in_picture_enabled);

    let allowed = WebViewBuilder::new()
      .with_fullscreen_enabled(true)
      .with_picture_in_picture_enabled(true);
    assert!(allowed.attrs.fullscreen_enabled);
    assert!(allowed.attrs.picture_in_picture_enabled);
  }

  #[test]
  fn navigation_guard_requires_initial_presentation_concealment() {
    let ordinary = WebViewBuilder::new().with_visible(true).with_focused(true);
    assert!(!ordinary.attrs.guards_initial_presentation());
    assert!(ordinary.attrs.maps_during_initial_construction());
    assert!(ordinary.attrs.focuses_during_initial_construction());

    let hidden = WebViewBuilder::new()
      .with_visible(false)
      .with_focused(false);
    assert!(!hidden.attrs.maps_during_initial_construction());
    assert!(!hidden.attrs.focuses_during_initial_construction());

    let guarded = WebViewBuilder::new()
      .with_visible(true)
      .with_focused(true)
      .with_navigation_presentation_guard(|| {});
    assert!(guarded.attrs.guards_initial_presentation());
    assert!(!guarded.attrs.maps_during_initial_construction());
    assert!(!guarded.attrs.focuses_during_initial_construction());
  }

  #[test]
  fn webkitgtk_keeps_guarded_construction_unmapped_until_embedder_stage() {
    let source = include_str!("webkitgtk/mod.rs");
    let constructor = source
      .split("pub fn new_gtk<W>(")
      .nth(1)
      .and_then(|source| source.split("fn create_webview(").next())
      .expect("WebKitGTK constructor");
    let policy = constructor
      .find("let guards_initial_presentation = attributes.guards_initial_presentation();")
      .expect("retained construction presentation policy");
    let mapping_policy = constructor
      .find("let maps_during_initial_construction = attributes.maps_during_initial_construction();")
      .expect("retained construction mapping policy");
    let focus_policy = constructor
      .find(
        "let focuses_during_initial_construction = attributes.focuses_during_initial_construction();",
      )
      .expect("retained construction focus policy");
    let conceal = constructor[mapping_policy..]
      .find("if guards_initial_presentation {")
      .map(|offset| mapping_policy + offset)
      .expect("construction presentation guard");
    let sensitivity = constructor[conceal..]
      .find("webview.set_sensitive(false)")
      .map(|offset| conceal + offset)
      .expect("construction-time input revocation");
    let child_visibility = constructor[conceal..]
      .find("webview.set_child_visible(false)")
      .map(|offset| conceal + offset)
      .expect("construction-time mapping barrier");
    let opacity = constructor[conceal..]
      .find("webview.set_opacity(0.0)")
      .map(|offset| conceal + offset)
      .expect("construction-time paint revocation");
    let hide = constructor[conceal..]
      .find("webview.hide()")
      .map(|offset| conceal + offset)
      .expect("construction-time ordinary visibility revocation");
    let parenting = constructor
      .find("Self::add_to_container(&webview")
      .expect("WebKitGTK parenting");
    let post_parent = constructor[parenting..]
      .find("if guards_initial_presentation {")
      .map(|offset| parenting + offset)
      .expect("post-parenting presentation guard");
    let post_parent_barrier = constructor[post_parent..]
      .find("webview.set_child_visible(false)")
      .map(|offset| post_parent + offset)
      .expect("post-parenting mapping barrier");
    let post_parent_hide = constructor[post_parent..]
      .find("webview.hide()")
      .map(|offset| post_parent + offset)
      .expect("post-parenting ordinary visibility revocation");
    let navigation = constructor
      .find("web_context.load_uri(w.webview.clone()")
      .expect("WebKitGTK initial navigation");
    let mapping_policy_gate = constructor
      .find("if maps_during_initial_construction {")
      .expect("non-guarded construction mapping gate");
    let mapping = constructor[mapping_policy_gate..]
      .find("w.webview.show_all()")
      .map(|offset| mapping_policy_gate + offset)
      .expect("WebKitGTK mapping");
    let focus_policy_gate = constructor
      .find("if focuses_during_initial_construction {")
      .expect("non-guarded construction focus gate");
    let focus = constructor[focus_policy_gate..]
      .find("w.webview.grab_focus()")
      .map(|offset| focus_policy_gate + offset)
      .expect("WebKitGTK focus request");

    assert!(policy < mapping_policy);
    assert!(mapping_policy < focus_policy);
    assert!(focus_policy < conceal);
    assert!(conceal < sensitivity);
    assert!(sensitivity < child_visibility);
    assert!(child_visibility < opacity);
    assert!(opacity < hide);
    assert!(hide < parenting);
    assert!(parenting < post_parent_barrier);
    assert!(post_parent_barrier < post_parent_hide);
    assert!(post_parent_hide < navigation);
    assert!(navigation < mapping_policy_gate);
    assert!(mapping_policy_gate < mapping);
    assert!(mapping < focus_policy_gate);
    assert!(focus_policy_gate < focus);
    assert_eq!(constructor.matches("w.webview.show_all()").count(), 1);
    assert_eq!(constructor.matches("w.webview.grab_focus()").count(), 1);
    assert!(!constructor.contains("if attributes.visible {\n      w.webview.show_all();"));
    assert!(!constructor.contains("if attributes.focused {\n      w.webview.grab_focus();"));
  }

  #[test]
  fn webkitgtk_x11_wrapper_does_not_map_a_guarded_constructor() {
    let source = include_str!("webkitgtk/mod.rs");
    let constructor = source
      .split("fn new_x11<W: HasWindowHandle>(")
      .nth(1)
      .and_then(|source| source.split("fn create_container_x11_window(").next())
      .expect("WebKitGTK X11 constructor");
    let policy = constructor
      .find("let guards_initial_presentation = attributes.guards_initial_presentation();")
      .expect("X11 retained construction policy");
    let build = constructor
      .find("Self::new_gtk(&vbox, attributes, pl_attrs)")
      .expect("inner GTK construction");
    let mapping_gate = constructor
      .find("if !guards_initial_presentation {")
      .expect("guarded X11 mapping gate");
    let mapping = constructor[mapping_gate..]
      .find("gtk_window.show_all()")
      .map(|offset| mapping_gate + offset)
      .expect("X11 mapping");

    assert!(policy < build);
    assert!(build < mapping_gate);
    assert!(mapping_gate < mapping);
    assert_eq!(
      constructor
        .matches("\n        gtk_window.show_all();")
        .count(),
      1
    );
  }

  #[test]
  fn webkitgtk_x11_native_child_respects_the_initial_mapping_guard() {
    let source = include_str!("webkitgtk/mod.rs");
    let constructor = source
      .split("fn create_container_x11_window(")
      .nth(1)
      .and_then(|source| source.split("pub fn create_gtk_window(").next())
      .expect("WebKitGTK native X11 child constructor");
    let mapping_gate = constructor
      .find("if attributes.maps_during_initial_construction() {")
      .expect("native X11 initial mapping gate");
    let mapping = constructor[mapping_gate..]
      .find("(xlib.XMapWindow)(display, window)")
      .map(|offset| mapping_gate + offset)
      .expect("native X11 child mapping");

    assert!(mapping_gate < mapping);
    assert!(!constructor.contains("if attributes.visible {"));
  }

  #[test]
  fn webkitgtk_commit_revokes_permit_input_and_paint_before_embedder_dispatch() {
    let source = include_str!("webkitgtk/mod.rs");
    assert!(
      !source.contains("set_sensitive(true)"),
      "Wry may revoke GTK input, but only the embedder's attributed stage may restore it"
    );
    let callback = source
      .split("webview.connect_load_changed(move |webview, load_event| {")
      .nth(1)
      .and_then(|source| source.split("let failed_handler").next())
      .expect("bounded WebKitGTK load-changed callback");
    let commit = callback
      .find("let (transition, native_committed) = match load_event {")
      .expect("raw native commit marker");
    let guarded = &callback[commit..];
    let native_commit_arm = guarded
      .find("LoadEvent::Committed => (")
      .expect("raw native committed arm");
    let native_commit_arm_end = guarded[native_commit_arm..]
      .find("LoadEvent::Finished => (")
      .map(|offset| native_commit_arm + offset)
      .expect("end of raw native committed arm");
    let native_gate = guarded
      .find("if native_committed {")
      .expect("raw native commit concealment gate");
    let permit = guarded.find("guard();").expect("permit revocation");
    let sensitivity = guarded
      .find("webview.set_sensitive(false)")
      .expect("input revocation");
    let child_visibility = guarded
      .find("webview.set_child_visible(false)")
      .expect("mapping revocation");
    let opacity = guarded
      .find("webview.set_opacity(0.0)")
      .expect("paint concealment");
    let hide = guarded.find("webview.hide()").expect("ordinary GTK hide");
    let final_child_visibility = guarded[hide..]
      .find("webview.set_child_visible(false)")
      .map(|offset| hide + offset)
      .expect("terminal mapping barrier");
    let dispatch = guarded
      .find("changed_handler(NavigationEvent")
      .expect("embedder navigation dispatch");

    assert!(native_commit_arm < native_gate);
    assert!(guarded[native_commit_arm..native_commit_arm_end].contains("true,"));
    assert!(native_gate < permit);
    assert!(permit < sensitivity);
    assert!(sensitivity < child_visibility);
    assert!(child_visibility < opacity);
    assert!(opacity < hide);
    assert!(hide < final_child_visibility);
    assert!(final_child_visibility < dispatch);
    assert_eq!(
      guarded[..dispatch]
        .matches("webview.set_child_visible(false)")
        .count(),
      2
    );
    assert!(!guarded[..dispatch].contains("Some((_, NavigationEventPhase::Committed))"));
  }

  #[test]
  fn webkitgtk_container_routing_accepts_gtk_subclasses_on_every_host() {
    let source = include_str!("webkitgtk/mod.rs");
    let initial_parenting = source
      .split("fn add_to_container<W>(")
      .nth(1)
      .and_then(|source| source.split("fn attach_ipc_handler(").next())
      .expect("WebKitGTK initial container routing");
    let reparenting = source
      .split("pub fn reparent<W>(")
      .nth(1)
      .and_then(|source| source.split("pub fn platform_webview_version(").next())
      .expect("WebKitGTK reparent container routing");

    for routing in [initial_parenting, reparenting] {
      assert!(routing.contains("dynamic_cast_ref::<gtk::Box>()"));
      assert!(routing.contains("dynamic_cast_ref::<gtk::Fixed>()"));
      assert!(!routing.contains("container.type_().name()"));
      assert!(!routing.contains("container_type == \"GtkBox\""));
      assert!(!routing.contains("container_type == \"GtkFixed\""));
    }

    let set_bounds = source
      .split("pub fn set_bounds(&self, bounds: Rect)")
      .nth(1)
      .and_then(|source| source.split("fn set_visible_x11(").next())
      .expect("WebKitGTK bounds routing");
    assert!(source.contains("is_in_fixed_parent: Cell<bool>"));
    assert!(set_bounds.contains("self.is_in_fixed_parent.get()"));
    assert!(reparenting.contains("self.is_in_fixed_parent.set(false)"));
    assert!(reparenting.contains("self.is_in_fixed_parent.set(true)"));
    assert!(reparenting.contains("Err(Error::GtkReparentFailed)"));
  }

  #[test]
  fn webkitgtk_pathless_contexts_are_ephemeral_before_native_allocation() {
    let constructor = include_str!("webkitgtk/mod.rs")
      .split("pub fn new_gtk<W>(")
      .nth(1)
      .expect("WebKitGTK constructor");
    let incognito_check = constructor
      .find("if !context.context().is_ephemeral()")
      .expect("supplied-context incognito check");
    let allocate = constructor
      .find("Self::create_webview(web_context")
      .expect("native WebView allocation");
    let context_source = include_str!("webkitgtk/web_context.rs");
    let pathless = context_source
      .find("let Some(data_directory) = data_directory else")
      .expect("pathless context policy");
    let persistent_builder = context_source
      .find("let mut context_builder =")
      .expect("persistent context builder");

    assert!(incognito_check < allocate);
    assert!(pathless < persistent_builder);
    assert!(context_source.contains("return Self::create_ephemeral();"));
  }

  #[test]
  fn webkitgtk_linux_api_floor_and_bindings_are_guarded_on_every_host() {
    let source = include_str!("webkitgtk/mod.rs");
    let synthetic_mouse_source = include_str!("webkitgtk/synthetic_mouse_events.rs");
    let web_context_source = include_str!("webkitgtk/web_context.rs");
    let manifest = include_str!("../Cargo.toml");

    assert!(manifest
      .contains("webkit2gtk = { version = \"=2.0.2\", features = [\"v2_40\"], optional = true }"));
    assert!(manifest.contains(
      "webkit2gtk-sys = { version = \"=2.0.2\", features = [\"v2_40\"], optional = true }"
    ));
    assert!(source.contains("webkit_user_media_permission_is_for_display_device(request.as_ptr())"));
    assert!(source.contains("user_media_request_is_for_display_device(media_request)"));
    assert!(!source.contains("feature = \"v2_42\""));
    assert!(!source.contains(".run_javascript("));
    assert!(!synthetic_mouse_source.contains(".run_javascript("));
    assert!(source.contains(".evaluate_javascript(js, None, None, cancellable"));
    assert!(synthetic_mouse_source.contains("webview.evaluate_javascript("));
    assert!(web_context_source.contains("URIRequest, URIRequestExt,"));
  }

  #[test]
  #[cfg_attr(miri, ignore)]
  fn should_get_webview_version() {
    if let Err(error) = webview_version() {
      panic!("{}", error);
    }
  }
}
