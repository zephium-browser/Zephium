use std::{
  collections::{hash_map::Entry, HashMap, VecDeque},
  sync::{Mutex, OnceLock},
};

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::{DeclaredClass, Message};
use objc2_foundation::{
  MainThreadMarker, NSError, NSHTTPURLResponse, NSObjectProtocol, NSString,
  NSURLErrorFailingURLErrorKey, NSURL,
};
use objc2_web_kit::{
  WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationResponse,
  WKNavigationResponsePolicy, WKNavigationType,
};

#[cfg(target_os = "ios")]
use crate::wkwebview::ios::WKWebView::WKWebView;
#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;

use crate::{
  native_bounds::{bounded_nsstring, CUSTOM_PROTOCOL_METHOD_LIMIT, PAGE_URL_LIMIT},
  AppleNavigationAction, AppleNavigationType, NavigationEvent, NavigationEventPhase,
  NavigationFailure, NavigationId, PageLoadEvent,
};

use super::class::wry_navigation_delegate::WryNavigationDelegate;

const ACTIVE_APPLE_NAVIGATION_LIMIT: usize = 64;
const APPLE_REDIRECT_EVENT_LIMIT: usize = 32;
const WEB_EXTENSION_URL_PREFIX: &str = "webkit-extension://";

#[cfg(target_os = "macos")]
fn extension_tab_trace_enabled() -> bool {
  static ENABLED: OnceLock<bool> = OnceLock::new();
  *ENABLED.get_or_init(|| std::env::var("ZEPHIUM_EXTENSION_TAB_TRACE").as_deref() == Ok("1"))
}

#[cfg(target_os = "macos")]
fn navigation_origin_category(value: &str) -> &'static str {
  let Ok(url) = url::Url::parse(value) else {
    return "invalid";
  };
  if url.scheme() == "https"
    && url.host_str().is_some_and(|host| {
      host
        .strip_suffix(".chromiumapp.org")
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|byte| matches!(byte, b'a'..=b'p')))
    })
  {
    "chromiumapp_shaped"
  } else if url.scheme() == "https" {
    "other_https"
  } else if url.scheme() == "http" {
    "other_http"
  } else {
    "other_scheme"
  }
}

fn native_web_extension_subframe_owns_policy(
  url: &str,
  target_is_main_frame: Option<bool>,
) -> bool {
  target_is_main_frame == Some(false) && url.starts_with(WEB_EXTENSION_URL_PREFIX)
}

/// URL attribution for WKNavigation's public delegate contract.
///
/// `WKNavigationAction` and `WKNavigationResponse` do not expose the
/// `WKNavigation` object that later appears in lifecycle callbacks. A global
/// "next action/response" slot is therefore not a valid correlation mechanism:
/// two provisional loads can overlap and deliver their callbacks in either
/// order. Programmatic loads register the exact object returned by
/// `WKWebView::loadRequest`. Exact callbacks that re-enter before that method
/// returns are buffered by their native identity, never by callback order.
/// Page-driven loads are admitted by policy but are first attributed after
/// `didCommit`, where the callback supplies the exact navigation identity and
/// `WKWebView.URL` names the now-current page.
pub(crate) struct AppleNavigationEventState {
  active: HashMap<usize, AppleNavigationRecord>,
  deferred_order: VecDeque<(usize, u128)>,
  next_record_token: Option<u128>,
  programmatic_load_depth: usize,
  deferred_flush_scheduled: bool,
}

impl Default for AppleNavigationEventState {
  fn default() -> Self {
    Self {
      active: HashMap::with_capacity(ACTIVE_APPLE_NAVIGATION_LIMIT),
      deferred_order: VecDeque::with_capacity(ACTIVE_APPLE_NAVIGATION_LIMIT),
      next_record_token: Some(1),
      programmatic_load_depth: 0,
      deferred_flush_scheduled: false,
    }
  }
}

impl AppleNavigationEventState {
  fn begin_programmatic(&mut self) -> bool {
    let Some(depth) = self.programmatic_load_depth.checked_add(1) else {
      return false;
    };
    if depth > ACTIVE_APPLE_NAVIGATION_LIMIT {
      return false;
    }
    self.programmatic_load_depth = depth;
    true
  }

  fn cancel_programmatic(&mut self) -> AppleNavigationUpdate {
    self.programmatic_load_depth = self.programmatic_load_depth.saturating_sub(1);
    let mut update = AppleNavigationUpdate::default();
    self.settle_programmatic_boundary(&mut update);
    update
  }

  fn register_programmatic(&mut self, key: usize, url: String) -> AppleNavigationUpdate {
    self.programmatic_load_depth = self.programmatic_load_depth.saturating_sub(1);

    let mut update = AppleNavigationUpdate {
      registration_accepted: true,
      ..Default::default()
    };
    let Some(record) = self.record_or_insert(key) else {
      update.registration_accepted = false;
      self.settle_programmatic_boundary(&mut update);
      return update;
    };
    {
      if record.programmatic_url.is_some() || record.page_driven {
        update.registration_accepted = false;
      } else {
        record.programmatic_url = Some(url);
        update.events = drain_record(key, record);
        if record.terminal.is_some() {
          self.active.remove(&key);
        }
      }
    }
    self.settle_programmatic_boundary(&mut update);
    update
  }

  fn settle_programmatic_boundary(&mut self, update: &mut AppleNavigationUpdate) {
    if self.programmatic_load_depth == 0 {
      // Every bracketed loadRequest has now either registered its exact
      // WKNavigation or failed. Any other committed key is conclusively
      // page-driven, so publish it before returning to the native run loop and
      // before WebKit can paint it under the previous browser-chrome URL.
      let mut deferred = self.flush_deferred();
      update.events.append(&mut deferred.events);
      update.stop_loading |= deferred.stop_loading;
      update.schedule_deferred |= deferred.schedule_deferred;
    } else {
      update.schedule_deferred |= self.arm_deferred_flush_if_needed();
    }
  }

  fn started(&mut self, key: usize) -> AppleNavigationUpdate {
    let Some(record) = self.record_or_insert(key) else {
      return AppleNavigationUpdate::rejected();
    };
    if record.terminal.is_some() {
      return AppleNavigationUpdate::default();
    }
    record.start_observed = true;
    let events = drain_record(key, record);
    AppleNavigationUpdate {
      events,
      ..Default::default()
    }
  }

  fn redirected(&mut self, key: usize) -> AppleNavigationUpdate {
    let Some(record) = self.record_or_insert(key) else {
      return AppleNavigationUpdate::rejected();
    };
    if record.terminal.is_some() {
      return AppleNavigationUpdate::default();
    }
    if record.committed_url.is_some() || record.redirects_observed >= APPLE_REDIRECT_EVENT_LIMIT {
      record.terminal = Some(NavigationEventPhase::Failed);
      let events = drain_record(key, record);
      if record.programmatic_url.is_some() || record.page_driven {
        self.active.remove(&key);
      }
      self.queue_deferred_if_needed(key);
      let schedule_deferred = self.arm_deferred_flush_if_needed();
      return AppleNavigationUpdate {
        events,
        stop_loading: true,
        schedule_deferred,
        ..Default::default()
      };
    }
    record.start_observed = true;
    record.redirects_observed += 1;
    let events = drain_record(key, record);
    AppleNavigationUpdate {
      events,
      ..Default::default()
    }
  }

  fn committed(&mut self, key: usize, url: Option<String>) -> AppleNavigationUpdate {
    // Outside a bracketed `loadRequest` call, a programmatic navigation has
    // already registered the exact WKNavigation returned by WebKit before this
    // delegate callback can run. An unregistered commit is therefore
    // page-driven and can be attributed in this didCommit turn. Only the
    // genuinely re-entrant case must wait for `loadRequest` to return and bind
    // its exact object; deferring every page-driven commit would leave the old
    // browser-chrome URL over an already committed document for one main-queue
    // turn.
    let attribute_page_driven_now = self.programmatic_load_depth == 0;
    let Some(record) = self.record_or_insert(key) else {
      return AppleNavigationUpdate {
        guard_presentation: true,
        ..AppleNavigationUpdate::rejected()
      };
    };
    if record.terminal.is_some() {
      return AppleNavigationUpdate::default();
    }
    if record.committed_url.is_some() || url.is_none() {
      let first_commit = record.committed_url.is_none();
      record.start_observed = true;
      record.terminal = Some(NavigationEventPhase::Failed);
      let events = drain_record(key, record);
      if record.programmatic_url.is_some() || record.page_driven {
        self.active.remove(&key);
      }
      self.queue_deferred_if_needed(key);
      let schedule_deferred = self.arm_deferred_flush_if_needed();
      return AppleNavigationUpdate {
        events,
        stop_loading: true,
        schedule_deferred,
        guard_presentation: first_commit,
        ..Default::default()
      };
    }

    record.start_observed = true;
    record.committed_url = url;
    if attribute_page_driven_now && record.programmatic_url.is_none() {
      record.page_driven = true;
    }
    let events = drain_record(key, record);
    self.queue_deferred_if_needed(key);
    let schedule_deferred = self.arm_deferred_flush_if_needed();
    AppleNavigationUpdate {
      events,
      schedule_deferred,
      guard_presentation: true,
      ..Default::default()
    }
  }

  fn terminal(&mut self, key: usize, phase: NavigationEventPhase) -> AppleNavigationUpdate {
    if !self.active.contains_key(&key) && self.programmatic_load_depth == 0 {
      // There is no safely attributed URL to report, and no synchronous
      // loadRequest call that could still bind this exact identity. This also
      // makes duplicate terminal callbacks exactly-once no-ops.
      return AppleNavigationUpdate::default();
    }
    let Some(record) = self.record_or_insert(key) else {
      return AppleNavigationUpdate::default();
    };
    if record.terminal.is_some() {
      return AppleNavigationUpdate::default();
    }
    record.start_observed = true;
    record.terminal = Some(phase);
    let events = drain_record(key, record);
    if record.programmatic_url.is_some() || record.page_driven {
      self.active.remove(&key);
    }
    self.queue_deferred_if_needed(key);
    let schedule_deferred = self.arm_deferred_flush_if_needed();
    AppleNavigationUpdate {
      events,
      schedule_deferred,
      ..Default::default()
    }
  }

  fn flush_deferred(&mut self) -> AppleNavigationUpdate {
    self.deferred_flush_scheduled = false;
    if self.programmatic_load_depth != 0 {
      return AppleNavigationUpdate::default();
    }

    let mut events = Vec::new();
    while let Some((key, token)) = self.deferred_order.pop_front() {
      let Some(record) = self.active.get_mut(&key) else {
        continue;
      };
      if record.token != token {
        continue;
      }
      record.deferred_queued = false;
      if !record.needs_deferred_attribution() {
        continue;
      }
      if record.committed_url.is_some() {
        record.page_driven = true;
        events.extend(drain_record(key, record));
      }
      if record.terminal.is_some() {
        // A page-driven provisional failure with no committed URL has no URL
        // that can safely be attributed to this exact WKNavigation. Forget it
        // without manufacturing a browser-chrome observation.
        self.active.remove(&key);
      }
    }
    AppleNavigationUpdate {
      events,
      ..Default::default()
    }
  }

  fn record_or_insert(&mut self, key: usize) -> Option<&mut AppleNavigationRecord> {
    let has_capacity = self.active.len() < ACTIVE_APPLE_NAVIGATION_LIMIT;
    match self.active.entry(key) {
      Entry::Occupied(entry) => Some(entry.into_mut()),
      Entry::Vacant(entry) => {
        if !has_capacity {
          return None;
        }
        let token = self.next_record_token?;
        self.next_record_token = token.checked_add(1);
        Some(entry.insert(AppleNavigationRecord::new(token)))
      }
    }
  }

  fn queue_deferred_if_needed(&mut self, key: usize) {
    let Some(record) = self.active.get_mut(&key) else {
      return;
    };
    if !record.needs_deferred_attribution() || record.deferred_queued {
      return;
    }
    record.deferred_queued = true;
    self.deferred_order.push_back((key, record.token));
  }

  fn arm_deferred_flush_if_needed(&mut self) -> bool {
    if self.programmatic_load_depth != 0 || self.deferred_flush_scheduled {
      return false;
    }
    if self.deferred_order.is_empty() {
      return false;
    }
    self.deferred_flush_scheduled = true;
    true
  }

  fn clear(&mut self) {
    self.active.clear();
    self.deferred_order.clear();
    self.next_record_token = Some(1);
    self.programmatic_load_depth = 0;
    self.deferred_flush_scheduled = false;
  }
}

struct AppleNavigationRecord {
  token: u128,
  programmatic_url: Option<String>,
  committed_url: Option<String>,
  start_observed: bool,
  redirects_observed: usize,
  started_emitted: bool,
  redirects_emitted: usize,
  committed_emitted: bool,
  terminal: Option<NavigationEventPhase>,
  page_driven: bool,
  deferred_queued: bool,
}

impl AppleNavigationRecord {
  fn new(token: u128) -> Self {
    Self {
      token,
      programmatic_url: None,
      committed_url: None,
      start_observed: false,
      redirects_observed: 0,
      started_emitted: false,
      redirects_emitted: 0,
      committed_emitted: false,
      terminal: None,
      page_driven: false,
      deferred_queued: false,
    }
  }

  fn needs_deferred_attribution(&self) -> bool {
    self.programmatic_url.is_none()
      && !self.page_driven
      && (self.committed_url.is_some() || self.terminal.is_some())
  }
}

#[derive(Debug, PartialEq, Eq)]
struct AppleBufferedNavigationEvent {
  key: usize,
  phase: NavigationEventPhase,
  url: String,
}

#[derive(Default)]
struct AppleNavigationUpdate {
  events: Vec<AppleBufferedNavigationEvent>,
  stop_loading: bool,
  schedule_deferred: bool,
  registration_accepted: bool,
  guard_presentation: bool,
}

impl AppleNavigationUpdate {
  fn rejected() -> Self {
    Self {
      stop_loading: true,
      ..Default::default()
    }
  }
}

fn drain_record(
  key: usize,
  record: &mut AppleNavigationRecord,
) -> Vec<AppleBufferedNavigationEvent> {
  let start_url = if let Some(url) = record.programmatic_url.as_ref() {
    Some(url)
  } else if record.page_driven {
    record.committed_url.as_ref()
  } else {
    None
  }
  .cloned();
  let Some(start_url) = start_url else {
    return Vec::new();
  };

  let mut events = Vec::new();
  if !record.started_emitted
    && (record.start_observed || record.committed_url.is_some() || record.terminal.is_some())
  {
    events.push(AppleBufferedNavigationEvent {
      key,
      phase: NavigationEventPhase::Started,
      url: start_url.clone(),
    });
    record.started_emitted = true;
  }
  while record.redirects_emitted < record.redirects_observed {
    events.push(AppleBufferedNavigationEvent {
      key,
      phase: NavigationEventPhase::Redirected,
      url: start_url.clone(),
    });
    record.redirects_emitted += 1;
  }
  if !record.committed_emitted {
    if let Some(url) = record.committed_url.clone() {
      events.push(AppleBufferedNavigationEvent {
        key,
        phase: NavigationEventPhase::Committed,
        url,
      });
      record.committed_emitted = true;
    }
  }
  if let Some(native_terminal) = record.terminal {
    let phase = if record.committed_emitted || native_terminal == NavigationEventPhase::Cancelled {
      native_terminal
    } else {
      // A native "finished" callback without a preceding commit violates the
      // public delegate sequence. Normalize it to failure instead of claiming
      // a document became authoritative when no committed URL was observed.
      NavigationEventPhase::Failed
    };
    let url = record
      .committed_url
      .clone()
      .unwrap_or_else(|| start_url.clone());
    events.push(AppleBufferedNavigationEvent { key, phase, url });
  }
  events
}

fn navigation_key(navigation: &WKNavigation) -> usize {
  std::ptr::from_ref(navigation) as usize
}

fn with_navigation_state<R>(
  state: &Mutex<AppleNavigationEventState>,
  mutate: impl FnOnce(&mut AppleNavigationEventState) -> R,
) -> R {
  let mut state = state
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
  mutate(&mut state)
}

fn emit_navigation_event(this: &WryNavigationDelegate, event: AppleBufferedNavigationEvent) {
  let Some(handler) = &this.ivars().navigation_event_handler else {
    return;
  };
  handler(NavigationEvent {
    id: NavigationId::from_raw(event.key as u64),
    phase: event.phase,
    url: event.url,
  });
}

fn apply_navigation_update(
  this: &WryNavigationDelegate,
  webview: Option<&WKWebView>,
  update: AppleNavigationUpdate,
) -> bool {
  if update.stop_loading {
    if let Some(webview) = webview {
      unsafe { webview.stopLoading() };
    }
  }
  for event in update.events {
    emit_navigation_event(this, event);
  }
  if update.schedule_deferred {
    schedule_deferred_navigation_flush(this);
  }
  !update.stop_loading
}

fn schedule_deferred_navigation_flush(this: &WryNavigationDelegate) {
  let Some(mtm) = MainThreadMarker::new() else {
    return;
  };
  let delegate = MainThreadBound::new(this.retain(), mtm);
  DispatchQueue::main().exec_async(move || {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      let Some(mtm) = MainThreadMarker::new() else {
        return;
      };
      let delegate = delegate.get(mtm);
      let update = with_navigation_state(&delegate.ivars().navigation_event_state, |state| {
        state.flush_deferred()
      });
      apply_navigation_update(delegate, None, update);
    }));
  });
}

pub(crate) fn begin_programmatic_navigation(this: &WryNavigationDelegate) -> bool {
  if this.ivars().navigation_event_handler.is_none() {
    return true;
  }
  with_navigation_state(&this.ivars().navigation_event_state, |state| {
    state.begin_programmatic()
  })
}

pub(crate) fn cancel_programmatic_navigation(this: &WryNavigationDelegate) {
  if this.ivars().navigation_event_handler.is_none() {
    return;
  }
  let update = with_navigation_state(&this.ivars().navigation_event_state, |state| {
    state.cancel_programmatic()
  });
  apply_navigation_update(this, None, update);
}

pub(crate) fn register_programmatic_navigation(
  this: &WryNavigationDelegate,
  navigation: &WKNavigation,
  url: String,
) -> bool {
  if this.ivars().navigation_event_handler.is_none() {
    return true;
  }
  let update = with_navigation_state(&this.ivars().navigation_event_state, |state| {
    state.register_programmatic(navigation_key(navigation), url)
  });
  let accepted = update.registration_accepted;
  apply_navigation_update(this, None, update);
  accepted
}

pub(crate) fn did_start_provisional_navigation(
  this: &WryNavigationDelegate,
  webview: &WKWebView,
  navigation: &WKNavigation,
) {
  let update = with_navigation_state(&this.ivars().navigation_event_state, |state| {
    state.started(navigation_key(navigation))
  });
  apply_navigation_update(this, Some(webview), update);
}

pub(crate) fn did_receive_server_redirect(
  this: &WryNavigationDelegate,
  webview: &WKWebView,
  navigation: &WKNavigation,
) {
  let update = with_navigation_state(&this.ivars().navigation_event_state, |state| {
    state.redirected(navigation_key(navigation))
  });
  apply_navigation_update(this, Some(webview), update);
}

// WebKit policy interruption includes conversion to WKDownload. Keep the
// exact WKNavigation identity and restore prior presentation without inventing
// a committed URL or treating cancellation as a controller-construction error.
fn navigation_error_phase(domain: Option<&str>, code: isize) -> NavigationEventPhase {
  if matches!(
    (domain, code),
    (Some("WebKitErrorDomain"), 102) | (Some("NSURLErrorDomain"), -999)
  ) {
    NavigationEventPhase::Cancelled
  } else {
    NavigationEventPhase::Failed
  }
}

/// The category a person can be told; never the native error text.
fn navigation_failure(domain: Option<&str>, code: isize) -> NavigationFailure {
  if domain != Some("NSURLErrorDomain") {
    return NavigationFailure::Other;
  }
  match code {
    // Not connected, international roaming off, cellular data not allowed.
    -1009 | -1018 | -1020 => NavigationFailure::Offline,
    // Cannot find host, DNS lookup failed.
    -1003 | -1006 => NavigationFailure::HostNotFound,
    // Cannot connect to host, connection lost.
    -1004 | -1005 => NavigationFailure::Unreachable,
    -1001 => NavigationFailure::TimedOut,
    // Secure connection failed and the certificate errors that follow it.
    -1206..=-1200 => NavigationFailure::Insecure,
    _ => NavigationFailure::Other,
  }
}

pub(crate) fn did_fail_navigation(
  this: &WryNavigationDelegate,
  webview: &WKWebView,
  navigation: &WKNavigation,
  error: &NSError,
  stage: &'static str,
) {
  let domain = bounded_nsstring(
    &error.domain(),
    crate::native_bounds::NativeStringLimit {
      max_utf16_units: 128,
      max_utf8_bytes: 128,
    },
  );
  let phase = navigation_error_phase(domain.as_deref(), error.code());
  if phase == NavigationEventPhase::Failed {
    let domain_kind = match domain.as_deref() {
      Some("NSURLErrorDomain") => "NSURLErrorDomain",
      Some("WebKitErrorDomain") => "WebKitErrorDomain",
      _ => "other",
    };
    #[cfg(target_os = "macos")]
    if extension_tab_trace_enabled() {
      let failing_category = error
        .userInfo()
        .objectForKey(unsafe { NSURLErrorFailingURLErrorKey })
        .and_then(|value| {
          value
            .downcast_ref::<NSURL>()
            .and_then(|url| url.absoluteString())
        })
        .and_then(|value| bounded_nsstring(&value, PAGE_URL_LIMIT))
        .map_or("unavailable", |value| navigation_origin_category(&value));
      eprintln!(
        "extension-tab-trace: native-failure stage={stage} domain={domain_kind} code={} origin={failing_category}",
        error.code()
      );
    } else {
      eprintln!(
        "view-create: WebKit navigation failure stage={stage} domain={domain_kind} code={}",
        error.code()
      );
    }
    #[cfg(not(target_os = "macos"))]
    eprintln!(
      "view-create: WebKit navigation failure stage={stage} domain={domain_kind} code={}",
      error.code()
    );
  }
  // Reported before the update applies, so the reason is known whether the
  // Failed event is emitted now or from a deferred flush.
  if phase == NavigationEventPhase::Failed {
    if let Some(handler) = &this.ivars().navigation_failure_handler {
      handler(
        NavigationId::from_raw(navigation_key(navigation) as u64),
        navigation_failure(domain.as_deref(), error.code()),
      );
    }
  }
  let update = with_navigation_state(&this.ivars().navigation_event_state, |state| {
    state.terminal(navigation_key(navigation), phase)
  });
  apply_navigation_update(this, Some(webview), update);
}

pub(crate) fn did_commit_navigation(
  this: &WryNavigationDelegate,
  webview: &WKWebView,
  navigation: &WKNavigation,
) {
  if this.ivars().navigation_event_handler.is_some() {
    let committed_url = unsafe {
      webview
        .URL()
        .and_then(|url| url.absoluteString())
        .and_then(|url| bounded_nsstring(&url, PAGE_URL_LIMIT))
    };
    let update = with_navigation_state(&this.ivars().navigation_event_state, |state| {
      state.committed(navigation_key(navigation), committed_url)
    });
    if update.guard_presentation {
      // WKNavigationDelegate's didCommit boundary precedes presentation of
      // the newly committed main-frame document. Hide once per exact native
      // identity before calling the embedder; duplicate callbacks cannot
      // strand an already-acknowledged document behind a second hide.
      if let Some(guard) = &this.ivars().navigation_presentation_guard {
        // Revoke the exact reveal permit before setHidden can pump AppKit.
        guard();
        webview.setHidden(true);
      }
    }
    if !apply_navigation_update(this, Some(webview), update) {
      // Missing/oversized current URLs, duplicate commits, and exhausted
      // identity state cannot be attributed safely. Do not run page-load
      // hooks or inject scripts into a rejected document.
      return;
    }
  }
  unsafe {
    // Call on_load_handler
    if let Some(on_page_load) = &this.ivars().on_page_load_handler {
      on_page_load(PageLoadEvent::Started);
    }

    // Inject scripts
    let Ok(mut pending_scripts) = this.ivars().pending_scripts.lock() else {
      // A poisoned script queue is not a reason to abort the whole browser
      // from a page-driven navigation callback. Skip injection fail-closed.
      return;
    };
    if let Some(scripts) = &*pending_scripts {
      for script in scripts {
        webview.evaluateJavaScript_completionHandler(&NSString::from_str(script), None);
      }
      *pending_scripts = None;
    }
  }
}

pub(crate) fn did_finish_navigation(
  this: &WryNavigationDelegate,
  webview: &WKWebView,
  navigation: &WKNavigation,
) {
  let update = with_navigation_state(&this.ivars().navigation_event_state, |state| {
    state.terminal(navigation_key(navigation), NavigationEventPhase::Finished)
  });
  apply_navigation_update(this, Some(webview), update);
  if let Some(on_page_load) = &this.ivars().on_page_load_handler {
    on_page_load(PageLoadEvent::Finished);
  }
}

// Navigation handler
pub(crate) fn navigation_policy(
  this: &WryNavigationDelegate,
  webview: &WKWebView,
  action: &WKNavigationAction,
  handler: &block2::Block<dyn Fn(WKNavigationActionPolicy)>,
) {
  unsafe {
    // <https://developer.apple.com/documentation/webkit/wknavigationaction/shouldperformdownload>
    // Available: macOS 11.3+, iOS 14.5+
    let can_download = action.respondsToSelector(objc2::sel!(shouldPerformDownload));
    let should_download: bool = if can_download {
      action.shouldPerformDownload()
    } else {
      false
    };
    if should_download {
      let has_download_handler = this.ivars().has_download_handler;
      if has_download_handler {
        (*handler).call((WKNavigationActionPolicy::Download,));
      } else {
        (*handler).call((WKNavigationActionPolicy::Cancel,));
      }
    } else {
      let request = action.request();
      let Some(url) = request
        .URL()
        .and_then(|url| url.absoluteString())
        .and_then(|url| bounded_nsstring(&url, PAGE_URL_LIMIT))
      else {
        (*handler).call((WKNavigationActionPolicy::Cancel,));
        return;
      };
      #[cfg(target_os = "macos")]
      if action.navigationType() == objc2_web_kit::WKNavigationType::LinkActivated
        && action.targetFrame().is_some()
      {
        use objc2_app_kit::NSEventModifierFlags as Flags;
        let flags = action.modifierFlags();
        if flags.contains(Flags::Command) || action.buttonNumber() == 2 {
          if let Some(open) = &this.ivars().new_window_req_handler {
            let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
              open(
                url.clone(),
                crate::NewWindowFeatures {
                  user_initiated: true,
                  foreground: flags.contains(Flags::Shift),
                  size: None,
                  position: None,
                  opener: crate::NewWindowOpener {
                    webview: objc2::rc::Retained::retain(webview as *const _ as *mut _)
                      .expect("borrowed live webview"),
                    target_configuration: webview.configuration(),
                  },
                },
              )
            }))
            .unwrap_or(crate::NewWindowResponse::Deny);
            match response {
              crate::NewWindowResponse::Create { webview: created } => {
                // Preserve method, body, referrer and native cookie context.
                created.loadRequest(&request);
                (*handler).call((WKNavigationActionPolicy::Cancel,));
                return;
              }
              crate::NewWindowResponse::Deny => {
                (*handler).call((WKNavigationActionPolicy::Cancel,));
                return;
              }
              crate::NewWindowResponse::Allow => {}
            }
          }
        }
      }
      // The embedder's navigation callback owns browser-level top-frame URL
      // admission. It cannot authenticate a WebExtension child resource and
      // must not preempt WebKit's controller-bound URL scheme handler, which
      // verifies both the exact loaded context and `web_accessible_resources`.
      // Keep main-frame and target-less actions on the host policy path.
      let target_is_main_frame = action.targetFrame().map(|frame| frame.isMainFrame());
      if native_web_extension_subframe_owns_policy(&url, target_is_main_frame) {
        (*handler).call((WKNavigationActionPolicy::Allow,));
        return;
      }
      let function = &this.ivars().navigation_policy_function;
      let navigation_type = match action.navigationType() {
        WKNavigationType::LinkActivated => AppleNavigationType::LinkActivated,
        WKNavigationType::FormSubmitted => AppleNavigationType::FormSubmitted,
        WKNavigationType::BackForward => AppleNavigationType::BackForward,
        WKNavigationType::Reload => AppleNavigationType::Reload,
        WKNavigationType::FormResubmitted => AppleNavigationType::FormResubmitted,
        _ => AppleNavigationType::Other,
      };
      let is_get = request
        .HTTPMethod()
        .and_then(|method| bounded_nsstring(&method, CUSTOM_PROTOCOL_METHOD_LIMIT))
        .is_some_and(|method| method == "GET");
      let policy_allows = function(
        url.clone(),
        AppleNavigationAction {
          navigation_type,
          is_get,
          target_is_main_frame,
        },
      );
      #[cfg(target_os = "macos")]
      if extension_tab_trace_enabled() {
        let frame = match target_is_main_frame {
          Some(true) => "main",
          Some(false) => "subframe",
          None => "targetless",
        };
        eprintln!(
          "extension-tab-trace: native-policy origin={} frame={frame} admitted={policy_allows}",
          navigation_origin_category(&url)
        );
      }
      #[cfg(target_os = "macos")]
      if policy_allows && target_is_main_frame == Some(true) {
        if let Some(observe) = &this.ivars().main_frame_navigation_attempt_handler {
          observe(url.clone());
        }
      }
      match policy_allows {
        true => (*handler).call((WKNavigationActionPolicy::Allow,)),
        false => (*handler).call((WKNavigationActionPolicy::Cancel,)),
      };
    }
  }
}

// Navigation handler
pub(crate) fn navigation_policy_response(
  this: &WryNavigationDelegate,
  _webview: &WKWebView,
  response: &WKNavigationResponse,
  handler: &block2::Block<dyn Fn(WKNavigationResponsePolicy)>,
) {
  unsafe {
    let can_show_mime_type = response.canShowMIMEType();

    let native_response = response.response();
    let attachment = native_response
      .downcast_ref::<NSHTTPURLResponse>()
      .and_then(|response| {
        response.valueForHTTPHeaderField(&NSString::from_str("Content-Disposition"))
      })
      .and_then(|value| bounded_nsstring(&value, crate::native_bounds::DOWNLOAD_FILENAME_LIMIT))
      .is_some_and(|value| is_attachment_disposition(&value));
    if !can_show_mime_type || attachment {
      let has_download_handler = this.ivars().has_download_handler;
      if has_download_handler {
        (*handler).call((WKNavigationResponsePolicy::Download,));
      } else {
        (*handler).call((WKNavigationResponsePolicy::Cancel,));
      }
      return;
    }

    if response.isForMainFrame() {
      let response = response.response();
      let Some(_url) = response
        .URL()
        .and_then(|url| url.absoluteString())
        .and_then(|url| bounded_nsstring(&url, PAGE_URL_LIMIT))
      else {
        (*handler).call((WKNavigationResponsePolicy::Cancel,));
        return;
      };
    }

    (*handler).call((WKNavigationResponsePolicy::Allow,));
  }
}

pub(crate) fn web_content_process_did_terminate(
  this: &WryNavigationDelegate,
  _webview: &WKWebView,
) {
  with_navigation_state(&this.ivars().navigation_event_state, |state| {
    state.clear();
  });
  if let Some(on_web_content_process_terminate) =
    &this.ivars().on_web_content_process_terminate_handler
  {
    on_web_content_process_terminate();
  }
}

#[cfg(test)]
mod navigation_event_state_tests {
  use super::*;

  fn phases(update: &AppleNavigationUpdate) -> Vec<NavigationEventPhase> {
    update.events.iter().map(|event| event.phase).collect()
  }

  fn register(
    state: &mut AppleNavigationEventState,
    key: usize,
    url: &str,
  ) -> AppleNavigationUpdate {
    assert!(state.begin_programmatic());
    state.register_programmatic(key, url.into())
  }

  #[test]
  fn web_extension_child_resources_delegate_only_to_native_webkit_policy() {
    let extension = "webkit-extension://abcdefghijklmnopabcdefghijklmnop/inline/menu.html";
    assert!(native_web_extension_subframe_owns_policy(
      extension,
      Some(false)
    ));
    assert!(!native_web_extension_subframe_owns_policy(
      extension,
      Some(true)
    ));
    assert!(!native_web_extension_subframe_owns_policy(extension, None));
    assert!(!native_web_extension_subframe_owns_policy(
      "https://example.com/frame",
      Some(false)
    ));
    assert!(!native_web_extension_subframe_owns_policy(
      "webkit-extensionx://abcdefghijklmnopabcdefghijklmnop/frame",
      Some(false)
    ));
  }

  #[test]
  fn programmatic_navigation_keeps_one_exact_native_identity() {
    let mut state = AppleNavigationEventState::default();
    let registered = register(&mut state, 11, "https://origin.example/start");
    assert!(registered.registration_accepted);
    assert!(registered.events.is_empty());

    let started = state.started(11);
    assert_eq!(phases(&started), vec![NavigationEventPhase::Started]);
    assert_eq!(started.events[0].key, 11);
    assert_eq!(started.events[0].url, "https://origin.example/start");

    let redirected = state.redirected(11);
    assert_eq!(phases(&redirected), vec![NavigationEventPhase::Redirected]);
    assert_eq!(redirected.events[0].url, "https://origin.example/start");

    let committed = state.committed(11, Some("https://final.example/landing".into()));
    assert_eq!(phases(&committed), vec![NavigationEventPhase::Committed]);
    assert_eq!(committed.events[0].url, "https://final.example/landing");

    let finished = state.terminal(11, NavigationEventPhase::Finished);
    assert_eq!(phases(&finished), vec![NavigationEventPhase::Finished]);
    assert_eq!(finished.events[0].url, "https://final.example/landing");
    assert!(!state.active.contains_key(&11));
  }

  #[test]
  fn overlapping_programmatic_loads_cannot_swap_urls() {
    let mut state = AppleNavigationEventState::default();
    assert!(register(&mut state, 7, "https://first.example/").registration_accepted);
    assert!(register(&mut state, 8, "https://second.example/").registration_accepted);
    assert_eq!(state.started(8).events[0].url, "https://second.example/");
    assert_eq!(state.started(7).events[0].url, "https://first.example/");
    assert_eq!(
      state
        .committed(8, Some("https://second.example/final".into()))
        .events[0]
        .url,
      "https://second.example/final"
    );
    assert_eq!(
      state
        .committed(7, Some("https://first.example/final".into()))
        .events[0]
        .url,
      "https://first.example/final"
    );
  }

  #[test]
  fn reentrant_start_before_load_request_returns_is_bound_exactly() {
    let mut state = AppleNavigationEventState::default();
    assert!(state.begin_programmatic());
    let early_start = state.started(31);
    assert!(early_start.events.is_empty());
    assert!(!early_start.schedule_deferred);

    let registered = state.register_programmatic(31, "https://requested.example/".into());
    assert!(registered.registration_accepted);
    assert_eq!(phases(&registered), vec![NavigationEventPhase::Started]);
    assert_eq!(registered.events[0].key, 31);
    assert_eq!(registered.events[0].url, "https://requested.example/");
  }

  #[test]
  fn reentrant_commit_before_load_request_returns_preserves_requested_then_final_urls() {
    let mut state = AppleNavigationEventState::default();
    assert!(state.begin_programmatic());
    let early_commit = state.committed(41, Some("https://final.example/".into()));
    assert!(early_commit.events.is_empty());
    assert!(!early_commit.schedule_deferred);

    let registered = state.register_programmatic(41, "https://requested.example/".into());
    assert!(registered.registration_accepted);
    assert_eq!(
      phases(&registered),
      vec![
        NavigationEventPhase::Started,
        NavigationEventPhase::Committed
      ]
    );
    assert_eq!(registered.events[0].url, "https://requested.example/");
    assert_eq!(registered.events[1].url, "https://final.example/");
  }

  #[test]
  fn nested_reentrant_programmatic_loads_bind_by_key_not_stack_order() {
    let mut state = AppleNavigationEventState::default();
    assert!(state.begin_programmatic());
    assert!(state.started(51).events.is_empty());
    assert!(state.begin_programmatic());
    assert!(state
      .committed(52, Some("https://second.example/final".into()))
      .events
      .is_empty());

    let second = state.register_programmatic(52, "https://second.example/start".into());
    assert!(second.registration_accepted);
    assert_eq!(second.events[0].key, 52);
    assert_eq!(second.events[0].url, "https://second.example/start");
    assert_eq!(second.events[1].url, "https://second.example/final");

    let first = state.register_programmatic(51, "https://first.example/start".into());
    assert!(first.registration_accepted);
    assert_eq!(first.events[0].key, 51);
    assert_eq!(first.events[0].url, "https://first.example/start");
  }

  #[test]
  fn reentrant_multi_hop_redirects_are_replayed_in_order_after_binding() {
    let mut state = AppleNavigationEventState::default();
    assert!(state.begin_programmatic());
    state.started(53);
    state.redirected(53);
    state.redirected(53);
    state.committed(53, Some("https://final.example/".into()));

    let registered = state.register_programmatic(53, "https://requested.example/".into());
    assert_eq!(
      phases(&registered),
      vec![
        NavigationEventPhase::Started,
        NavigationEventPhase::Redirected,
        NavigationEventPhase::Redirected,
        NavigationEventPhase::Committed
      ]
    );
    assert_eq!(registered.events[0].url, "https://requested.example/");
    assert_eq!(registered.events[1].url, "https://requested.example/");
    assert_eq!(registered.events[2].url, "https://requested.example/");
    assert_eq!(registered.events[3].url, "https://final.example/");
  }

  #[test]
  fn registered_commit_before_start_synthesizes_a_valid_order() {
    let mut state = AppleNavigationEventState::default();
    assert!(register(&mut state, 43, "https://requested.example/").registration_accepted);
    let committed = state.committed(43, Some("https://final.example/".into()));
    assert_eq!(
      phases(&committed),
      vec![
        NavigationEventPhase::Started,
        NavigationEventPhase::Committed
      ]
    );
    assert_eq!(committed.events[0].url, "https://requested.example/");
    assert_eq!(committed.events[1].url, "https://final.example/");
  }

  #[test]
  fn page_driven_commit_is_attributed_in_the_did_commit_turn() {
    let mut state = AppleNavigationEventState::default();
    assert!(state.started(19).events.is_empty());
    let committed = state.committed(19, Some("https://page-driven.example/".into()));
    assert!(!committed.schedule_deferred);
    assert_eq!(
      phases(&committed),
      vec![
        NavigationEventPhase::Started,
        NavigationEventPhase::Committed
      ]
    );
    assert!(committed.events.iter().all(|event| event.key == 19));
    assert!(committed
      .events
      .iter()
      .all(|event| event.url == "https://page-driven.example/"));
    assert!(state.flush_deferred().events.is_empty());

    let finished = state.terminal(19, NavigationEventPhase::Finished);
    assert_eq!(phases(&finished), vec![NavigationEventPhase::Finished]);
  }

  #[test]
  fn cancelling_the_last_programmatic_bracket_flushes_page_commit_synchronously() {
    let mut state = AppleNavigationEventState::default();
    assert!(state.begin_programmatic());
    let page_commit = state.committed(61, Some("https://page-driven.example/".into()));
    assert!(page_commit.events.is_empty());
    assert!(!page_commit.schedule_deferred);

    let cancelled = state.cancel_programmatic();
    assert!(!cancelled.schedule_deferred);
    assert_eq!(
      phases(&cancelled),
      vec![
        NavigationEventPhase::Started,
        NavigationEventPhase::Committed
      ]
    );
    assert!(cancelled.events.iter().all(|event| event.key == 61));
    assert!(state.flush_deferred().events.is_empty());
  }

  #[test]
  fn registering_the_last_programmatic_key_flushes_other_page_commit_synchronously() {
    let mut state = AppleNavigationEventState::default();
    assert!(state.begin_programmatic());
    let page_commit = state.committed(63, Some("https://page-driven.example/".into()));
    assert!(page_commit.events.is_empty());
    assert!(!page_commit.schedule_deferred);

    let registered = state.register_programmatic(64, "https://requested.example/".into());
    assert!(registered.registration_accepted);
    assert!(!registered.schedule_deferred);
    assert_eq!(
      phases(&registered),
      vec![
        NavigationEventPhase::Started,
        NavigationEventPhase::Committed
      ]
    );
    assert!(registered.events.iter().all(|event| event.key == 63));
    assert!(state.flush_deferred().events.is_empty());
  }

  #[test]
  fn overlapping_page_driven_commits_keep_native_callback_order() {
    let mut state = AppleNavigationEventState::default();
    let first = state.committed(71, Some("https://first.example/".into()));
    assert!(!first.schedule_deferred);
    let second = state.committed(72, Some("https://second.example/".into()));
    assert!(!second.schedule_deferred);
    assert_eq!(
      first
        .events
        .iter()
        .map(|event| event.key)
        .collect::<Vec<_>>(),
      vec![71, 71]
    );
    assert_eq!(
      second
        .events
        .iter()
        .map(|event| event.key)
        .collect::<Vec<_>>(),
      vec![72, 72]
    );
    assert_eq!(first.events[0].url, "https://first.example/");
    assert_eq!(second.events[0].url, "https://second.example/");
    assert!(state.flush_deferred().events.is_empty());
  }

  #[test]
  fn missing_commit_url_fails_an_exact_programmatic_navigation_once() {
    let mut state = AppleNavigationEventState::default();
    assert!(register(&mut state, 23, "https://requested.example/").registration_accepted);
    let failed = state.committed(23, None);
    assert!(failed.stop_loading);
    assert_eq!(
      phases(&failed),
      vec![NavigationEventPhase::Started, NavigationEventPhase::Failed]
    );
    assert!(failed
      .events
      .iter()
      .all(|event| event.url == "https://requested.example/"));
    assert!(!state.active.contains_key(&23));
    assert!(state
      .terminal(23, NavigationEventPhase::Failed)
      .events
      .is_empty());
  }

  #[test]
  fn reentrant_missing_url_failure_waits_for_exact_programmatic_attribution() {
    let mut state = AppleNavigationEventState::default();
    assert!(state.begin_programmatic());
    let early_failure = state.committed(29, None);
    assert!(early_failure.stop_loading);
    assert!(early_failure.events.is_empty());
    assert!(!early_failure.schedule_deferred);

    let registered = state.register_programmatic(29, "https://requested.example/".into());
    assert!(registered.registration_accepted);
    assert_eq!(
      phases(&registered),
      vec![NavigationEventPhase::Started, NavigationEventPhase::Failed]
    );
    assert!(!state.active.contains_key(&29));
  }

  #[test]
  fn finish_without_commit_is_normalized_to_failure() {
    let mut state = AppleNavigationEventState::default();
    assert!(register(&mut state, 37, "https://requested.example/").registration_accepted);
    let terminal = state.terminal(37, NavigationEventPhase::Finished);
    assert_eq!(
      phases(&terminal),
      vec![NavigationEventPhase::Started, NavigationEventPhase::Failed]
    );
    assert!(!state.active.contains_key(&37));
  }

  #[test]
  fn duplicate_terminal_callback_cannot_complete_twice() {
    let mut state = AppleNavigationEventState::default();
    assert!(register(&mut state, 47, "https://requested.example/").registration_accepted);
    state.committed(47, Some("https://final.example/".into()));
    assert_eq!(
      phases(&state.terminal(47, NavigationEventPhase::Finished)),
      vec![NavigationEventPhase::Finished]
    );
    assert!(state
      .terminal(47, NavigationEventPhase::Finished)
      .events
      .is_empty());
  }

  #[test]
  fn active_navigation_identity_budget_is_fail_closed() {
    let mut state = AppleNavigationEventState::default();
    for key in 0..ACTIVE_APPLE_NAVIGATION_LIMIT {
      assert!(register(&mut state, key, &format!("https://{key}.example/")).registration_accepted);
    }
    assert!(state.begin_programmatic());
    let overflow = state.register_programmatic(
      ACTIVE_APPLE_NAVIGATION_LIMIT,
      "https://overflow.example/".into(),
    );
    assert!(!overflow.registration_accepted);
    assert!(state.started(ACTIVE_APPLE_NAVIGATION_LIMIT).stop_loading);

    state.terminal(0, NavigationEventPhase::Failed);
    assert!(
      register(
        &mut state,
        ACTIVE_APPLE_NAVIGATION_LIMIT,
        "https://reused.example/"
      )
      .registration_accepted
    );
  }

  #[test]
  fn page_driven_identity_uses_the_native_key_without_a_wrapping_counter() {
    let mut state = AppleNavigationEventState::default();
    let native_key = usize::MAX;
    let committed = state.committed(native_key, Some("https://page.example/".into()));
    assert!(!committed.schedule_deferred);
    assert!(committed.events.iter().all(|event| event.key == native_key));
    assert_eq!(
      NavigationId::from_raw(committed.events[0].key as u64),
      NavigationId::from_raw(native_key as u64)
    );
  }

  #[test]
  fn exact_native_commit_arms_presentation_once_even_if_delegate_repeats() {
    let mut state = AppleNavigationEventState::default();
    assert!(register(&mut state, 91, "https://requested.example/").registration_accepted);
    let first = state.committed(91, Some("https://final.example/".into()));
    assert!(first.guard_presentation);
    assert_eq!(
      phases(&first),
      vec![
        NavigationEventPhase::Started,
        NavigationEventPhase::Committed
      ]
    );

    let duplicate = state.committed(91, Some("https://final.example/".into()));
    assert!(!duplicate.guard_presentation);
    assert!(duplicate.stop_loading);
  }
}

fn is_attachment_disposition(value: &str) -> bool {
  value
    .split(';')
    .next()
    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("attachment"))
}

#[cfg(test)]
mod attachment_tests {
  use super::is_attachment_disposition;
  #[test]
  fn only_the_disposition_token_selects_downloads() {
    assert!(is_attachment_disposition("attachment; filename=report.txt"));
    assert!(is_attachment_disposition(
      " Attachment ; filename=report.txt"
    ));
    assert!(!is_attachment_disposition(
      "inline; filename=attachment.txt"
    ));
    assert!(!is_attachment_disposition("attachment-invalid"));
  }
}

#[cfg(test)]
mod cancellation_tests {
  use super::*;

  #[test]
  fn failures_reduce_to_categories_a_person_can_act_on() {
    let url = Some("NSURLErrorDomain");
    assert_eq!(navigation_failure(url, -1009), NavigationFailure::Offline);
    assert_eq!(
      navigation_failure(url, -1003),
      NavigationFailure::HostNotFound
    );
    assert_eq!(
      navigation_failure(url, -1004),
      NavigationFailure::Unreachable
    );
    assert_eq!(navigation_failure(url, -1001), NavigationFailure::TimedOut);
    assert_eq!(navigation_failure(url, -1202), NavigationFailure::Insecure);
    assert_eq!(navigation_failure(url, -1100), NavigationFailure::Other);
    assert_eq!(
      navigation_failure(Some("WebKitErrorDomain"), -1009),
      NavigationFailure::Other
    );
  }
  #[test]
  fn native_cancellation_is_domain_scoped_and_never_a_commit() {
    assert_eq!(
      navigation_error_phase(Some("WebKitErrorDomain"), 102),
      NavigationEventPhase::Cancelled
    );
    assert_eq!(
      navigation_error_phase(Some("NSURLErrorDomain"), -999),
      NavigationEventPhase::Cancelled
    );
    assert_eq!(
      navigation_error_phase(Some("NSURLErrorDomain"), 102),
      NavigationEventPhase::Failed
    );
    assert_eq!(
      navigation_error_phase(None, 102),
      NavigationEventPhase::Failed
    );
    let mut state = AppleNavigationEventState::default();
    assert!(state.begin_programmatic());
    state.register_programmatic(41, "https://fixture.test/download".into());
    let update = state.terminal(41, NavigationEventPhase::Cancelled);
    assert_eq!(
      update
        .events
        .iter()
        .map(|event| event.phase)
        .collect::<Vec<_>>(),
      vec![
        NavigationEventPhase::Started,
        NavigationEventPhase::Cancelled
      ]
    );
    assert!(state
      .terminal(41, NavigationEventPhase::Failed)
      .events
      .is_empty());
  }
}
