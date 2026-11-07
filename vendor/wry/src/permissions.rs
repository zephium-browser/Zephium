use std::num::NonZeroU64;

/// Permission types that can be requested by the webview.
///
/// See [`crate::WebViewBuilder::with_permission_handler`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PermissionKind {
  /// Microphone access permission.
  Microphone,
  /// Camera access permission.
  Camera,
  /// Geolocation access permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION`.
  /// - **Linux**: Supported via `GeolocationPermissionRequest`.
  /// - **Android**: Supported via `WebChromeClient.onGeolocationPermissionsShowPrompt`.
  /// - **macOS**: Supported via WKUIDelegate's geolocation callback (public on
  ///   macOS 27, the private spelling before it).
  /// - **iOS**: Not yet supported by platform backends.
  Geolocation,
  /// Notifications permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS`.
  /// - **Linux**: Supported via `NotificationPermissionRequest`.
  /// - **macOS / Android / iOS**: Not yet supported by platform backends.
  Notifications,
  /// Clipboard read permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ`.
  /// - **macOS / Linux / Android / iOS**: Not yet supported by platform backends.
  ClipboardRead,
  /// Display capture permission (for getDisplayMedia).
  ///
  /// ## Platform-specific
  ///
  /// - **macOS / iOS**: Not routed by this backend. WebKit's public camera and
  ///   microphone delegate does not govern display capture. Returning `Deny`
  ///   from a permission handler does not prevent the system display picker.
  DisplayCapture,
  /// Midi access permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES`.
  /// - **Android**: Supported via `android.webkit.resource.MIDI_SYSEX`.
  /// - **macOS / Linux / iOS**: Not yet supported by platform backends.
  Midi,
  /// Sensors (accelerometer, gyroscope, etc.) access permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS`.
  /// - **macOS / Linux / Android / iOS**: Not yet supported by platform backends.
  Sensors,
  /// Media key system access permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Android**: Supported via `android.webkit.resource.PROTECTED_MEDIA_ID`.
  /// - **Windows / macOS / Linux / iOS**: Not yet supported by platform backends.
  MediaKeySystemAccess,
  /// Local fonts access permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS`.
  /// - **macOS / Linux / Android / iOS**: Not yet supported by platform backends.
  LocalFonts,
  /// Window management permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_WINDOW_MANAGEMENT`.
  /// - **macOS / Linux / Android / iOS**: Not yet supported by platform backends.
  WindowManagement,
  /// Pointer lock permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Linux**: Supported via `PointerLockPermissionRequest`.
  /// - **Windows / macOS / Android / iOS**: Not yet supported by platform backends.
  PointerLock,
  /// Automatic downloads permission (multiple downloads without user interaction).
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS`.
  /// - **macOS / Linux / Android / iOS**: Not yet supported by platform backends.
  AutomaticDownloads,
  /// File system access permission (read/write via File System Access API).
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE`.
  /// - **macOS / Linux / Android / iOS**: Not yet supported by platform backends.
  FileSystemAccess,
  /// Media autoplay permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: Supported via `COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY`.
  /// - **macOS / Linux / Android / iOS**: Not yet supported by platform backends.
  Autoplay,
  /// Other unrecognized permission type.
  Other,
}

/// Process-local identity of one deferred native permission request.
///
/// The identity is minted by the platform adapter, is never derived from web
/// content, and is meaningful only to the exact live [`crate::WebView`] that
/// emitted it. Embedders must still bind it to their own view generation and
/// navigation lifecycle before presenting a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PermissionRequestId(NonZeroU64);

impl PermissionRequestId {
  #[cfg(any(target_os = "macos", target_os = "ios"))]
  pub(crate) const fn new(value: NonZeroU64) -> Self {
    Self(value)
  }

  /// Reconstructs an opaque identity previously obtained from
  /// [`PermissionRequest::id`]. Unknown identities remain harmless: the
  /// platform settlement method returns `false` unless the exact WebView
  /// still owns a matching deferred completion.
  pub const fn from_u64(value: u64) -> Option<Self> {
    match NonZeroU64::new(value) {
      Some(value) => Some(Self(value)),
      None => None,
    }
  }

  /// Returns the opaque process-local integer identity.
  pub const fn get(self) -> u64 {
    self.0.get()
  }
}

/// Bounded native security-origin components attached to a permission request.
///
/// These values come from the platform WebView, not JavaScript. They are still
/// untrusted page metadata: Wry bounds them before allocation but deliberately
/// does not claim that they form an embedder-supported or canonical origin.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermissionOrigin {
  scheme: Box<str>,
  host: Box<str>,
  port: Option<u16>,
}

impl PermissionOrigin {
  #[cfg(any(target_os = "macos", target_os = "ios"))]
  pub(crate) fn new(scheme: String, host: String, port: Option<u16>) -> Self {
    Self {
      scheme: scheme.into_boxed_str(),
      host: host.into_boxed_str(),
      port,
    }
  }

  pub fn scheme(&self) -> &str {
    &self.scheme
  }

  pub fn host(&self) -> &str {
    &self.host
  }

  pub const fn port(&self) -> Option<u16> {
    self.port
  }
}

/// Native capability cohort represented by one exactly-once completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PermissionRequestKind {
  Single(PermissionKind),
  /// WebKit represents a combined camera-and-microphone request with one
  /// completion. Splitting it into independently settleable prompts would be
  /// dishonest, so the pair remains atomic at this boundary.
  CameraAndMicrophone,
}

/// One bounded, origin-labelled native permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
  id: PermissionRequestId,
  origin: PermissionOrigin,
  kind: PermissionRequestKind,
}

impl PermissionRequest {
  #[cfg(any(target_os = "macos", target_os = "ios"))]
  pub(crate) const fn new(
    id: PermissionRequestId,
    origin: PermissionOrigin,
    kind: PermissionRequestKind,
  ) -> Self {
    Self { id, origin, kind }
  }

  pub const fn id(&self) -> PermissionRequestId {
    self.id
  }

  pub const fn origin(&self) -> &PermissionOrigin {
    &self.origin
  }

  pub const fn kind(&self) -> PermissionRequestKind {
    self.kind
  }
}

/// Initial disposition returned by an origin-labelled permission broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionRequestDisposition {
  Allow,
  Deny,
  /// Retain the native completion until the exact request is resolved through
  /// the matching platform [`crate::WebView`] extension method.
  Defer,
}

impl std::fmt::Display for PermissionKind {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Microphone => write!(f, "microphone"),
      Self::Camera => write!(f, "camera"),
      Self::Geolocation => write!(f, "geolocation"),
      Self::Notifications => write!(f, "notifications"),
      Self::ClipboardRead => write!(f, "clipboard-read"),
      Self::DisplayCapture => write!(f, "display-capture"),
      Self::Midi => write!(f, "midi"),
      Self::Sensors => write!(f, "sensors"),
      Self::MediaKeySystemAccess => write!(f, "media-key-system-access"),
      Self::LocalFonts => write!(f, "local-fonts"),
      Self::WindowManagement => write!(f, "window-management"),
      Self::PointerLock => write!(f, "pointer-lock"),
      Self::AutomaticDownloads => write!(f, "automatic-downloads"),
      Self::FileSystemAccess => write!(f, "file-system-access"),
      Self::Autoplay => write!(f, "autoplay"),
      Self::Other => write!(f, "other"),
    }
  }
}

/// Response for permission requests.
///
/// See [`crate::WebViewBuilder::with_permission_handler`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PermissionResponse {
  /// Grant the permission.
  ///
  /// ## Platform-specific
  ///
  /// - **Android**: Not supported for runtime permissions; the normal Android
  ///   permission flow is used instead.
  Allow,
  /// Deny the permission.
  Deny,
  /// Use the platform or browser default behavior.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows / macOS / Linux**: Zephium's Wry fork treats this as
  ///   [`Self::Deny`] so raw pages cannot fall back to unbrokered native UI.
  /// - **Android**: The default behavior continues the platform permission flow.
  #[default]
  Default,
  /// Leave the request to the engine's own prompt.
  ///
  /// ## Platform-specific
  ///
  /// - **Windows**: WebView2 shows its built-in, origin-labelled prompt for
  ///   camera, microphone, geolocation, notifications, clipboard read, MIDI
  ///   system-exclusive messages, sensors, local fonts and multiple automatic
  ///   downloads; every other kind is denied.
  /// - **macOS / Linux / Android**: Treated as [`Self::Deny`].
  Prompt,
}

impl std::fmt::Display for PermissionResponse {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Allow => write!(f, "allow"),
      Self::Deny => write!(f, "deny"),
      Self::Default => write!(f, "default"),
      Self::Prompt => write!(f, "prompt"),
    }
  }
}
