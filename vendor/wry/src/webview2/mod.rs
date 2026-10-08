// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

mod drag_drop;
mod util;

use std::{
  borrow::Cow,
  cell::{Cell, RefCell},
  collections::{HashMap, HashSet},
  fmt::Write,
  num::NonZeroU64,
  path::PathBuf,
  ptr::NonNull,
  rc::Rc,
  sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    mpsc, Mutex,
  },
};

use dpi::{PhysicalPosition, PhysicalSize};
use http::{Request, Response as HttpResponse, StatusCode};
use once_cell::sync::Lazy;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::{
  core::{s, w, Interface, BOOL, HRESULT, HSTRING, PCWSTR, PWSTR},
  Win32::{
    Foundation::*,
    Globalization::*,
    Graphics::Gdi::*,
    System::{Com::*, LibraryLoader::GetModuleHandleW},
    UI::{Input::KeyboardAndMouse::SetFocus, Shell::*, WindowsAndMessaging::*},
  },
};

use self::drag_drop::DragDropController;
use super::Theme;
use crate::{
  custom_protocol_workaround,
  native_admission::{
    InFlightAdmission, InFlightPermit, CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT,
    CUSTOM_PROTOCOL_OVERFLOW_STATUS,
  },
  native_bounds::{
    bounded_utf16, CustomProtocolRequestBudget, NativeStringLimit,
    CUSTOM_PROTOCOL_HEADER_NAME_LIMIT, CUSTOM_PROTOCOL_HEADER_VALUE_LIMIT,
    CUSTOM_PROTOCOL_METHOD_LIMIT, IPC_PAYLOAD_LIMIT, PAGE_TITLE_LIMIT, PAGE_URL_LIMIT,
  },
  native_cleanup::{CleanupPlan, CleanupStep},
  proxy::ProxyConfig,
  Error, MemoryUsageLevel, NavigationEvent, NavigationEventPhase, NavigationId, NewWindowFeatures,
  NewWindowOpener, NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, Rect,
  RequestAsyncResponder, Result, WebViewAttributes, RGBA,
};

type EventRegistrationToken = i64;

const PARENT_SUBCLASS_ID: u32 = WM_USER + 0x64;
const MAIN_THREAD_DISPATCHER_SUBCLASS_ID: u32 = WM_USER + 0x66;
static EXEC_MSG_ID: Lazy<u32> = Lazy::new(|| unsafe { RegisterWindowMessageA(s!("Wry::ExecMsg")) });
// The message carries a pointer-sized capability, but it is never trusted by
// itself. Only pointers installed in this registry by `dispatch_handler` may
// be reconstructed by the window procedure.
static PENDING_DISPATCHES: Lazy<Mutex<HashMap<usize, isize>>> =
  Lazy::new(|| Mutex::new(HashMap::new()));
// This registry owns heap allocations referenced by posted window messages.
// USER32's message queue is much larger than the amount of work Wry should
// retain, and hostile page events can otherwise make memory grow until that
// OS queue saturates. Rejection is fail-closed at every current call site:
// popup deferral guards deny in Drop, while response callbacks retain their
// own bounded native admission/timeout lifecycle.
const PENDING_DISPATCH_LIMIT: usize = 1024;
// WebView2 permits navigation event sequences with different IDs to overlap.
// Retain only a small bounded set of admitted main-frame identities so
// ContentLoading never has to derive an event URL from mutable global Source.
const IN_FLIGHT_NAVIGATION_LIMIT: usize = 64;
static NEXT_WEB_RESOURCE_RESPONSE: AtomicUsize = AtomicUsize::new(1);
// A custom-protocol handler is application code and may fail to answer, while
// posting its answer back to the UI queue may also fail. Never retain a
// WebView2 deferral indefinitely: the UI thread settles it with a fail-closed
// response after this bounded grace period. The timer is owned by the Wry
// container HWND, so its callback executes in the COM object's apartment.
const WEB_RESOURCE_RESPONSE_TIMEOUT_MS: u32 = 30_000;
const WEBVIEW2_VERSION_LIMIT: NativeStringLimit = NativeStringLimit {
  max_utf16_units: 256,
  max_utf8_bytes: 256,
};
const _: () = assert!(WEB_RESOURCE_RESPONSE_TIMEOUT_MS >= USER_TIMER_MINIMUM);
const _: () = assert!(WEB_RESOURCE_RESPONSE_TIMEOUT_MS <= 60_000);

fn take_pwstr_bounded(source: PWSTR, limit: NativeStringLimit) -> Option<String> {
  // WebView2 transfers a CoTaskMemAlloc-owned pointer. Keep it in an RAII guard
  // so every early limit rejection still frees the native allocation exactly
  // once without first duplicating it into an unbounded Rust String.
  let source = CoTaskMemPWSTR::from(source);
  let pointer = source.as_ref().as_pcwstr().as_ptr();
  if pointer.is_null() {
    return Some(String::new());
  }

  let mut length = 0;
  while length <= limit.max_utf16_units {
    // SAFETY: WebView2's out-string contract supplies a NUL-terminated buffer;
    // this bounded scan reads at most the configured limit plus one unit.
    if unsafe { pointer.add(length).read() } == 0 {
      // SAFETY: the scan above established exactly `length` initialized units
      // before the terminator and the CoTaskMem guard is still alive.
      let units = unsafe { std::slice::from_raw_parts(pointer, length) };
      return bounded_utf16(units, limit);
    }
    length += 1;
  }
  None
}

fn bounded_stream_read_len(cb_read: u32, buffer_len: usize) -> Option<usize> {
  let cb_read = usize::try_from(cb_read).ok()?;
  (cb_read <= buffer_len).then_some(cb_read)
}

thread_local! {
  // WebView2's projected COM interfaces are apartment-bound and deliberately
  // do not implement Send. Keep their ownership on the creating UI thread;
  // asynchronous protocol workers receive only a numeric lookup token.
  static PENDING_WEB_RESOURCE_RESPONSES:
    RefCell<HashMap<usize, PendingWebResourceResponse>> = RefCell::new(HashMap::new());
  // Generic Drop cannot report a native close failure to its caller. Retain
  // such debts on the creating apartment until the embedder drains them.
  // Overflow is an invariant failure: intentionally leak the owned native
  // references until process exit and expose a sticky fail-closed marker;
  // never forget an obligation or abort from a destructor.
  static ORPHANED_CLEANUP_DEBTS: RefCell<Vec<(NonZeroU64, NativeCleanupDebt)>> = const { RefCell::new(Vec::new()) };
  static ORPHANED_CLEANUP_OVERFLOWED: Cell<bool> = const { Cell::new(false) };
}

const MAX_ORPHANED_CLEANUP_DEBTS: usize = 4_096;
static NEXT_CLEANUP_INCIDENT_ID: AtomicU64 = AtomicU64::new(1);

impl From<webview2_com::Error> for Error {
  fn from(err: webview2_com::Error) -> Self {
    Error::WebView2Error(err)
  }
}

impl From<windows::core::Error> for Error {
  fn from(err: windows::core::Error) -> Self {
    Error::WebView2Error(webview2_com::Error::WindowsError(err))
  }
}

/// One explicit WebView2 teardown contract that has not yet been proven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WebView2CleanupStep {
  /// Removing Wry's parent-window subclass failed.
  ParentSubclass,
  /// `ICoreWebView2Controller::Close` failed.
  Controller,
  /// Destroying Wry's child container HWND failed.
  ContainerWindow,
}

/// The first native teardown failure observed during one retry cohort.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebView2CleanupFailure {
  pub step: WebView2CleanupStep,
  /// HRESULT-compatible error code. Win32 failures are converted through
  /// `HRESULT_FROM_WIN32` so callers can record one stable representation.
  pub code: i32,
}

/// Send-safe description of apartment-bound cleanup retained after a failed
/// WebView2 construction. The native COM/HWND ownership never leaves the UI
/// thread; callers use this incident only for diagnostics and fail-closed
/// resource accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebView2ConstructionCleanupIncident {
  /// Process-unique correlation id when Wry retained the debt in its bounded
  /// apartment registry. `None` means registry admission failed and Wry set
  /// the sticky cleanup-overflow marker while intentionally leaking native
  /// ownership until process exit.
  pub id: Option<NonZeroU64>,
  pub first_failure: WebView2CleanupFailure,
  pub may_own_controller: bool,
}

/// Main-thread-owned, retryable WebView2 teardown obligations.
pub struct WebView2CleanupDebt {
  inner: Option<NativeCleanupDebt>,
}

impl std::fmt::Debug for WebView2CleanupDebt {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter
      .debug_struct("WebView2CleanupDebt")
      .field("complete", &self.is_complete())
      .field("last_failure", &self.last_failure())
      .finish_non_exhaustive()
  }
}

impl WebView2CleanupDebt {
  fn new(inner: NativeCleanupDebt) -> Self {
    Self { inner: Some(inner) }
  }

  /// Retries every outstanding native release once. Successful steps are not
  /// repeated; independent later steps are still attempted after a failure.
  pub fn retry(&mut self) -> std::result::Result<(), WebView2CleanupFailure> {
    let Some(inner) = self.inner.as_mut() else {
      return Ok(());
    };
    match inner.retry() {
      Ok(()) => {
        self.inner = None;
        Ok(())
      }
      Err(failure) => Err(failure),
    }
  }

  pub fn is_complete(&self) -> bool {
    self.inner.is_none()
  }

  pub fn last_failure(&self) -> Option<WebView2CleanupFailure> {
    self.inner.as_ref().and_then(|inner| inner.last_failure)
  }

  /// Whether this debt still retains a controller whose explicit `Close`
  /// contract has not succeeded. Embedders use this for native resource
  /// admission; HWND/subclass-only debts remain teardown blockers but do not
  /// consume another controller slot.
  pub fn may_own_controller(&self) -> bool {
    self
      .inner
      .as_ref()
      .is_some_and(NativeCleanupDebt::may_own_controller)
  }
}

impl Drop for WebView2CleanupDebt {
  fn drop(&mut self) {
    let Some(mut debt) = self.inner.take() else {
      return;
    };
    let _ = debt.retry();
    if !debt.is_complete() {
      let _ = retain_orphaned_cleanup_debt(debt);
    }
  }
}

/// Drains fallback debts produced by ordinary `Drop`. The returned values
/// remain main-thread-bound and must be retried or retained by the embedder.
pub fn pending_webview2_cleanup_debts() -> Vec<WebView2CleanupDebt> {
  ORPHANED_CLEANUP_DEBTS.with(|pending| {
    let Ok(mut pending) = pending.try_borrow_mut() else {
      // Preserve the already-owned debts in place. The sticky marker tells the
      // embedder that it could not obtain an authoritative drain snapshot.
      ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
      return Vec::new();
    };
    std::mem::take(&mut *pending)
      .into_iter()
      .map(|(_, debt)| WebView2CleanupDebt::new(debt))
      .collect()
  })
}

/// Returns whether Wry had to retain an unreportable cleanup obligation after
/// its bounded fallback queue was unavailable or full. The native references
/// are intentionally leaked until process exit; embedders must fail closed.
pub fn webview2_cleanup_overflowed() -> bool {
  ORPHANED_CLEANUP_OVERFLOWED.with(Cell::get)
}

fn next_cleanup_incident_id() -> Option<NonZeroU64> {
  NEXT_CLEANUP_INCIDENT_ID
    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
      current.checked_add(1)
    })
    .ok()
    .and_then(NonZeroU64::new)
}

fn cleanup_registry_has_capacity(current: usize) -> bool {
  current < MAX_ORPHANED_CLEANUP_DEBTS
}

fn dispatch_registry_has_capacity(current: usize) -> bool {
  current < PENDING_DISPATCH_LIMIT
}

// A native stopped/cancelled navigation is not a controller-construction
// failure. Keep its uncommitted view reusable (including download handoff);
// no document is committed or granted presentation by this classification.
fn navigation_completion_phase(
  succeeded: bool,
  status: COREWEBVIEW2_WEB_ERROR_STATUS,
) -> NavigationEventPhase {
  if succeeded {
    NavigationEventPhase::Finished
  } else if matches!(
    status,
    COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED
      | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED
  ) {
    NavigationEventPhase::Cancelled
  } else {
    NavigationEventPhase::Failed
  }
}

/// The category a person can act on for a failed navigation, or None when it
/// was cancelled (a stop, a download conversion) rather than failed.
fn web_error_failure(status: COREWEBVIEW2_WEB_ERROR_STATUS) -> Option<crate::NavigationFailure> {
  use crate::NavigationFailure as Failure;
  Some(match status {
    COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED
    | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED => return None,
    COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED => Failure::Offline,
    COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED => Failure::HostNotFound,
    COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE
    | COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT
    | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET => Failure::Unreachable,
    COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT => Failure::TimedOut,
    COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
    | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
    | COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS
    | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED
    | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID => Failure::Insecure,
    _ => Failure::Other,
  })
}

#[derive(Default)]
struct InFlightNavigationUrls {
  urls: HashMap<u64, String>,
  committed: HashSet<u64>,
}

impl InFlightNavigationUrls {
  fn admit(&mut self, id: u64, url: String) -> bool {
    if !self.urls.contains_key(&id) && self.urls.len() >= IN_FLIGHT_NAVIGATION_LIMIT {
      return false;
    }
    self.urls.insert(id, url);
    true
  }

  fn commit(&mut self, id: u64) -> Option<String> {
    if !self.urls.contains_key(&id) || !self.committed.insert(id) {
      return None;
    }
    self.urls.get(&id).cloned()
  }

  fn finish(&mut self, id: u64) -> Option<String> {
    self.committed.remove(&id);
    self.urls.remove(&id)
  }
}

fn retain_orphaned_cleanup_debt(debt: NativeCleanupDebt) -> Option<NonZeroU64> {
  let Some(incident_id) = next_cleanup_incident_id() else {
    ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
    std::mem::forget(debt);
    return None;
  };
  ORPHANED_CLEANUP_DEBTS.with(|pending| {
    let Ok(mut pending) = pending.try_borrow_mut() else {
      ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
      std::mem::forget(debt);
      return None;
    };
    if !cleanup_registry_has_capacity(pending.len()) {
      ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
      std::mem::forget(debt);
      return None;
    }
    pending.push((incident_id, debt));
    Some(incident_id)
  })
}

struct ParentSubclassState {
  controller: ICoreWebView2Controller,
  detached: Cell<bool>,
}

struct ContainerWindowRegistration {
  hwnd: HWND,
  // A non-zero-sized allocation provides an address unique among live Wry
  // containers. USER32 owns only the numeric property value; this registration
  // remains the sole Rust owner of the allocation.
  identity: Box<u8>,
}

impl ContainerWindowRegistration {
  fn install(hwnd: HWND) -> windows::core::Result<Self> {
    let registration = Self {
      hwnd,
      identity: Box::new(0),
    };
    let identity = HANDLE((&*registration.identity as *const u8).cast_mut().cast());
    unsafe { SetPropW(hwnd, w!("Wry::ContainerIdentity"), Some(identity))? };
    Ok(registration)
  }

  fn is_exact_window(&self) -> bool {
    if !unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
      return false;
    }
    let expected = (&*self.identity as *const u8).cast_mut().cast();
    unsafe { GetPropW(self.hwnd, w!("Wry::ContainerIdentity")) }.0 == expected
  }
}

struct ParentSubclassRegistration {
  parent: HWND,
  // The installed callback receives only an opaque pointer. It takes a
  // temporary strong reference on entry so a reentrant close/remove cannot
  // free callback state while USER32 is still executing that callback.
  state: Rc<ParentSubclassState>,
}

impl ParentSubclassRegistration {
  fn retry_detach(&mut self) -> std::result::Result<(), i32> {
    if self.state.detached.get() || !unsafe { IsWindow(Some(self.parent)) }.as_bool() {
      self.state.detached.set(true);
      return Ok(());
    }
    let mut installed_state = 0_usize;
    let installed = unsafe {
      GetWindowSubclass(
        self.parent,
        Some(InnerWebView::parent_subclass_proc),
        PARENT_SUBCLASS_ID as _,
        Some(&mut installed_state),
      )
    }
    .as_bool();
    let expected_state = Rc::as_ptr(&self.state) as usize;
    if !installed || installed_state != expected_state {
      // The exact (HWND, callback, id, refdata) capability is absent. This
      // covers normal WM_NCDESTROY removal and protects against HWND reuse:
      // never remove a newer Wry registration that happens to reuse the same
      // numeric window handle and subclass id.
      self.state.detached.set(true);
      return Ok(());
    }
    if unsafe {
      RemoveWindowSubclass(
        self.parent,
        Some(InnerWebView::parent_subclass_proc),
        PARENT_SUBCLASS_ID as _,
      )
    }
    .as_bool()
    {
      self.state.detached.set(true);
      Ok(())
    } else {
      Err(windows::core::Error::from_win32().code().0)
    }
  }
}

struct NativeCleanupDebt {
  plan: CleanupPlan,
  controller: Option<ICoreWebView2Controller>,
  container_window: Option<ContainerWindowRegistration>,
  parent_subclasses: Vec<ParentSubclassRegistration>,
  last_failure: Option<WebView2CleanupFailure>,
}

#[cfg(feature = "windows-cleanup-qualification")]
thread_local! {
  static FAIL_CLOSE_ONCE: RefCell<Option<ICoreWebView2Controller>> = const { RefCell::new(None) };
}

/// Injects one controller-Close failure for this exact owned controller on this
/// apartment. Only explicit native qualification builds contain this hook.
#[cfg(feature = "windows-cleanup-qualification")]
pub fn fail_next_webview2_controller_close_for_qualification(controller: &ICoreWebView2Controller) {
  FAIL_CLOSE_ONCE.with(|slot| *slot.borrow_mut() = Some(controller.clone()));
}

impl NativeCleanupDebt {
  fn new(hwnd: HWND) -> windows::core::Result<Self> {
    Ok(Self {
      plan: CleanupPlan::new(false, true),
      controller: None,
      container_window: Some(ContainerWindowRegistration::install(hwnd)?),
      parent_subclasses: Vec::new(),
      last_failure: None,
    })
  }

  fn retain_controller(&mut self, controller: &ICoreWebView2Controller) {
    self.controller = Some(controller.clone());
    self.plan.set_controller(true);
  }

  fn retain_parent_subclass(&mut self, registration: ParentSubclassRegistration) {
    self.parent_subclasses.push(registration);
    self.plan.set_parent_subclass(true);
  }

  fn detach_parent_subclass(&mut self, parent: HWND) -> std::result::Result<(), i32> {
    let mut first_failure = None;
    for registration in self
      .parent_subclasses
      .iter_mut()
      .filter(|registration| registration.parent == parent)
    {
      if let Err(code) = registration.retry_detach() {
        first_failure.get_or_insert(code);
      }
    }
    self
      .parent_subclasses
      .retain(|registration| !registration.state.detached.get());
    self
      .plan
      .set_parent_subclass(!self.parent_subclasses.is_empty());
    first_failure.map_or(Ok(()), Err)
  }

  fn retry(&mut self) -> std::result::Result<(), WebView2CleanupFailure> {
    let mut plan = std::mem::take(&mut self.plan);
    let mut failures = HashMap::new();
    let first_step = plan.retry(|step| match step {
      CleanupStep::ParentSubclass => {
        let mut all_detached = true;
        for registration in &mut self.parent_subclasses {
          if let Err(code) = registration.retry_detach() {
            all_detached = false;
            failures
              .entry(WebView2CleanupStep::ParentSubclass)
              .or_insert(code);
          }
        }
        if all_detached {
          self.parent_subclasses.clear();
        }
        all_detached
      }
      CleanupStep::Controller => {
        let result = self
          .controller
          .as_ref()
          .map(|controller| {
            #[cfg(feature = "windows-cleanup-qualification")]
            if FAIL_CLOSE_ONCE.with(|slot| {
              let mut slot = slot.borrow_mut();
              if slot.as_ref().is_some_and(|target| target == controller) {
                slot.take();
                true
              } else {
                false
              }
            }) {
              return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_FAIL,
              ));
            }
            unsafe { controller.Close() }
          })
          .unwrap_or(Ok(()));
        match result {
          Ok(()) => {
            self.controller = None;
            true
          }
          Err(error) => {
            failures
              .entry(WebView2CleanupStep::Controller)
              .or_insert(error.code().0);
            false
          }
        }
      }
      CleanupStep::ContainerWindow => {
        let result = self.container_window.as_ref().map_or(Ok(()), |window| {
          if !window.is_exact_window() {
            Ok(())
          } else {
            unsafe { DestroyWindow(window.hwnd) }.map_err(|error| error.code().0)
          }
        });
        match result {
          Ok(()) => {
            self.container_window = None;
            true
          }
          Err(code) => {
            failures
              .entry(WebView2CleanupStep::ContainerWindow)
              .or_insert(code);
            false
          }
        }
      }
    });
    self.plan = plan;
    if self.plan.is_complete() {
      self.last_failure = None;
      return Ok(());
    }
    let step = match first_step.unwrap_or(CleanupStep::Controller) {
      CleanupStep::ParentSubclass => WebView2CleanupStep::ParentSubclass,
      CleanupStep::Controller => WebView2CleanupStep::Controller,
      CleanupStep::ContainerWindow => WebView2CleanupStep::ContainerWindow,
    };
    let failure = WebView2CleanupFailure {
      step,
      code: failures.get(&step).copied().unwrap_or(E_FAIL.0),
    };
    self.last_failure = Some(failure);
    Err(failure)
  }

  fn is_complete(&self) -> bool {
    self.plan.is_complete()
  }

  fn may_own_controller(&self) -> bool {
    self.plan.controller_pending()
  }
}

pub(crate) struct InnerWebView {
  id: String,
  parent: RefCell<HWND>,
  hwnd: HWND,
  is_child: bool,
  pub controller: ICoreWebView2Controller,
  pub webview: ICoreWebView2,
  pub env: ICoreWebView2Environment,
  custom_protocol_admission: InFlightAdmission,
  cleanup: RefCell<Option<NativeCleanupDebt>>,
  // Store FileDropController in here to make sure it gets dropped when
  // the webview gets dropped, otherwise we'll have a memory leak
  #[allow(dead_code)]
  drag_drop_controller: Option<DragDropController>,
}

/// Owns native resources during the fallible part of WebView2 construction.
/// In particular, releasing the last COM reference is not a substitute for
/// the controller's explicit `Close` contract. Without this guard, any error
/// between controller creation and `InnerWebView` construction leaves the
/// embedder unable to prove when the UDF/process-group obligation is released.
struct ControllerConstructionGuard {
  cleanup: Option<NativeCleanupDebt>,
}

impl ControllerConstructionGuard {
  fn new(hwnd: HWND) -> Result<Self> {
    match NativeCleanupDebt::new(hwnd) {
      Ok(cleanup) => Ok(Self {
        cleanup: Some(cleanup),
      }),
      Err(error) => {
        // No callback-capable work or message-loop pump occurred between HWND
        // creation and this identity installation attempt. Destroy once now;
        // if even that fails, preserve a sticky fail-closed marker and leave
        // the OS-owned window for process teardown rather than retrying an
        // HWND whose identity cannot be proven later.
        if unsafe { DestroyWindow(hwnd) }.is_err() {
          ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
        }
        Err(error.into())
      }
    }
  }

  fn retain_controller(&mut self, controller: &ICoreWebView2Controller) {
    if let Some(cleanup) = self.cleanup.as_mut() {
      cleanup.retain_controller(controller);
    } else {
      ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
      std::mem::forget(controller.clone());
    }
  }

  fn retain_parent_subclass(&mut self, registration: ParentSubclassRegistration) {
    if let Some(cleanup) = self.cleanup.as_mut() {
      cleanup.retain_parent_subclass(registration);
    } else {
      // Construction cannot safely lose a live subclass registration. Keep
      // its native state until exit and expose the global fail-closed marker.
      ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
      std::mem::forget(registration);
    }
  }

  fn into_cleanup(mut self) -> Option<NativeCleanupDebt> {
    self.cleanup.take()
  }

  fn finish_failure(mut self, source: Error) -> Error {
    let Some(native_cleanup) = self.cleanup.take() else {
      ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
      return source;
    };
    let mut cleanup = WebView2CleanupDebt::new(native_cleanup);
    match cleanup.retry() {
      Ok(()) => source,
      Err(first_failure) => {
        let may_own_controller = cleanup.may_own_controller();
        let id = if let Some(native_cleanup) = cleanup.inner.take() {
          retain_orphaned_cleanup_debt(native_cleanup)
        } else {
          // A failed retry without retained native ownership is internally
          // inconsistent. Do not abort a panic=abort embedder; make the
          // cleanup state globally untrustworthy and return a send-safe
          // incident that forces fail-closed handling.
          ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
          None
        };
        Error::WebView2ConstructionCleanup {
          source: Box::new(source),
          incident: WebView2ConstructionCleanupIncident {
            id,
            first_failure,
            may_own_controller,
          },
        }
      }
    }
  }
}

impl Drop for ControllerConstructionGuard {
  fn drop(&mut self) {
    if let Some(mut cleanup) = self.cleanup.take() {
      let _ = cleanup.retry();
      if !cleanup.is_complete() {
        let _ = retain_orphaned_cleanup_debt(cleanup);
      }
    }
  }
}

struct NewWindowDeferralGuard {
  args: ICoreWebView2NewWindowRequestedEventArgs,
  deferral: ICoreWebView2Deferral,
  completed: bool,
}

struct PendingWebResourceResponse {
  args: ICoreWebView2WebResourceRequestedEventArgs,
  env: ICoreWebView2Environment,
  deferral: ICoreWebView2Deferral,
  hwnd: isize,
  completed: bool,
  _permit: InFlightPermit,
}

impl PendingWebResourceResponse {
  fn finish(mut self, sent_response: HttpResponse<Cow<'static, [u8]>>) {
    unsafe {
      let response = InnerWebView::prepare_web_request_response(&self.env, &sent_response)
        .or_else(|error| InnerWebView::prepare_web_request_err(&self.env, error));
      if let Ok(response) = response {
        let _ = self.args.SetResponse(&response);
      }
      // Mark first: an HRESULT failure must not make Drop complete the same
      // native deferral a second time.
      self.completed = true;
      let _ = self.deferral.Complete();
    }
  }
}

impl Drop for PendingWebResourceResponse {
  fn drop(&mut self) {
    if self.completed {
      return;
    }
    unsafe {
      // A responder can disappear during shutdown or callback cancellation.
      // Install a bounded synthetic response before completing; completing an
      // intercepted request without a response may resume the rewritten HTTP
      // request instead of failing it closed.
      let response = web_resource_failure_response(StatusCode::INTERNAL_SERVER_ERROR);
      let response = InnerWebView::prepare_web_request_response(&self.env, &response)
        .or_else(|error| InnerWebView::prepare_web_request_err(&self.env, error));
      if let Ok(response) = response {
        let _ = self.args.SetResponse(&response);
      }
      self.completed = true;
      let _ = self.deferral.Complete();
    }
  }
}

fn web_resource_failure_response(status: StatusCode) -> HttpResponse<Cow<'static, [u8]>> {
  let mut response = HttpResponse::new(Cow::Borrowed(&[] as &[u8]));
  *response.status_mut() = status;
  response
}

fn pending_response_hwnd(hwnd: isize) -> HWND {
  HWND(hwnd as *mut core::ffi::c_void)
}

fn insert_pending_web_resource_response(pending: PendingWebResourceResponse) -> Option<usize> {
  let hwnd = pending_response_hwnd(pending.hwnd);
  let token = PENDING_WEB_RESOURCE_RESPONSES.with(|responses| {
    let mut responses = responses.try_borrow_mut().ok()?;
    loop {
      let token = NEXT_WEB_RESOURCE_RESPONSE.fetch_add(1, Ordering::Relaxed);
      if token != 0 && !responses.contains_key(&token) {
        responses.insert(token, pending);
        return Some(token);
      }
    }
  })?;

  let timer = unsafe {
    SetTimer(
      Some(hwnd),
      token,
      WEB_RESOURCE_RESPONSE_TIMEOUT_MS,
      Some(web_resource_response_timeout),
    )
  };
  if timer != 0 {
    Some(token)
  } else {
    // Timer admission is part of accepting ownership of the native deferral.
    // If it cannot be armed, remove the entry on this UI thread; Drop installs
    // the fail-closed response and completes exactly once.
    let pending = take_pending_web_resource_response(token, None)
      .ok()
      .flatten();
    drop(pending);
    None
  }
}

fn take_pending_web_resource_response(
  token: usize,
  expected_hwnd: Option<isize>,
) -> std::result::Result<Option<PendingWebResourceResponse>, ()> {
  PENDING_WEB_RESOURCE_RESPONSES.with(|responses| {
    let mut responses = responses.try_borrow_mut().map_err(|_| ())?;
    if let Some(hwnd) = expected_hwnd {
      match responses.get(&token) {
        Some(pending) if pending.hwnd == hwnd => {}
        _ => return Ok(None),
      }
    }
    Ok(responses.remove(&token))
  })
}

fn finish_pending_web_resource_response(token: usize, response: HttpResponse<Cow<'static, [u8]>>) {
  let pending = take_pending_web_resource_response(token, None)
    .ok()
    .flatten();
  if let Some(pending) = pending {
    let _ = unsafe { KillTimer(Some(pending_response_hwnd(pending.hwnd)), token) };
    pending.finish(response);
  }
}

fn cancel_pending_web_resource_responses(hwnd: isize) {
  let pending = PENDING_WEB_RESOURCE_RESPONSES.with(|responses| {
    if let Ok(mut responses) = responses.try_borrow_mut() {
      let tokens: Vec<_> = responses
        .iter()
        .filter_map(|(token, pending)| (pending.hwnd == hwnd).then_some(*token))
        .collect();
      return tokens
        .into_iter()
        .filter_map(|token| responses.remove(&token).map(|pending| (token, pending)))
        .collect();
    }
    Vec::new()
  });
  for (token, pending) in pending {
    let _ = unsafe { KillTimer(Some(pending_response_hwnd(hwnd)), token) };
    drop(pending);
  }
}

unsafe extern "system" fn web_resource_response_timeout(
  hwnd: HWND,
  _message: u32,
  token: usize,
  _time: u32,
) {
  // Never unwind through USER32's callback ABI. If the TLS map is temporarily
  // borrowed by a reentrant callback, leave the repeating timer armed so the
  // next tick can settle it on the same UI thread.
  let _ = std::panic::catch_unwind(|| {
    match take_pending_web_resource_response(token, Some(hwnd.0 as isize)) {
      Ok(Some(pending)) => {
        let _ = unsafe { KillTimer(Some(hwnd), token) };
        pending.finish(web_resource_failure_response(StatusCode::GATEWAY_TIMEOUT));
      }
      Ok(None) => {
        // A late timer notification after normal completion must not leave a
        // repeating USER32 timer installed.
        let _ = unsafe { KillTimer(Some(hwnd), token) };
      }
      Err(()) => {}
    }
  });
}

impl NewWindowDeferralGuard {
  fn new(args: ICoreWebView2NewWindowRequestedEventArgs, deferral: ICoreWebView2Deferral) -> Self {
    Self {
      args,
      deferral,
      completed: false,
    }
  }

  fn finish(mut self, response: NewWindowResponse) {
    unsafe {
      match response {
        NewWindowResponse::Allow => {
          let _ = self.args.SetHandled(false);
        }
        NewWindowResponse::CreateGuarded { webview, attached } => {
          let success = self.args.SetNewWindow(&webview).is_ok();
          // The engine closes the child if post-attachment policy cannot be
          // established. Network navigation stays deferred until this returns.
          attached(success);
          let _ = self.args.SetHandled(true);
        }
        NewWindowResponse::Create { webview } => {
          let _ = self.args.SetNewWindow(&webview);
          let _ = self.args.SetHandled(true);
        }
        NewWindowResponse::Deny => {
          let _ = self.args.SetHandled(true);
        }
      }
      // Mark before invoking COM so an HRESULT failure cannot cause a second
      // native completion attempt from Drop.
      self.completed = true;
      let _ = self.deferral.Complete();
    }
  }
}

impl Drop for NewWindowDeferralGuard {
  fn drop(&mut self) {
    if self.completed {
      return;
    }
    unsafe {
      let _ = self.args.SetHandled(true);
      self.completed = true;
      let _ = self.deferral.Complete();
    }
  }
}

impl Drop for InnerWebView {
  fn drop(&mut self) {
    cancel_pending_web_resource_responses(self.hwnd.0 as isize);
    self.custom_protocol_admission.seal_and_drain();
    if let Some(mut cleanup) = self.cleanup.get_mut().take() {
      let _ = cleanup.retry();
      if !cleanup.is_complete() {
        let _ = retain_orphaned_cleanup_debt(cleanup);
      }
    }
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WebView2ExtensionStartupRefusal {
  ExtensionPath,
  StartupFence,
}

const fn extension_startup_refusal(
  extension_path_configured: bool,
  browser_extensions_enabled: bool,
  startup_gate_configured: bool,
) -> Option<WebView2ExtensionStartupRefusal> {
  if extension_path_configured {
    Some(WebView2ExtensionStartupRefusal::ExtensionPath)
  } else if browser_extensions_enabled && !startup_gate_configured {
    Some(WebView2ExtensionStartupRefusal::StartupFence)
  } else {
    None
  }
}

impl InnerWebView {
  #[inline]
  pub fn new(
    window: &impl HasWindowHandle,
    attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
  ) -> Result<Self> {
    let window = match window.window_handle()?.as_raw() {
      RawWindowHandle::Win32(window) => HWND(window.hwnd.get() as _),
      _ => return Err(Error::UnsupportedWindowHandle),
    };
    Self::new_in_hwnd(window, attributes, pl_attrs, false)
  }

  #[inline]
  pub fn new_as_child(
    parent: &impl HasWindowHandle,
    attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
  ) -> Result<Self> {
    let parent = match parent.window_handle()?.as_raw() {
      RawWindowHandle::Win32(parent) => HWND(parent.hwnd.get() as _),
      _ => return Err(Error::UnsupportedWindowHandle),
    };

    Self::new_in_hwnd(parent, attributes, pl_attrs, true)
  }

  #[inline]
  fn new_in_hwnd(
    parent: HWND,
    mut attributes: WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
    is_child: bool,
  ) -> Result<Self> {
    if let Some(refusal) = extension_startup_refusal(
      pl_attrs.extension_path.is_some(),
      pl_attrs.browser_extensions_enabled,
      pl_attrs.browser_extension_startup_gate.is_some(),
    ) {
      return Err(match refusal {
        WebView2ExtensionStartupRefusal::ExtensionPath => Error::WebView2ExtensionPathUnsupported,
        WebView2ExtensionStartupRefusal::StartupFence => {
          Error::WebView2ExtensionsStartupFenceUnavailable
        }
      });
    }

    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    let hwnd = Self::create_container_hwnd(parent, &attributes, is_child)?;
    let mut construction = ControllerConstructionGuard::new(hwnd)?;

    let drop_handler = attributes.drag_drop_handler.take();
    let bounds = attributes.bounds;

    let id = attributes
      .id
      .map(|id| id.to_string())
      .unwrap_or_else(|| (hwnd.0 as isize).to_string());

    let background_color = if attributes.transparent {
      Some((0, 0, 0, 0))
    } else {
      attributes.background_color
    };

    let env = match if let Some(env) = &pl_attrs.environment {
      Ok(env.clone())
    } else {
      Self::create_environment(&attributes, pl_attrs.clone())
    } {
      Ok(env) => env,
      Err(error) => return Err(construction.finish_failure(error)),
    };
    if let Some(handler) = &pl_attrs.environment_created_handler {
      handler(&env);
    }
    let controller = match Self::create_controller(
      hwnd,
      &env,
      attributes.incognito,
      background_color,
      pl_attrs.profile_name.as_deref(),
    ) {
      Ok(controller) => controller,
      Err(error) => return Err(construction.finish_failure(error)),
    };
    construction.retain_controller(&controller);
    if let Some(startup_gate) = &pl_attrs.browser_extension_startup_gate {
      let core = unsafe { controller.CoreWebView2() }
        .map_err(webview2_com::Error::WindowsError)
        .map_err(Error::WebView2Error);
      let core = match core {
        Ok(core) => core,
        Err(error) => return Err(construction.finish_failure(error)),
      };
      if let Err(error) = startup_gate(&env, &core) {
        return Err(construction.finish_failure(Error::WebView2Error(
          webview2_com::Error::WindowsError(error),
        )));
      }
    }
    let custom_protocol_admission = InFlightAdmission::new(CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT);
    let webview = match Self::init_webview(
      parent,
      hwnd,
      id.clone(),
      attributes,
      &env,
      &controller,
      pl_attrs,
      is_child,
      &mut construction,
      &custom_protocol_admission,
    ) {
      Ok(webview) => webview,
      Err(error) => {
        cancel_pending_web_resource_responses(hwnd.0 as isize);
        custom_protocol_admission.seal_and_drain();
        return Err(construction.finish_failure(error));
      }
    };

    let initial_bounds = if is_child {
      let bounds = bounds.unwrap_or_default();
      let dpi = unsafe { util::hwnd_dpi(hwnd) };
      let scale_factor = util::dpi_to_scale_factor(dpi);
      let size = bounds.size.to_physical::<i32>(scale_factor);
      let position = bounds.position.to_physical(scale_factor);
      Self::set_bounds_native(hwnd, &controller, size, position)
    } else {
      Self::parent_bounds(parent)
        .and_then(|size| Self::set_bounds_native(hwnd, &controller, size, (0, 0).into()))
    };
    if let Err(error) = initial_bounds {
      cancel_pending_web_resource_responses(hwnd.0 as isize);
      custom_protocol_admission.seal_and_drain();
      return Err(construction.finish_failure(error));
    }

    let drag_drop_controller = drop_handler.map(|handler| {
      // Disable file drops, so our handler can capture it
      unsafe {
        let _ = controller
          .cast::<ICoreWebView2Controller4>()
          .and_then(|c| c.SetAllowExternalDrop(false));
      }
      DragDropController::new(hwnd, handler)
    });

    let Some(cleanup) = construction.into_cleanup() else {
      cancel_pending_web_resource_responses(hwnd.0 as isize);
      custom_protocol_admission.seal_and_drain();
      ORPHANED_CLEANUP_OVERFLOWED.with(|overflowed| overflowed.set(true));
      std::mem::forget(controller);
      return Err(Error::WebView2Error(webview2_com::Error::WindowsError(
        windows::core::Error::from_hresult(E_UNEXPECTED),
      )));
    };
    Ok(Self {
      id,
      parent: RefCell::new(parent),
      hwnd,
      controller,
      is_child,
      webview,
      env,
      custom_protocol_admission,
      cleanup: RefCell::new(Some(cleanup)),
      drag_drop_controller,
    })
  }

  #[inline]
  fn create_container_hwnd(
    parent: HWND,
    attributes: &WebViewAttributes,
    is_child: bool,
  ) -> Result<HWND> {
    unsafe extern "system" fn default_window_proc(
      hwnd: HWND,
      msg: u32,
      wparam: WPARAM,
      lparam: LPARAM,
    ) -> LRESULT {
      if msg == WM_SETFOCUS {
        // Fix https://github.com/DioxusLabs/dioxus/issues/2900
        // Get the first child window of the window
        let child = GetWindow(hwnd, GW_CHILD).ok();
        if child.is_some() {
          // Set focus to the child window(WebView document)
          let _ = SetFocus(child);
        }
      }

      DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    let class_name = w!("WRY_WEBVIEW");

    let class = WNDCLASSEXW {
      cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
      style: CS_HREDRAW | CS_VREDRAW,
      lpfnWndProc: Some(default_window_proc),
      cbClsExtra: 0,
      cbWndExtra: 0,
      hInstance: unsafe { HINSTANCE(GetModuleHandleW(PCWSTR::null()).unwrap_or_default().0) },
      hIcon: HICON::default(),
      hCursor: HCURSOR::default(),
      hbrBackground: HBRUSH::default(),
      lpszMenuName: PCWSTR::null(),
      lpszClassName: class_name,
      hIconSm: HICON::default(),
    };

    unsafe { RegisterClassExW(&class) };

    let mut window_styles = WS_CHILD | WS_CLIPCHILDREN;
    if attributes.visible {
      window_styles |= WS_VISIBLE;
    }

    let dpi = unsafe { util::hwnd_dpi(parent) };
    let scale_factor = util::dpi_to_scale_factor(dpi);

    let (x, y, width, height) = if is_child {
      let (x, y) = attributes
        .bounds
        .map(|b| b.position.to_physical::<f64>(scale_factor))
        .map(Into::into)
        .unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT));
      let (width, height) = attributes
        .bounds
        .map(|b| b.size.to_physical::<u32>(scale_factor))
        .map(Into::into)
        .unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT));

      (x, y, width, height)
    } else {
      let PhysicalSize { width, height } = Self::parent_bounds(parent)?;
      (0, 0, width, height)
    };

    let hwnd = unsafe {
      CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        class_name,
        PCWSTR::null(),
        window_styles,
        x,
        y,
        width,
        height,
        Some(parent),
        None,
        GetModuleHandleW(PCWSTR::null()).map(Into::into).ok(),
        None,
      )?
    };

    unsafe {
      SetWindowPos(
        hwnd,
        Some(HWND_TOP),
        0,
        0,
        0,
        0,
        SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOOWNERZORDER | SWP_NOSIZE,
      )
    }?;

    Ok(hwnd)
  }

  #[inline]
  fn create_environment(
    attributes: &WebViewAttributes,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
  ) -> Result<ICoreWebView2Environment> {
    let data_directory = attributes
      .context
      .as_deref()
      .and_then(|context| context.data_directory())
      .map(HSTRING::from);

    // additional browser args
    let additional_browser_args = pl_attrs.additional_browser_args.unwrap_or_else(|| {
      // remove "mini menu" - See https://github.com/tauri-apps/wry/issues/535
      // and "smart screen" - See https://github.com/tauri-apps/tauri/issues/1345
      let default_args = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";
      let mut arguments = String::from(default_args);

      if attributes.autoplay {
        arguments.push_str(" --autoplay-policy=no-user-gesture-required");
      }

      if let Some(proxy_setting) = &attributes.proxy_config {
        match proxy_setting {
          ProxyConfig::Http(endpoint) => {
            arguments.push_str(" --proxy-server=http://");
            arguments.push_str(&endpoint.host);
            arguments.push(':');
            arguments.push_str(&endpoint.port);
          }
          ProxyConfig::Socks5(endpoint) => {
            arguments.push_str(" --proxy-server=socks5://");
            arguments.push_str(&endpoint.host);
            arguments.push(':');
            arguments.push_str(&endpoint.port);
          }
        };
      }

      arguments
    });

    let (tx, rx) = mpsc::channel();
    let options = CoreWebView2EnvironmentOptions::default();
    unsafe {
      options.set_additional_browser_arguments(additional_browser_args);
      // Zephium keeps crash diagnostics local for explicit user sharing. This
      // disables WebView2's automatic crash upload, not SmartScreen or the
      // runtime's separately governed required diagnostics.
      options.set_is_custom_crash_reporting_enabled(true);
      // A true value can reach this boundary only with the startup gate
      // retained above. The gate runs against the exact controller/profile
      // before WebView initialization or initial navigation.
      options.set_are_browser_extensions_enabled(pl_attrs.browser_extensions_enabled);

      // Get user's system language
      let lcid = GetUserDefaultUILanguage();
      let mut lang = [0; MAX_LOCALE_NAME as usize];
      LCIDToLocaleName(lcid as u32, Some(&mut lang), LOCALE_ALLOW_NEUTRAL_NAMES);
      options.set_language(String::from_utf16_lossy(&lang));

      let scroll_bar_style = match pl_attrs.scroll_bar_style {
        ScrollBarStyle::Default => COREWEBVIEW2_SCROLLBAR_STYLE_DEFAULT,
        ScrollBarStyle::FluentOverlay => COREWEBVIEW2_SCROLLBAR_STYLE_FLUENT_OVERLAY,
      };

      options.set_scroll_bar_style(scroll_bar_style);

      CreateCoreWebView2EnvironmentWithOptions(
        PCWSTR::null(),
        &data_directory.unwrap_or_default(),
        &ICoreWebView2EnvironmentOptions::from(options),
        // we don't use CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async
        // as it uses an mspc::channel under the hood, so we can avoid using two channels
        // by manually creating the callback handler and use webview2_com::with_with_bump
        &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
          move |error_code, environment| {
            let result = (|| {
              error_code?;
              environment.ok_or_else(|| windows::core::Error::from(E_POINTER).into())
            })();
            tx.send(result)
              .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
          },
        )),
      )?;
    }

    webview2_com::wait_with_pump(rx)?
  }

  #[inline]
  fn create_controller(
    hwnd: HWND,
    env: &ICoreWebView2Environment,
    incognito: bool,
    background_color: Option<(u8, u8, u8, u8)>,
    profile_name: Option<&str>,
  ) -> Result<ICoreWebView2Controller> {
    let (tx, rx) = mpsc::channel();

    // we don't use CreateCoreWebView2ControllerCompletedHandler::wait_for_async
    // as it uses an mspc::channel under the hood, so we can avoid using two channels
    // by manually creating the callback handler and use webview2_com::with_with_bump
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
      move |error_code, controller| {
        let result = (|| {
          error_code?;
          controller.ok_or_else(|| windows::core::Error::from(E_POINTER).into())
        })();
        tx.send(result)
          .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
      },
    ));

    unsafe {
      if let Ok(env10) = env.cast::<ICoreWebView2Environment10>() {
        let controller_opts = env10.CreateCoreWebView2ControllerOptions()?;

        if let Some((r, g, b, mut a)) = background_color {
          if let Ok(opts3) = controller_opts.cast::<ICoreWebView2ControllerOptions3>() {
            if a != 0 {
              a = 255;
            }
            opts3.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
              R: r,
              G: g,
              B: b,
              A: a,
            })?;
          }
        }

        controller_opts.SetIsInPrivateModeEnabled(incognito)?;

        if let Some(name) = profile_name {
          controller_opts.SetProfileName(&HSTRING::from(name))?;
        }

        env10.CreateCoreWebView2ControllerWithOptions(hwnd, &controller_opts, &handler)?;
      } else {
        env.CreateCoreWebView2Controller(hwnd, &handler)?
      }
    }

    webview2_com::wait_with_pump(rx)?
  }

  #[allow(clippy::too_many_arguments)]
  #[inline]
  fn init_webview(
    parent: HWND,
    hwnd: HWND,
    webview_id: String,
    mut attributes: WebViewAttributes,
    env: &ICoreWebView2Environment,
    controller: &ICoreWebView2Controller,
    pl_attrs: super::PlatformSpecificWebViewAttributes,
    is_child: bool,
    construction: &mut ControllerConstructionGuard,
    custom_protocol_admission: &InFlightAdmission,
  ) -> Result<ICoreWebView2> {
    let webview = unsafe { controller.CoreWebView2()? };

    // Theme
    if let Some(theme) = pl_attrs.theme {
      if let Err(error) = unsafe { set_theme(&webview, theme) } {
        match error {
          // Ignore cast error
          Error::WebView2Error(webview2_com::Error::WindowsError(windows_error))
            if windows_error.code() == E_NOINTERFACE => {}
          // Return error if other things went wrong
          _ => return Err(error),
        };
      }
    }

    // Background color
    if let Some(background_color) = attributes.background_color {
      if !attributes.transparent {
        unsafe { set_background_color(controller, background_color)? };
      }
    }

    // Transparent
    if attributes.transparent && !is_windows_7() {
      unsafe { set_background_color(controller, (0, 0, 0, 0))? };
    }

    // The EventRegistrationToken is an out-param from all of the event registration calls. We're
    // taking it in the local variable and then just ignoring it because all of the event handlers
    // are registered for the life of the webview, but if we wanted to be able to remove them later
    // we would hold onto them in self.
    let mut token = EventRegistrationToken::default();

    // Webview Settings
    unsafe { Self::set_webview_settings(&webview, &attributes, &pl_attrs)? };

    // Webview handlers
    unsafe {
      Self::attach_handlers(
        hwnd,
        controller,
        &webview,
        &mut attributes,
        &mut token,
        env,
        &pl_attrs,
      )?
    };

    // IPC handler
    if attributes.ipc_handler.is_some() {
      unsafe { Self::attach_ipc_handler(&webview, &mut attributes, &mut token)? };
    }

    // Custom protocols handler
    let http_or_https = if pl_attrs.use_https { "https" } else { "http" };
    let custom_protocols: HashSet<String> = attributes
      .custom_protocols
      .iter()
      .map(|n| n.0.clone())
      .collect();
    if !attributes.custom_protocols.is_empty() {
      unsafe {
        Self::attach_custom_protocol_handler(
          &webview,
          env,
          hwnd,
          webview_id,
          http_or_https,
          &mut attributes,
          &mut token,
          custom_protocol_admission,
        )?
      };
    }

    // Initialize main and subframe scripts
    for init_script in attributes.initialization_scripts {
      Self::add_script_to_execute_on_document_created(&webview, init_script.script)?;
    }

    // Install one construction-time broker for every permission. Missing
    // callbacks, malformed native state, and `Default` all remain denied;
    // `Prompt` defers only camera and microphone to WebView2's own prompt.
    let clipboard = attributes.clipboard;
    let permission_handler = attributes.permission_handler.take();
    unsafe {
      webview.add_PermissionRequested(
        &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
          let Some(args) = args else { return Ok(()) };
          args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
          // A persisted decision would outlive this broker's policy and
          // suppress later requests, so nothing set here is saved.
          if let Ok(args3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
            let _ = args3.SetSavesInProfile(false);
          }

          let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
          args.PermissionKind(&mut kind)?;

          let permission_kind = match kind {
            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE => PermissionKind::Microphone,
            COREWEBVIEW2_PERMISSION_KIND_CAMERA => PermissionKind::Camera,
            COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION => PermissionKind::Geolocation,
            COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS => PermissionKind::Notifications,
            COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ => PermissionKind::ClipboardRead,
            COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS => PermissionKind::LocalFonts,
            COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS => PermissionKind::Sensors,
            COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES => PermissionKind::Midi,
            COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS => {
              PermissionKind::AutomaticDownloads
            }
            COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE => PermissionKind::FileSystemAccess,
            COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY => PermissionKind::Autoplay,
            COREWEBVIEW2_PERMISSION_KIND_WINDOW_MANAGEMENT => PermissionKind::WindowManagement,
            _ => PermissionKind::Other,
          };

          let response = if clipboard && kind == COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ {
            PermissionResponse::Allow
          } else {
            permission_handler
              .as_ref()
              .map(|handler| handler(permission_kind))
              .unwrap_or(PermissionResponse::Deny)
          };
          let media = kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
            || kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE;
          match response {
            PermissionResponse::Allow => args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?,
            PermissionResponse::Prompt if media => {
              args.SetState(COREWEBVIEW2_PERMISSION_STATE_DEFAULT)?
            }
            _ => {}
          }
          Ok(())
        })),
        &mut token,
      )?;

      // Authentication and client-certificate requests are separate native
      // surfaces; `PermissionRequested` does not cover them. Requiring these
      // interfaces/registrations at construction prevents WebView2 from
      // falling back to unowned browser dialogs for hostile raw content.
      let webview10 = webview.cast::<ICoreWebView2_10>()?;
      webview10.add_BasicAuthenticationRequested(
        &BasicAuthenticationRequestedEventHandler::create(Box::new(|_, args| {
          if let Some(args) = args {
            args.SetCancel(true)?;
          }
          Ok(())
        })),
        &mut token,
      )?;

      let webview5 = webview.cast::<ICoreWebView2_5>()?;
      webview5.add_ClientCertificateRequested(
        &ClientCertificateRequestedEventHandler::create(Box::new(|_, args| {
          if let Some(args) = args {
            // Cancellation is set before `Handled` so even a later HRESULT
            // failure cannot re-enable native certificate selection.
            args.SetCancel(true)?;
            args.SetHandled(true)?;
          }
          Ok(())
        })),
        &mut token,
      )?;

      // Save As has its own native UI event and is not comprehensively
      // covered by context-menu, accelerator, PDF-toolbar, or download
      // suppression. Require the current interface and cancel before
      // suppressing the dialog so a later HRESULT failure stays fail-closed.
      let webview25 = webview.cast::<ICoreWebView2_25>()?;
      webview25.add_SaveAsUIShowing(
        &SaveAsUIShowingEventHandler::create(Box::new(|_, args| {
          if let Some(args) = args {
            args.SetCancel(true)?;
            args.SetSuppressDefaultDialog(true)?;
          }
          Ok(())
        })),
        &mut token,
      )?;
    }

    // Navigation
    if let Some(mut url) = attributes.url {
      if let Some((protocol, _)) = url.split_once("://") {
        if custom_protocols.contains(protocol) {
          // WebView2 supports non-standard protocols only on Windows 10+, so we have to use this workaround
          // See https://github.com/MicrosoftEdge/WebView2Feedback/issues/73
          url = custom_protocol_workaround::apply_uri_work_around(&url, http_or_https, protocol)
        }
      }

      if let Some(headers) = attributes.headers {
        load_url_with_headers(&webview, env, &url, headers)?;
      } else {
        let url = HSTRING::from(url);
        unsafe { webview.Navigate(&url)? };
      }
    } else if let Some(html) = attributes.html {
      let html = HSTRING::from(html);
      unsafe { webview.NavigateToString(&html)? };
    }

    // Subclass parent for resizing and focus
    if !is_child {
      let registration = unsafe { Self::attach_parent_subclass(parent, controller)? };
      construction.retain_parent_subclass(registration);
    }

    unsafe {
      controller.SetIsVisible(attributes.visible)?;

      if attributes.focused {
        controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
      }
    }

    Ok(webview)
  }

  #[inline]
  unsafe fn set_webview_settings(
    webview: &ICoreWebView2,
    attributes: &WebViewAttributes,
    pl_attrs: &super::PlatformSpecificWebViewAttributes,
  ) -> Result<()> {
    let settings = webview.Settings()?;
    settings.SetIsStatusBarEnabled(false)?;
    settings.SetAreDefaultContextMenusEnabled(pl_attrs.default_context_menus)?;
    // JavaScript alert/confirm/prompt must be brokered by the browser chrome;
    // never let a raw page summon WebView2's unlabelled default dialog.
    settings.SetAreDefaultScriptDialogsEnabled(false)?;
    settings.SetIsZoomControlEnabled(attributes.zoom_hotkeys_enabled)?;
    settings.SetAreDevToolsEnabled(attributes.devtools)?;
    settings.SetIsScriptEnabled(!attributes.javascript_disabled)?;

    if let Some(user_agent) = &attributes.user_agent {
      if let Ok(settings2) = settings.cast::<ICoreWebView2Settings2>() {
        settings2.SetUserAgent(&HSTRING::from(user_agent))?;
      }
    }

    if !pl_attrs.browser_accelerator_keys {
      // Callers use this as a security postcondition (among other things it
      // removes WebView2's unbrokered Ctrl+P surface), not as a best-effort UI
      // preference.  A runtime too old to expose Settings3 must therefore
      // fail construction, and a runtime that accepts but does not apply the
      // setting must not receive untrusted navigation.
      let settings3 = settings.cast::<ICoreWebView2Settings3>()?;
      settings3.SetAreBrowserAcceleratorKeysEnabled(false)?;
      let mut browser_accelerator_keys_enabled = BOOL::default();
      settings3.AreBrowserAcceleratorKeysEnabled(&mut browser_accelerator_keys_enabled)?;
      if browser_accelerator_keys_enabled.as_bool() {
        return Err(windows::core::Error::from_hresult(E_ACCESSDENIED).into());
      }
    }

    if let Ok(settings4) = settings.cast::<ICoreWebView2Settings4>() {
      settings4.SetIsGeneralAutofillEnabled(attributes.general_autofill_enabled)?;
    }

    if let Ok(settings5) = settings.cast::<ICoreWebView2Settings5>() {
      settings5.SetIsPinchZoomEnabled(attributes.zoom_hotkeys_enabled)?;
    }

    if let Ok(settings6) = settings.cast::<ICoreWebView2Settings6>() {
      settings6.SetIsSwipeNavigationEnabled(attributes.back_forward_navigation_gestures)?;
    }

    if let Ok(settings9) = settings.cast::<ICoreWebView2Settings9>() {
      settings9.SetIsNonClientRegionSupportEnabled(true)?;
    }

    Ok(())
  }

  #[inline]
  unsafe fn attach_handlers(
    hwnd: HWND,
    controller: &ICoreWebView2Controller,
    webview: &ICoreWebView2,
    attributes: &mut WebViewAttributes,
    token: &mut EventRegistrationToken,
    env: &ICoreWebView2Environment,
    pl_attrs: &super::PlatformSpecificWebViewAttributes,
  ) -> Result<()> {
    // Page `window.close()` is only a request. The secure default deliberately
    // keeps the child HWND alive so an embedder cannot retain a logical view
    // and controller whose native container was destroyed behind its back.
    // Legacy destruction is an explicit opt-in for lifecycle-aware hosts.
    let page_close_policy = attributes.page_close_policy;
    let page_close_handler = attributes.page_close_handler.take();
    webview.add_WindowCloseRequested(
      &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
        if let Some(handler) = &page_close_handler {
          let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(handler));
          return Ok(());
        }
        match page_close_policy {
          crate::PageClosePolicy::Ignore => Ok(()),
          crate::PageClosePolicy::DestroyContainer => DestroyWindow(hwnd),
        }
      })),
      token,
    )?;

    // Document title changed handler
    if let Some(document_title_changed_handler) = attributes.document_title_changed_handler.take() {
      webview.add_DocumentTitleChanged(
        &DocumentTitleChangedEventHandler::create(Box::new(move |webview, _| {
          let Some(webview) = webview else {
            return Ok(());
          };

          let title = {
            let mut title = PWSTR::null();
            webview.DocumentTitle(&mut title)?;
            take_pwstr_bounded(title, PAGE_TITLE_LIMIT)
          };

          if let Some(title) = title {
            document_title_changed_handler(title);
          }
          Ok(())
        })),
        token,
      )?;
    }

    // Identity-bearing main-frame navigation handler. WebView2 explicitly
    // guarantees that redirects retain the original NavigationId and that
    // different navigation IDs may overlap. Preserve those native facts
    // instead of deriving document identity from the mutable Source value.
    let navigation_event_handler = attributes.navigation_event_handler.take().map(Rc::new);
    let navigation_presentation_guard = attributes.navigation_presentation_guard.take();
    // Never hold this lock across a COM call or embedder callback. A mutex
    // avoids RefCell panics in page-facing callbacks while its poison recovery
    // keeps malformed/re-entrant native activity from aborting the process.
    let in_flight_navigation_urls = Rc::new(Mutex::new(InFlightNavigationUrls::default()));
    if let Some(handler) = navigation_event_handler.as_ref() {
      let committed_urls = in_flight_navigation_urls.clone();
      let committed_handler = handler.clone();
      let committed_controller = controller.clone();
      webview.add_ContentLoading(
        &ContentLoadingEventHandler::create(Box::new(move |_, args| {
          let Some(args) = args else {
            return Ok(());
          };
          let mut navigation_id = 0;
          args.NavigationId(&mut navigation_id)?;
          // WebView2's own error page is not the document the person asked
          // for. Hide it like any commit, but report no commit: the failure
          // that follows lets the embedder explain it in its own surface.
          let mut error_page = BOOL::default();
          args.IsErrorPage(&mut error_page)?;
          if error_page.as_bool() {
            let tracked = committed_urls
              .lock()
              .unwrap_or_else(|poisoned| poisoned.into_inner())
              .urls
              .contains_key(&navigation_id);
            if tracked {
              if let Some(guard) = navigation_presentation_guard.as_ref() {
                guard();
                let _ = ShowWindow(hwnd, SW_HIDE);
                let _ = committed_controller.SetIsVisible(false);
              }
            }
            return Ok(());
          }
          let url = committed_urls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .commit(navigation_id);
          if let Some(url) = url {
            if let Some(guard) = navigation_presentation_guard.as_ref() {
              // ContentLoading precedes document script/content presentation.
              // Revoke the embedder's exact reveal permit before any native
              // hide call can pump re-entrant stage work, then hide the child
              // HWND and controller once per navigation identity.
              guard();
              let _ = ShowWindow(hwnd, SW_HIDE);
              let _ = committed_controller.SetIsVisible(false);
            }
            committed_handler(NavigationEvent {
              id: NavigationId::from_raw(navigation_id),
              phase: NavigationEventPhase::Committed,
              url,
            });
          }
          Ok(())
        })),
        token,
      )?;

      let completed_urls = in_flight_navigation_urls.clone();
      let completed_handler = handler.clone();
      let failure_handler = attributes.navigation_failure_handler.take();
      webview.add_NavigationCompleted(
        &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
          let Some(args) = args else {
            return Ok(());
          };
          let mut navigation_id = 0;
          let mut succeeded = BOOL::default();
          args.NavigationId(&mut navigation_id)?;
          args.IsSuccess(&mut succeeded)?;
          let mut status = COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
          if !succeeded.as_bool() {
            args.WebErrorStatus(&mut status)?;
          }
          let url = completed_urls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish(navigation_id);
          if let Some(url) = url {
            if !succeeded.as_bool() {
              if let (Some(report), Some(failure)) =
                (failure_handler.as_ref(), web_error_failure(status))
              {
                report(NavigationId::from_raw(navigation_id), failure);
              }
            }
            completed_handler(NavigationEvent {
              id: NavigationId::from_raw(navigation_id),
              phase: navigation_completion_phase(succeeded.as_bool(), status),
              url,
            });
          }
          Ok(())
        })),
        token,
      )?;
    }

    // Legacy page load handler.
    if let Some(on_page_load_handler) = attributes.on_page_load_handler.take() {
      let on_page_load_handler = Rc::new(on_page_load_handler);
      let on_page_load_handler_ = on_page_load_handler.clone();

      webview.add_ContentLoading(
        &ContentLoadingEventHandler::create(Box::new(move |webview, _| {
          let Some(webview) = webview else {
            return Ok(());
          };

          if let Some(url) = Self::bounded_url_from_webview(&webview)? {
            on_page_load_handler_(PageLoadEvent::Started, url);
          }

          Ok(())
        })),
        token,
      )?;
      webview.add_NavigationCompleted(
        &NavigationCompletedEventHandler::create(Box::new(move |webview, _| {
          let Some(webview) = webview else {
            return Ok(());
          };

          if let Some(url) = Self::bounded_url_from_webview(&webview)? {
            on_page_load_handler(PageLoadEvent::Finished, url);
          }

          Ok(())
        })),
        token,
      )?;
    }

    // Navigation policy and identity start/redirect observation share one
    // callback so a denied or malformed target can never be reported as an
    // admitted navigation. Read every identity field while cancellation is
    // still dominant; a getter failure therefore remains fail-closed.
    let nav_callback = attributes.navigation_handler.take();
    if nav_callback.is_some() || navigation_event_handler.is_some() {
      let starting_handler = navigation_event_handler.clone();
      let starting_urls = in_flight_navigation_urls.clone();
      webview.add_NavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(move |_, args| {
          let Some(args) = args else {
            return Ok(());
          };
          if nav_callback.is_some() || starting_handler.is_some() {
            // A missing/malformed URI must never bypass the navigation callback.
            args.SetCancel(true)?;
          }

          let uri = {
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            take_pwstr_bounded(uri, PAGE_URL_LIMIT)
          };

          let admitted = match (&nav_callback, uri.as_ref()) {
            (Some(callback), Some(uri)) => callback(uri.clone()),
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (None, None) => false,
          };
          if !admitted {
            return Ok(());
          }

          let mut tracking_failed = false;
          let event = if starting_handler.is_some() {
            let mut navigation_id = 0;
            let mut redirected = BOOL::default();
            args.NavigationId(&mut navigation_id)?;
            args.IsRedirected(&mut redirected)?;
            uri.map(|url| {
              tracking_failed = !starting_urls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .admit(navigation_id, url.clone());
              NavigationEvent {
                id: NavigationId::from_raw(navigation_id),
                phase: if redirected.as_bool() {
                  NavigationEventPhase::Redirected
                } else {
                  NavigationEventPhase::Started
                },
                url,
              }
            })
          } else {
            None
          };

          if starting_handler.is_some() && event.is_none() {
            // Identity tracking is mandatory when requested. Capacity or
            // malformed native state leaves cancellation in force.
            return Ok(());
          }
          if tracking_failed {
            // The native navigation is still canceled. Complete the observed
            // start synthetically so an embedder can roll back an explicit
            // pending navigation instead of waiting for a completion whose
            // identity was deliberately not retained.
            if let (Some(handler), Some(event)) = (starting_handler.as_ref(), event.as_ref()) {
              handler(event.clone());
              handler(NavigationEvent {
                phase: NavigationEventPhase::Failed,
                ..event.clone()
              });
            }
            return Ok(());
          }
          if nav_callback.is_some() || starting_handler.is_some() {
            args.SetCancel(false)?;
          }
          if let (Some(handler), Some(event)) = (starting_handler.as_ref(), event) {
            handler(event);
          }

          Ok(())
        })),
        token,
      )?;
    }

    let new_window_req_handler = attributes
      .new_window_req_handler
      .take()
      .map(std::rc::Rc::new);
    let env_ = env.clone();
    // New window handler
    webview.add_NewWindowRequested(
      &NewWindowRequestedEventHandler::create(Box::new(move |webview, args| {
        let Some(args) = args else {
          return Ok(());
        };
        // Default to denial before reading any optional native value. URI,
        // sender, feature, deferral, or dispatch failure must not fall back to
        // WebView2's native popup behavior.
        args.SetHandled(true)?;

        if let Some(new_window_req_handler) = &new_window_req_handler {
          let Some(webview) = webview else {
            // Missing sender state is not permission to fall back to the
            // runtime's native popup behavior.
            return Ok(());
          };
          let uri = {
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            take_pwstr_bounded(uri, PAGE_URL_LIMIT)
          };
          let Some(uri) = uri else {
            return Ok(());
          };

          let mut user_initiated = BOOL::default();
          args.IsUserInitiated(&mut user_initiated)?;
          let foreground = {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
              GetKeyState, VK_CONTROL, VK_MBUTTON, VK_SHIFT,
            };
            let modified =
              GetKeyState(i32::from(VK_CONTROL.0)) < 0 || GetKeyState(i32::from(VK_MBUTTON.0)) < 0;
            !modified || GetKeyState(i32::from(VK_SHIFT.0)) < 0
          };
          let features = args
            .WindowFeatures()
            .map(|f| {
              let mut position = None;
              let mut size = None;

              let mut has_position: BOOL = false.into();
              let _ = f.HasPosition(&mut has_position);

              if has_position.as_bool() {
                let mut left = 0;
                let _ = f.Left(&mut left);
                let mut top = 0;
                let _ = f.Top(&mut top);
                position.replace(dpi::LogicalPosition::new(left as f64, top as f64));
              }

              let mut has_size: BOOL = false.into();
              let _ = f.HasSize(&mut has_size);
              if has_size.as_bool() {
                let mut width = 0;
                let _ = f.Width(&mut width);
                let mut height = 0;
                let _ = f.Height(&mut height);
                size.replace(dpi::LogicalSize::new(width as f64, height as f64));
              }

              NewWindowFeatures {
                user_initiated: user_initiated.as_bool(),
                foreground,
                position,
                size,
                opener: NewWindowOpener {
                  webview: webview.clone(),
                  environment: env_.clone(),
                },
              }
            })
            .unwrap_or_else(|_| NewWindowFeatures {
              user_initiated: user_initiated.as_bool(),
              foreground,
              position: None,
              size: None,
              opener: NewWindowOpener {
                webview: webview.clone(),
                environment: env_.clone(),
              },
            });

          let new_window_req_handler = new_window_req_handler.clone();
          let deferral = args.GetDeferral()?;
          let completion = NewWindowDeferralGuard::new(args.clone(), deferral);
          // Use `dispatch_handler` to schedule the run on the message loop after this callback completes,
          // this is needed for `new_window_req_handler` to create new webviews for `NewWindowResponse::Create`
          // or it will deadlock, see https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/threading-model#reentrancy
          unsafe {
            Self::dispatch_local_handler(hwnd, move || {
              let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                new_window_req_handler(uri, features)
              }))
              .unwrap_or(NewWindowResponse::Deny);
              completion.finish(response);
            });
          }
        } else {
          // The request was denied before optional values were inspected.
        }

        Ok(())
      })),
      token,
    )?;
    Self::attach_main_thread_dispatcher(hwnd)?;

    if let Some(handler) = pl_attrs.native_context_menu_handler.clone() {
      // Save-page/SaveAs is distinct from DownloadStarting. Keep that separate
      // filesystem surface denied even when selected download/edit menu items
      // are enabled. Registration is mandatory before any content can load.
      let core25: ICoreWebView2_25 = webview.cast()?;
      core25.add_SaveAsUIShowing(
        &SaveAsUIShowingEventHandler::create(Box::new(|_, args| {
          if let Some(args) = args {
            args.SetCancel(true)?;
          }
          Ok(())
        })),
        token,
      )?;
      let owner = controller.clone();
      let core11: ICoreWebView2_11 = webview.cast()?;
      core11.add_ContextMenuRequested(
        &ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
          let Some(args) = args else {
            return Ok(());
          };
          args.SetHandled(true)?;
          if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&owner, &args)))
            .unwrap_or(false)
          {
            args.SetHandled(false)?;
          }
          Ok(())
        })),
        token,
      )?;
    }

    // Download handler. Deny-without-metadata is a separate native path: it
    // cancels before requesting the operation URI or destination path and
    // takes precedence over callbacks regardless of builder call order.
    if attributes.download_policy.inspect_metadata(|| ()).is_none() {
      let webview4: ICoreWebView2_4 = webview.cast()?;
      webview4.add_DownloadStarting(
        &DownloadStartingEventHandler::create(Box::new(move |_, args| {
          if let Some(args) = args {
            args.SetCancel(true)?;
          }
          Ok(())
        })),
        token,
      )?;
    } else if let Some(native) = pl_attrs.native_download_handler.clone() {
      let owner = controller.clone();
      let webview4: ICoreWebView2_4 = webview.cast()?;
      webview4.add_DownloadStarting(
        &DownloadStartingEventHandler::create(Box::new(move |_, args| {
          let Some(args) = args else {
            return Ok(());
          };
          args.SetCancel(true)?;
          args.SetHandled(true)?;
          if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| native(&owner, &args)))
            .is_err()
          {
            args.SetCancel(true)?;
          }
          Ok(())
        })),
        token,
      )?;
    } else if attributes.download_started_handler.is_some()
      || attributes.download_completed_handler.is_some()
    {
      let mut download_started_handler = attributes.download_started_handler.take();
      let download_completed_handler = attributes.download_completed_handler.take();

      let webview4: ICoreWebView2_4 = webview.cast()?;
      webview4.add_DownloadStarting(
        &DownloadStartingEventHandler::create(Box::new(move |_, args| {
          let Some(args) = args else {
            return Ok(());
          };
          // Keep cancellation set until an explicit started-handler accepts a
          // sanitized destination. Completed-only handlers are observational,
          // not authorization to invoke the native download surface.
          args.SetCancel(true)?;

          let operation = args.DownloadOperation()?;
          let uri = {
            let mut uri = PWSTR::null();
            operation.Uri(&mut uri)?;
            take_pwstr_bounded(uri, PAGE_URL_LIMIT)
          };
          let Some(uri) = uri else {
            return Ok(());
          };

          if let Some(download_completed_handler) = &download_completed_handler {
            let download_completed_handler = download_completed_handler.clone();
            let terminal_reported = Rc::new(Cell::new(false));

            operation.add_StateChanged(
              &StateChangedEventHandler::create(Box::new(move |download_operation, _| {
                let Some(download_operation) = download_operation else {
                  return Ok(());
                };

                let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
                download_operation.State(&mut state)?;

                if state != COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS
                  && !terminal_reported.replace(true)
                {
                  let uri = {
                    let mut uri = PWSTR::null();
                    download_operation.Uri(&mut uri)?;
                    take_pwstr_bounded(uri, PAGE_URL_LIMIT)
                  };
                  let Some(uri) = uri else {
                    return Ok(());
                  };

                  let success = state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;

                  let path = if success {
                    let mut path = PWSTR::null();
                    download_operation.ResultFilePath(&mut path)?;
                    Some(PathBuf::from(take_pwstr(path)))
                  } else {
                    None
                  };

                  download_completed_handler(uri, path, success);
                }

                Ok(())
              })),
              &mut EventRegistrationToken::default(),
            )?;
          }

          if let Some(download_started_handler) = &mut download_started_handler {
            let mut path = {
              let mut path = PWSTR::null();
              args.ResultFilePath(&mut path)?;
              let path = take_pwstr(path);
              PathBuf::from(&path)
            };

            if download_started_handler(uri, &mut path) {
              let simplified = dunce::simplified(&path);
              let path = HSTRING::from(simplified);
              args.SetResultFilePath(&path)?;
              args.SetHandled(true)?;
              args.SetCancel(false)?;
            }
          }

          Ok(())
        })),
        token,
      )?;
    }

    Ok(())
  }

  #[inline]
  unsafe fn attach_ipc_handler(
    webview: &ICoreWebView2,
    attributes: &mut WebViewAttributes,
    token: &mut EventRegistrationToken,
  ) -> Result<()> {
    Self::add_script_to_execute_on_document_created(
      webview,
      String::from(
        r#"Object.defineProperty(window, 'ipc', { value: Object.freeze({ postMessage: function(s) { if (typeof s === 'string' && s.length <= 65536) window.chrome.webview.postMessage(s); } }) });"#,
      ),
    )?;

    let ipc_handler = attributes.ipc_handler.take();
    webview.add_WebMessageReceived(
      &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
        let (Some(args), Some(ipc_handler)) = (args, &ipc_handler) else {
          return Ok(());
        };

        let url = {
          let mut url = PWSTR::null();
          args.Source(&mut url)?;
          take_pwstr_bounded(url, PAGE_URL_LIMIT)
        };
        let Some(url) = url else {
          return Ok(());
        };

        let js = {
          let mut js = PWSTR::null();
          if args.TryGetWebMessageAsString(&mut js).is_err() {
            return Ok(());
          }
          let Some(js) = take_pwstr_bounded(js, IPC_PAYLOAD_LIMIT) else {
            return Ok(());
          };
          js
        };

        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!(parent: None, "wry::ipc::handle").entered();
        let Ok(request) = Request::builder().uri(url).body(js) else {
          // A malformed native Source value is not allowed to abort a
          // panic=abort embedder from page-driven message delivery.
          return Ok(());
        };
        ipc_handler(request);

        Ok(())
      })),
      token,
    )?;

    Ok(())
  }

  #[allow(clippy::too_many_arguments)]
  #[inline]
  unsafe fn attach_custom_protocol_handler(
    webview: &ICoreWebView2,
    env: &ICoreWebView2Environment,
    hwnd: HWND,
    webview_id: String,
    http_or_https: &'static str,
    attributes: &mut WebViewAttributes,
    token: &mut EventRegistrationToken,
    protocol_admission: &InFlightAdmission,
  ) -> Result<()> {
    for name in attributes.custom_protocols.keys() {
      // WebView2 supports non-standard protocols only on Windows 10+, so we have to use this workaround
      // See https://github.com/MicrosoftEdge/WebView2Feedback/issues/73
      let work_around_uri = custom_protocol_workaround::work_around_uri_prefix(http_or_https, name);
      let filter = HSTRING::from(format!("{work_around_uri}*"));

      // If WebView2 version is high enough, use the new API to add the filter to allow Shared Workers and
      // iframes to work with custom protocols
      // See https://github.com/MicrosoftEdge/WebView2Feedback/issues/1114
      if let Ok(webview_22) = webview.cast::<ICoreWebView2_22>() {
        webview_22.AddWebResourceRequestedFilterWithRequestSourceKinds(
          &filter,
          COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
          COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        )?;
      } else {
        // Fallback to the old API
        webview.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
      }
    }

    let env = env.clone();
    let custom_protocols = std::mem::take(&mut attributes.custom_protocols);
    let main_thread_id = std::thread::current().id();
    let protocol_admission = protocol_admission.clone();

    webview.add_WebResourceRequested(
      &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
          return Ok(());
        };

        #[cfg(feature = "tracing")]
        let span = tracing::info_span!(parent: None, "wry::custom_protocol::handle", uri = tracing::field::Empty)
          .entered();

        // Request uri
        let webview_request = args.Request()?;

        // Request uri
        let uri = {
          let mut uri = PWSTR::null();
          webview_request.Uri(&mut uri)?;
          take_pwstr_bounded(uri, PAGE_URL_LIMIT)
        };
        let Some(uri) = uri else {
          let err_response = Self::prepare_web_request_err(
            &env,
            "custom-protocol URL exceeded the native-to-Rust allocation limit",
          )?;
          args.SetResponse(&err_response)?;
          return Ok(());
        };
        #[cfg(feature = "tracing")]
        span.record("uri", &uri);

        if let Some((custom_protocol, custom_protocol_handler)) = custom_protocols
          .iter()
          .find(|(protocol, _)| custom_protocol_workaround::is_work_around_uri(&uri, http_or_https, protocol))
        {
          let Some(permit) = protocol_admission.try_acquire() else {
            // Do not acquire a native deferral for rejected work. Installing
            // the empty failure response synchronously completes this event
            // and leaves no pending object or accounting obligation behind.
            let failure = web_resource_failure_response(CUSTOM_PROTOCOL_OVERFLOW_STATUS);
            let response = Self::prepare_web_request_response(&env, &failure)?;
            args.SetResponse(&response)?;
            return Ok(());
          };
          let request = match Self::prepare_request(http_or_https, custom_protocol, &webview_request, &uri)
          {
            Ok(req) => req,
            Err(e) => {
              let err_response = Self::prepare_web_request_err(&env, e)?;
              args.SetResponse(&err_response)?;
              return Ok(());
            }
          };

          let deferral = args.GetDeferral()?;
          let Some(pending_token) =
            insert_pending_web_resource_response(PendingWebResourceResponse {
              args: args.clone(),
              env: env.clone(),
              deferral,
              hwnd: hwnd.0 as isize,
              completed: false,
              _permit: permit,
            })
          else {
            return Ok(());
          };
          let dispatcher_hwnd = hwnd.0 as isize;

          let async_responder = Box::new(move |sent_response| {
            let handler = move || {
              finish_pending_web_resource_response(pending_token, sent_response);
            };

            if std::thread::current().id() == main_thread_id {
              handler();
            } else {
              Self::dispatch_handler(dispatcher_hwnd, handler);
            }
          });

          #[cfg(feature = "tracing")]
          let _span = tracing::info_span!("wry::custom_protocol::call_handler").entered();
          custom_protocol_handler(
            &webview_id,
            request,
            RequestAsyncResponder {
              responder: Some(async_responder),
            },
          );
        }

        Ok(())
      })),
      token,
    )?;

    Ok(())
  }

  #[inline]
  unsafe fn prepare_request(
    http_or_https: &'static str,
    custom_protocol: &str,
    webview_request: &ICoreWebView2WebResourceRequest,
    webview_request_uri: &str,
  ) -> Result<http::Request<Vec<u8>>> {
    let mut request = Request::builder();
    let mut budget = CustomProtocolRequestBudget::default();

    // Request method (GET, POST, PUT etc..)
    let mut method = PWSTR::null();
    webview_request.Method(&mut method)?;
    let method = take_pwstr_bounded(method, CUSTOM_PROTOCOL_METHOD_LIMIT)
      .ok_or(Error::CustomProtocolRequestTooLarge("method"))?;
    request = request.method(method.as_str());

    // Get all headers from the request
    let headers = webview_request.Headers()?.GetIterator()?;
    let mut has_current = BOOL::default();
    headers.HasCurrentHeader(&mut has_current)?;
    while has_current.as_bool() {
      let mut key = PWSTR::null();
      let mut value = PWSTR::null();
      headers.GetCurrentHeader(&mut key, &mut value)?;

      // Convert both COM-owned values even if one is rejected so each native
      // allocation is released exactly once.
      let (key, value) = (
        take_pwstr_bounded(key, CUSTOM_PROTOCOL_HEADER_NAME_LIMIT),
        take_pwstr_bounded(value, CUSTOM_PROTOCOL_HEADER_VALUE_LIMIT),
      );
      let (Some(key), Some(value)) = (key, value) else {
        return Err(Error::CustomProtocolRequestTooLarge("header field"));
      };
      if !budget.admit_header(key.len(), value.len()) {
        return Err(Error::CustomProtocolRequestTooLarge("header aggregate"));
      }
      request = request.header(&key, &value);

      headers.MoveNext(&mut has_current)?;
    }

    // Get the body if available
    let mut body_sent = Vec::new();
    if let Ok(content) = webview_request.Content() {
      let mut buffer: [u8; 1024] = [0; 1024];
      loop {
        let mut cb_read = 0;
        let content: IStream = content.cast()?;
        content
          .Read(
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as u32,
            Some(&mut cb_read),
          )
          .ok()?;

        // IStream is a native trust boundary. Validate the reported count
        // before using it for budget arithmetic or constructing a slice;
        // a broken/malicious COM implementation must not turn an impossible
        // count into a Rust bounds panic in a panic=abort browser.
        let Some(cb_read) = bounded_stream_read_len(cb_read, buffer.len()) else {
          return Err(Error::CustomProtocolRequestTooLarge("body chunk"));
        };

        if cb_read == 0 {
          break;
        }

        if !budget.admit_body_chunk(cb_read) {
          return Err(Error::CustomProtocolRequestTooLarge("body"));
        }
        body_sent.extend_from_slice(&buffer[..cb_read]);
      }
    }

    // Undo the protocol workaround when giving path to resolver
    let path = custom_protocol_workaround::revert_uri_work_around(
      webview_request_uri,
      http_or_https,
      custom_protocol,
    );

    let request = request.uri(&path).body(body_sent)?;

    Ok(request)
  }

  #[inline]
  unsafe fn prepare_web_request_response(
    env: &ICoreWebView2Environment,
    sent_response: &HttpResponse<Cow<'static, [u8]>>,
  ) -> windows::core::Result<ICoreWebView2WebResourceResponse> {
    let content = sent_response.body();

    let status = sent_response.status();
    let status_code = status.as_u16();
    let status = HSTRING::from(status.canonical_reason().unwrap_or("OK"));

    let mut headers_map = String::new();
    for (name, value) in sent_response.headers().iter() {
      let header_key = name.to_string();
      if let Ok(value) = value.to_str() {
        let _ = writeln!(headers_map, "{}: {}", header_key, value);
      }
    }
    let headers_map = HSTRING::from(headers_map);

    let mut stream = None;
    if !content.is_empty() {
      stream = SHCreateMemStream(Some(content));
    }

    env.CreateWebResourceResponse(stream.as_ref(), status_code as i32, &status, &headers_map)
  }

  #[inline]
  unsafe fn prepare_web_request_err<T>(
    env: &ICoreWebView2Environment,
    _err: T,
  ) -> windows::core::Result<ICoreWebView2WebResourceResponse> {
    let status = StatusCode::BAD_REQUEST;
    let status_code = status.as_u16();
    let status = HSTRING::from(status.canonical_reason().unwrap_or("Bad Request"));
    // The final argument is a raw HTTP header block, not an error-message
    // field. Passing arbitrary error text here can make response construction
    // fail and may expose implementation details to untrusted content.
    let headers = HSTRING::new();
    env.CreateWebResourceResponse(None, status_code as i32, &status, &headers)
  }

  #[inline]
  fn dispatch_handler<F>(hwnd: isize, function: F) -> bool
  where
    F: FnOnce() + Send + 'static,
  {
    // The only value crossing from the worker is a Send closure. Apartment-
    // bound COM objects remain in the UI thread's pending-response registry.
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    unsafe { Self::enqueue_dispatch(hwnd, Box::new(function)) }
  }

  /// Queue a non-Send closure without moving it between OS threads.
  ///
  /// # Safety
  ///
  /// The caller must already be running on `hwnd`'s owning thread. The window
  /// procedure invokes or drops the closure on that same thread.
  #[inline]
  unsafe fn dispatch_local_handler<F>(hwnd: HWND, function: F) -> bool
  where
    F: FnOnce() + 'static,
  {
    Self::enqueue_dispatch(hwnd, Box::new(function))
  }

  unsafe fn enqueue_dispatch(hwnd: HWND, function: Box<dyn FnOnce()>) -> bool {
    // We double-box because the trait object is a fat pointer.
    let boxed2: Box<Box<dyn FnOnce()>> = Box::new(function);

    let raw = Box::into_raw(boxed2);
    let capability = raw as usize;
    let Ok(mut pending) = PENDING_DISPATCHES.lock() else {
      drop(Box::from_raw(raw));
      return false;
    };
    if !dispatch_registry_has_capacity(pending.len()) {
      drop(pending);
      drop(Box::from_raw(raw));
      return false;
    }
    pending.insert(capability, hwnd.0 as isize);
    drop(pending);

    let result = PostMessageW(Some(hwnd), *EXEC_MSG_ID, WPARAM(capability), LPARAM(0));
    if result.is_err() {
      if let Ok(mut pending) = PENDING_DISPATCHES.lock() {
        pending.remove(&capability);
      }
      // `PostMessageW` failed, so the window procedure cannot concurrently own
      // this allocation.
      drop(Box::from_raw(raw));
    }

    #[cfg(any(debug_assertions, feature = "tracing"))]
    if let Err(err) = &result {
      let msg = format!(
        "PostMessage failed ; is the messages queue full? Error code {} - {}",
        err.code(),
        err.message()
      );
      #[cfg(feature = "tracing")]
      tracing::error!("{msg}");
      #[cfg(debug_assertions)]
      eprintln!("{msg}");
    }
    result.is_ok()
  }

  unsafe extern "system" fn main_thread_dispatcher_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uidsubclass: usize,
    _dwrefdata: usize,
  ) -> LRESULT {
    if msg == *EXEC_MSG_ID {
      let capability = wparam.0;
      let authorized = PENDING_DISPATCHES
        .lock()
        .ok()
        .and_then(|mut pending| pending.remove(&capability))
        == Some(hwnd.0 as isize);
      if !authorized {
        return DefSubclassProc(hwnd, msg, wparam, lparam);
      }
      let function: Box<Box<dyn FnOnce()>> = Box::from_raw(capability as *mut _);
      let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(function));
      let _ = RedrawWindow(Some(hwnd), None, None, RDW_INTERNALPAINT);
      return LRESULT(0);
    }

    if msg == WM_NCDESTROY {
      cancel_pending_web_resource_responses(hwnd.0 as isize);
      let abandoned = PENDING_DISPATCHES
        .lock()
        .ok()
        .map(|mut pending| {
          let capabilities: Vec<_> = pending
            .iter()
            .filter_map(|(capability, owner)| (*owner == hwnd.0 as isize).then_some(*capability))
            .collect();
          for capability in &capabilities {
            pending.remove(capability);
          }
          capabilities
        })
        .unwrap_or_default();
      for capability in abandoned {
        drop(Box::<Box<dyn FnOnce()>>::from_raw(capability as *mut _));
      }
    }

    DefSubclassProc(hwnd, msg, wparam, lparam)
  }

  unsafe fn attach_main_thread_dispatcher(hwnd: HWND) -> windows::core::Result<()> {
    if SetWindowSubclass(
      hwnd,
      Some(Self::main_thread_dispatcher_proc),
      MAIN_THREAD_DISPATCHER_SUBCLASS_ID as _,
      0,
    )
    .as_bool()
    {
      Ok(())
    } else {
      Err(windows::core::Error::from_win32())
    }
  }

  fn parent_bounds(hwnd: HWND) -> Result<PhysicalSize<i32>> {
    let mut client_rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client_rect)? };
    Ok(PhysicalSize::new(
      client_rect.right - client_rect.left,
      client_rect.bottom - client_rect.top,
    ))
  }

  unsafe extern "system" fn parent_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uidsubclass: usize,
    dwrefdata: usize,
  ) -> LRESULT {
    let state = NonNull::new(dwrefdata as *mut ParentSubclassState).map(|state| {
      // SAFETY: SetWindowSubclass receives `Rc::as_ptr` while the owning
      // registration remains in cleanup state. Taking a temporary strong
      // reference before doing any reentrant COM/USER32 work keeps the
      // allocation alive even if nested application code closes the WebView.
      Rc::increment_strong_count(state.as_ptr());
      Rc::from_raw(state.as_ptr())
    });
    match msg {
      WM_SIZE if wparam.0 != SIZE_MINIMIZED as usize => {
        let Some(state) = state.as_ref() else {
          return DefSubclassProc(hwnd, msg, wparam, lparam);
        };
        let controller = &state.controller;

        let Ok(PhysicalSize { width, height }) = Self::parent_bounds(hwnd) else {
          return DefSubclassProc(hwnd, msg, wparam, lparam);
        };

        let _ = controller.SetBounds(RECT {
          left: 0,
          top: 0,
          right: width,
          bottom: height,
        });

        let mut hwnd = HWND::default();
        if controller.ParentWindow(&mut hwnd).is_ok() {
          let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            width,
            height,
            SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOZORDER,
          );
        }
      }

      WM_SETFOCUS | WM_ENTERSIZEMOVE => {
        if let Some(state) = state.as_ref() {
          let _ = state
            .controller
            .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
      }

      msg if msg == WM_MOVE || msg == WM_MOVING => {
        if let Some(state) = state.as_ref() {
          let _ = state.controller.NotifyParentWindowPositionChanged();
        }
      }

      WM_NCDESTROY => {
        // The OS removes this subclass during final parent teardown. The
        // registration/debt remains the sole owner of `state`; never free it
        // from inside a callback that still has the raw pointer on its stack.
        if let Some(state) = state.as_ref() {
          state.detached.set(true);
        }
      }

      _ => (),
    }

    DefSubclassProc(hwnd, msg, wparam, lparam)
  }

  #[inline]
  unsafe fn attach_parent_subclass(
    parent: HWND,
    controller: &ICoreWebView2Controller,
  ) -> windows::core::Result<ParentSubclassRegistration> {
    let registration = ParentSubclassRegistration {
      parent,
      state: Rc::new(ParentSubclassState {
        controller: controller.clone(),
        detached: Cell::new(false),
      }),
    };
    let state = Rc::as_ptr(&registration.state);
    if !SetWindowSubclass(
      parent,
      Some(Self::parent_subclass_proc),
      PARENT_SUBCLASS_ID as _,
      state as _,
    )
    .as_bool()
    {
      return Err(windows::core::Error::from_win32());
    }
    Ok(registration)
  }

  #[inline]
  fn add_script_to_execute_on_document_created(webview: &ICoreWebView2, js: String) -> Result<()> {
    let webview = webview.clone();
    AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
      Box::new(move |handler| unsafe {
        let js = HSTRING::from(js);
        webview
          .AddScriptToExecuteOnDocumentCreated(&js, &handler)
          .map_err(Into::into)
      }),
      Box::new(|e, _| e),
    )
    .map_err(Into::into)
  }

  #[inline]
  fn execute_script(
    webview: &ICoreWebView2,
    js: &str,
    callback: impl FnOnce(String) + Send + 'static,
  ) -> windows::core::Result<()> {
    unsafe {
      #[cfg(feature = "tracing")]
      let span = tracing::debug_span!("wry::eval").entered();
      let js = HSTRING::from(js);
      webview.ExecuteScript(
        &js,
        &ExecuteScriptCompletedHandler::create(Box::new(|_, res| {
          #[cfg(feature = "tracing")]
          drop(span);
          callback(res);
          Ok(())
        })),
      )
    }
  }

  #[inline]
  fn bounded_url_from_webview(webview: &ICoreWebView2) -> windows::core::Result<Option<String>> {
    let mut pwstr = PWSTR::null();
    unsafe { webview.Source(&mut pwstr)? };
    Ok(take_pwstr_bounded(pwstr, PAGE_URL_LIMIT))
  }

  #[inline]
  fn url_from_webview(webview: &ICoreWebView2) -> windows::core::Result<String> {
    Self::bounded_url_from_webview(webview)?.ok_or_else(|| {
      windows::core::Error::new(
        E_INVALIDARG,
        "WebView2 URL exceeded the native-to-Rust allocation limit",
      )
    })
  }
}

/// Public APIs
impl InnerWebView {
  pub fn id(&self) -> crate::WebViewId<'_> {
    &self.id
  }

  #[inline]
  pub fn hwnd(&self) -> HWND {
    self.hwnd
  }

  pub fn eval(
    &self,
    js: &str,
    callback: Option<impl FnOnce(String) + Send + 'static>,
  ) -> Result<()> {
    if let Some(callback) = callback {
      Self::execute_script(&self.webview, js, callback)?
    } else {
      Self::execute_script(&self.webview, js, |_| ())?
    }
    Ok(())
  }

  pub fn url(&self) -> Result<String> {
    Self::url_from_webview(&self.webview).map_err(Into::into)
  }

  pub fn document_title(&self) -> Result<Option<String>> {
    let mut title = PWSTR::null();
    unsafe { self.webview.DocumentTitle(&mut title) }?;
    Ok(take_pwstr_bounded(title, PAGE_TITLE_LIMIT))
  }

  pub fn zoom(&self, scale_factor: f64) -> Result<()> {
    unsafe { self.controller.SetZoomFactor(scale_factor) }.map_err(Into::into)
  }

  pub fn load_url(&self, url: &str) -> Result<()> {
    let url = HSTRING::from(url);
    unsafe { self.webview.Navigate(&url) }.map_err(Into::into)
  }

  pub fn load_url_with_headers(&self, url: &str, headers: http::HeaderMap) -> Result<()> {
    load_url_with_headers(&self.webview, &self.env, url, headers)
  }

  pub fn load_html(&self, html: &str) -> Result<()> {
    let html = HSTRING::from(html);
    unsafe { self.webview.NavigateToString(&html) }.map_err(Into::into)
  }

  pub fn reload(&self) -> Result<()> {
    unsafe { self.webview.Reload() }.map_err(Into::into)
  }

  pub fn go_forward(&self) -> Result<()> {
    unsafe { self.webview.GoForward() }.map_err(Into::into)
  }

  pub fn go_back(&self) -> Result<()> {
    unsafe { self.webview.GoBack() }.map_err(Into::into)
  }

  pub fn can_go_forward(&self) -> Result<bool> {
    let mut can_go_forward = FALSE;
    unsafe { self.webview.CanGoForward(&mut can_go_forward) }.map_err(Into::<Error>::into)?;
    Ok(can_go_forward.into())
  }

  pub fn can_go_back(&self) -> Result<bool> {
    let mut can_go_back = FALSE;
    unsafe { self.webview.CanGoBack(&mut can_go_back) }.map_err(Into::<Error>::into)?;
    Ok(can_go_back.into())
  }

  pub fn bounds(&self) -> Result<Rect> {
    let mut bounds = Rect::default();
    let mut rect = RECT::default();
    if self.is_child {
      unsafe { GetClientRect(self.hwnd, &mut rect)? };

      let position_point = &mut [POINT {
        x: rect.left,
        y: rect.top,
      }];
      let parent = *self.parent.try_borrow().map_err(|_| {
        windows::core::Error::new(E_UNEXPECTED, "WebView parent is being updated reentrantly")
      })?;
      unsafe { MapWindowPoints(Some(self.hwnd), Some(parent), position_point) };

      bounds.position = PhysicalPosition::new(position_point[0].x, position_point[0].y).into();
    } else {
      unsafe { self.controller.Bounds(&mut rect) }?;
    }

    bounds.size = PhysicalSize::new(rect.right - rect.left, rect.bottom - rect.top).into();

    Ok(bounds)
  }

  pub fn set_bounds_inner(
    &self,
    size: PhysicalSize<i32>,
    position: PhysicalPosition<i32>,
  ) -> Result<()> {
    Self::set_bounds_native(self.hwnd, &self.controller, size, position)
  }

  fn set_bounds_native(
    hwnd: HWND,
    controller: &ICoreWebView2Controller,
    size: PhysicalSize<i32>,
    position: PhysicalPosition<i32>,
  ) -> Result<()> {
    unsafe {
      controller.SetBounds(RECT {
        top: 0,
        left: 0,
        right: size.width,
        bottom: size.height,
      })?;

      SetWindowPos(
        hwnd,
        None,
        position.x,
        position.y,
        size.width,
        size.height,
        SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOZORDER,
      )?;
    }

    Ok(())
  }

  pub fn close_explicit(&mut self) -> std::result::Result<(), WebView2CleanupDebt> {
    cancel_pending_web_resource_responses(self.hwnd.0 as isize);
    self.custom_protocol_admission.seal_and_drain();
    let Some(cleanup) = self.cleanup.get_mut().take() else {
      return Ok(());
    };
    let mut cleanup = WebView2CleanupDebt::new(cleanup);
    match cleanup.retry() {
      Ok(()) => Ok(()),
      Err(_) => Err(cleanup),
    }
  }

  pub fn set_bounds(&self, bounds: Rect) -> Result<()> {
    let dpi = unsafe { util::hwnd_dpi(self.hwnd) };
    let scale_factor = util::dpi_to_scale_factor(dpi);
    let size = bounds.size.to_physical::<i32>(scale_factor);
    let position = bounds.position.to_physical(scale_factor);
    self.set_bounds_inner(size, position)?;
    Ok(())
  }

  pub fn set_visible(&self, visible: bool) -> Result<()> {
    unsafe {
      let _ = ShowWindow(
        self.hwnd,
        match visible {
          true => SW_SHOW,
          false => SW_HIDE,
        },
      );

      self.controller.SetIsVisible(visible)?;
    }

    Ok(())
  }

  pub fn focus(&self) -> Result<()> {
    unsafe {
      self
        .controller
        .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
        .map_err(Into::into)
    }
  }

  pub fn focus_parent(&self) -> Result<()> {
    unsafe {
      let parent = *self.parent.try_borrow().map_err(|_| {
        windows::core::Error::new(E_UNEXPECTED, "WebView parent is being updated reentrantly")
      })?;
      if parent != HWND::default() {
        SetFocus(Some(parent))?;
      }
    }

    Ok(())
  }

  unsafe fn cookie_from_win32(cookie: ICoreWebView2Cookie) -> Result<cookie::Cookie<'static>> {
    let mut name = PWSTR::null();
    cookie.Name(&mut name)?;
    let name = take_pwstr(name);
    if !Self::is_valid_cookie_name(&name) {
      return Err(Error::InvalidCookie);
    }

    let mut value = PWSTR::null();
    cookie.Value(&mut value)?;
    let value = take_pwstr(value);
    if value.contains('\0') {
      return Err(Error::InvalidCookie);
    }

    let mut cookie_builder = cookie::CookieBuilder::new(name, value);

    let mut domain = PWSTR::null();
    cookie.Domain(&mut domain)?;
    let domain = take_pwstr(domain);
    if domain.contains('\0') {
      return Err(Error::InvalidCookie);
    }
    cookie_builder = cookie_builder.domain(domain);

    let mut path = PWSTR::null();
    cookie.Path(&mut path)?;
    let path = take_pwstr(path);
    if path.contains('\0') {
      return Err(Error::InvalidCookie);
    }
    cookie_builder = cookie_builder.path(path);

    let mut http_only: BOOL = false.into();
    cookie.IsHttpOnly(&mut http_only)?;
    cookie_builder = cookie_builder.http_only(http_only.as_bool());

    let mut secure: BOOL = false.into();
    cookie.IsSecure(&mut secure)?;
    cookie_builder = cookie_builder.secure(secure.as_bool());

    let mut same_site = COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX;
    cookie.SameSite(&mut same_site)?;
    let same_site = match same_site {
      COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX => cookie::SameSite::Lax,
      COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT => cookie::SameSite::Strict,
      COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE => cookie::SameSite::None,
      _ => return Err(Error::InvalidCookie),
    };
    cookie_builder = cookie_builder.same_site(same_site);

    let mut is_session: BOOL = false.into();
    cookie.IsSession(&mut is_session)?;

    let mut expires = 0.0;
    cookie.Expires(&mut expires)?;

    let expires = match expires {
      _ if expires == -1.0 || is_session.as_bool() => Some(cookie::Expiration::Session),
      datetime
        if datetime.is_finite() && datetime >= i64::MIN as f64 && datetime <= i64::MAX as f64 =>
      {
        Some(cookie::Expiration::DateTime(
          cookie::time::OffsetDateTime::from_unix_timestamp(datetime as i64)
            .map_err(|_| Error::InvalidCookie)?,
        ))
      }
      _ => return Err(Error::InvalidCookie),
    };
    if let Some(expires) = expires {
      cookie_builder = cookie_builder.expires(expires);
    }

    Ok(cookie_builder.build())
  }

  unsafe fn cookie_into_win32(
    cookie_manager: &ICoreWebView2CookieManager,
    cookie: &cookie::Cookie<'_>,
  ) -> windows::core::Result<ICoreWebView2Cookie> {
    if !Self::is_valid_cookie_name(cookie.name())
      || cookie.value().contains('\0')
      || cookie.domain().is_some_and(|value| value.contains('\0'))
      || cookie.path().is_some_and(|value| value.contains('\0'))
    {
      return Err(windows::core::Error::from(E_INVALIDARG));
    }
    let name = HSTRING::from(cookie.name());
    let value = HSTRING::from(cookie.value());
    let domain = match cookie.domain() {
      Some(domain) => HSTRING::from(domain),
      None => HSTRING::new(),
    };
    let path = match cookie.path() {
      Some(path) => HSTRING::from(path),
      None => HSTRING::new(),
    };

    let win32_cookie = cookie_manager.CreateCookie(&name, &value, &domain, &path)?;

    let expires = if let Some(max_age) = cookie.max_age() {
      let expires_ = cookie::time::OffsetDateTime::now_utc()
        .saturating_add(max_age)
        .unix_timestamp();
      Some(expires_)
    } else {
      cookie.expires_datetime().map(|dt| dt.unix_timestamp())
    };
    if let Some(expires) = expires {
      win32_cookie.SetExpires(expires as f64)?;
    }

    if let Some(http_only) = cookie.http_only() {
      win32_cookie.SetIsHttpOnly(http_only)?;
    }

    if let Some(same_site) = cookie.same_site() {
      let same_site = match same_site {
        cookie::SameSite::Lax => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
        cookie::SameSite::Strict => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
        cookie::SameSite::None => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE,
      };
      win32_cookie.SetSameSite(same_site)?;
    }

    if let Some(secure) = cookie.secure() {
      win32_cookie.SetIsSecure(secure)?;
    }

    Ok(win32_cookie)
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

  pub fn cookies_for_url(&self, url: &str) -> Result<Vec<cookie::Cookie<'static>>> {
    let uri = HSTRING::from(url);
    self.cookies_inner(PCWSTR::from_raw(uri.as_ptr()))
  }

  pub fn cookies(&self) -> Result<Vec<cookie::Cookie<'static>>> {
    self.cookies_inner(PCWSTR::null())
  }

  fn cookies_inner(&self, uri: PCWSTR) -> Result<Vec<cookie::Cookie<'static>>> {
    let (tx, rx) = mpsc::channel();

    let webview = self.webview.cast::<ICoreWebView2_2>()?;
    unsafe {
      webview.CookieManager()?.GetCookies(
        uri,
        // we don't use GetCookiesCompletedHandler::wait_for_async
        // as it uses an mspc::channel under the hood, so we can avoid using two channels
        // by manually creating the callback handler and use webview2_com::with_with_bump
        &GetCookiesCompletedHandler::create(Box::new(move |error_code, cookies| {
          let result = (move || {
            error_code?;

            let cookies = if let Some(cookies) = cookies {
              let mut count = 0;
              cookies.Count(&mut count)?;

              let mut out = Vec::with_capacity(count as _);

              for idx in 0..count {
                let cookie = cookies.GetValueAtIndex(idx)?;

                if let Ok(cookie) = Self::cookie_from_win32(cookie) {
                  out.push(cookie)
                }
              }

              out
            } else {
              Vec::new()
            };
            Ok(cookies)
          })();

          tx.send(result)
            .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
        })),
      )?;
    }

    webview2_com::wait_with_pump(rx)?
  }

  pub fn set_cookie(&self, cookie: &cookie::Cookie<'_>) -> Result<()> {
    let webview = self.webview.cast::<ICoreWebView2_2>()?;
    unsafe {
      let cookie_manager = webview.CookieManager()?;
      let cookie = Self::cookie_into_win32(&cookie_manager, cookie)?;
      cookie_manager.AddOrUpdateCookie(&cookie)?;
    }
    Ok(())
  }

  pub fn delete_cookie(&self, cookie: &cookie::Cookie<'_>) -> Result<()> {
    let webview = self.webview.cast::<ICoreWebView2_2>()?;
    unsafe {
      let cookie_manager = webview.CookieManager()?;
      let cookie = Self::cookie_into_win32(&cookie_manager, cookie)?;
      cookie_manager.DeleteCookie(&cookie)?;
    }
    Ok(())
  }

  pub fn reparent(&self, parent: isize) -> Result<()> {
    let parent = HWND(parent as _);

    unsafe {
      // Acquire both mutation capabilities before changing USER32 state. A
      // nested callback may attempt another reparent, but it receives a
      // fallible error instead of triggering RefCell's panic path.
      let mut current_parent = self.parent.try_borrow_mut().map_err(|_| {
        windows::core::Error::new(E_UNEXPECTED, "WebView parent is being updated reentrantly")
      })?;
      let old_parent = *current_parent;
      if old_parent == parent {
        return Ok(());
      }

      if self.is_child {
        SetParent(self.hwnd, Some(parent))?;
        *current_parent = parent;
      } else {
        let mut cleanup_slot = self.cleanup.try_borrow_mut().map_err(|_| {
          windows::core::Error::new(E_UNEXPECTED, "WebView cleanup is running reentrantly")
        })?;
        let Some(cleanup) = cleanup_slot.as_mut() else {
          return Err(windows::core::Error::from_hresult(E_UNEXPECTED).into());
        };

        SetParent(self.hwnd, Some(parent))?;
        let registration = match Self::attach_parent_subclass(parent, &self.controller) {
          Ok(registration) => registration,
          Err(error) => {
            let _ = SetParent(self.hwnd, Some(old_parent));
            return Err(error.into());
          }
        };
        cleanup.retain_parent_subclass(registration);
        *current_parent = parent;
        if let Err(code) = cleanup.detach_parent_subclass(old_parent) {
          return Err(windows::core::Error::from_hresult(HRESULT(code)).into());
        }

        let parent_bounds = Self::parent_bounds(parent)?;

        self.set_bounds_inner(parent_bounds, (0, 0).into())?;
      }
    }

    Ok(())
  }

  pub fn print(&self) -> Result<()> {
    // The embedder's trusted print command must not depend on the same page
    // primitive that hostile content can call. WebView2 has no event for
    // cancelling page-initiated scripted printing, so embedders may lock
    // `window.print` and `document.execCommand("print")` at document creation
    // while retaining this native command.
    unsafe {
      self
        .webview
        .cast::<ICoreWebView2_16>()?
        .ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER)?;
    }
    Ok(())
  }

  pub fn clear_all_browsing_data(&self) -> Result<()> {
    unsafe {
      self
        .webview
        .cast::<ICoreWebView2_13>()?
        .Profile()?
        .cast::<ICoreWebView2Profile2>()?
        .ClearBrowsingDataAll(&ClearBrowsingDataCompletedHandler::create(Box::new(
          move |_| Ok(()),
        )))
        .map_err(Into::into)
    }
  }

  pub fn set_theme(&self, theme: Theme) -> Result<()> {
    unsafe { set_theme(&self.webview, theme) }
  }

  pub fn set_background_color(&self, background_color: RGBA) -> Result<()> {
    unsafe { set_background_color(&self.controller, background_color) }
  }

  pub fn set_memory_usage_level(&self, level: MemoryUsageLevel) -> Result<()> {
    let webview = self.webview.cast::<ICoreWebView2_19>()?;
    // https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2memoryusagetargetlevel
    let level = match level {
      MemoryUsageLevel::Normal => 0,
      MemoryUsageLevel::Low => 1,
    };
    let level = COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL(level);
    unsafe { webview.SetMemoryUsageTargetLevel(level).map_err(Into::into) }
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn open_devtools(&self) {
    let _ = unsafe { self.webview.OpenDevToolsWindow() };
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn close_devtools(&self) {}

  #[cfg(any(debug_assertions, feature = "devtools"))]
  pub fn is_devtools_open(&self) -> bool {
    false
  }
}

/// The scrollbar style to use in the webview.
#[derive(Clone, Copy, Default)]
pub enum ScrollBarStyle {
  #[default]
  /// The browser default scrollbar style.
  Default,

  /// Fluent UI style overlay scrollbars.
  FluentOverlay,
}

#[inline]
fn load_url_with_headers(
  webview: &ICoreWebView2,
  env: &ICoreWebView2Environment,
  url: &str,
  headers: http::HeaderMap,
) -> Result<()> {
  let url = HSTRING::from(url);

  let headers_map = {
    let mut headers_map = String::new();
    for (name, value) in headers.iter() {
      let header_key = name.to_string();
      if let Ok(value) = value.to_str() {
        let _ = writeln!(headers_map, "{}: {}", header_key, value);
      }
    }
    HSTRING::from(headers_map)
  };

  unsafe {
    let env = env.cast::<ICoreWebView2Environment9>()?;
    let method = HSTRING::from("GET");
    if let Ok(request) = env.CreateWebResourceRequest(&url, &method, None, &headers_map) {
      let webview: ICoreWebView2_10 = webview.cast()?;
      webview.NavigateWithWebResourceRequest(&request)?;
    }
  };

  Ok(())
}

#[inline]
unsafe fn set_background_color(
  controller: &ICoreWebView2Controller,
  background_color: RGBA,
) -> Result<()> {
  let (r, g, b, mut a) = background_color;
  if is_windows_7() || a != 0 {
    a = 255;
  }

  let controller2: ICoreWebView2Controller2 = controller.cast()?;
  controller2
    .SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
      R: r,
      G: g,
      B: b,
      A: a,
    })
    .map_err(Into::into)
}

#[inline]
unsafe fn set_theme(webview: &ICoreWebView2, theme: Theme) -> Result<()> {
  let webview = webview.cast::<ICoreWebView2_13>()?;
  let profile = webview.Profile()?;
  profile
    .SetPreferredColorScheme(match theme {
      Theme::Dark => COREWEBVIEW2_PREFERRED_COLOR_SCHEME_DARK,
      Theme::Light => COREWEBVIEW2_PREFERRED_COLOR_SCHEME_LIGHT,
      Theme::Auto => COREWEBVIEW2_PREFERRED_COLOR_SCHEME_AUTO,
    })
    .map_err(Into::into)
}

pub fn platform_webview_version() -> Result<String> {
  let mut versioninfo = PWSTR::null();
  unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut versioninfo) }?;
  take_pwstr_bounded(versioninfo, WEBVIEW2_VERSION_LIMIT)
    .ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG).into())
}

#[inline]
fn is_windows_7() -> bool {
  let v = windows_version::OsVersion::current();
  // windows 7 is 6.1
  v.major == 6 && v.minor == 1
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn stopped_navigation_does_not_destroy_a_download_destination_context() {
    for status in [
      COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
      COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED,
    ] {
      assert_eq!(
        navigation_completion_phase(false, status),
        NavigationEventPhase::Cancelled
      );
    }
    for status in [
      COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID,
      COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED,
      COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET,
    ] {
      assert_eq!(
        navigation_completion_phase(false, status),
        NavigationEventPhase::Failed
      );
    }
    assert_eq!(
      navigation_completion_phase(true, COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN),
      NavigationEventPhase::Finished
    );
  }

  #[test]
  fn public_error_is_send_and_sync_for_tauri_runtime_propagation() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Error>();
  }

  #[test]
  fn extension_startup_refusal_is_complete_and_path_precedes_enablement() {
    assert_eq!(extension_startup_refusal(false, false, false), None);
    assert_eq!(
      extension_startup_refusal(false, true, false),
      Some(WebView2ExtensionStartupRefusal::StartupFence)
    );
    assert_eq!(extension_startup_refusal(false, true, true), None);
    assert_eq!(
      extension_startup_refusal(true, false, false),
      Some(WebView2ExtensionStartupRefusal::ExtensionPath)
    );
    assert_eq!(
      extension_startup_refusal(true, true, true),
      Some(WebView2ExtensionStartupRefusal::ExtensionPath),
      "a configured unmanaged path is the first reported refusal"
    );
  }

  #[test]
  fn cleanup_incident_registry_has_an_exact_fail_closed_bound() {
    assert!(cleanup_registry_has_capacity(0));
    assert!(cleanup_registry_has_capacity(
      MAX_ORPHANED_CLEANUP_DEBTS - 1
    ));
    assert!(!cleanup_registry_has_capacity(MAX_ORPHANED_CLEANUP_DEBTS));
    assert!(!cleanup_registry_has_capacity(usize::MAX));
  }

  #[test]
  fn ui_dispatch_registry_has_an_exact_fail_closed_bound() {
    assert!(dispatch_registry_has_capacity(0));
    assert!(dispatch_registry_has_capacity(PENDING_DISPATCH_LIMIT - 1));
    assert!(!dispatch_registry_has_capacity(PENDING_DISPATCH_LIMIT));
    assert!(!dispatch_registry_has_capacity(usize::MAX));
  }

  #[test]
  fn navigation_identity_urls_are_bounded_and_redirects_replace_in_place() {
    let mut urls = InFlightNavigationUrls::default();
    for id in 0..IN_FLIGHT_NAVIGATION_LIMIT as u64 {
      assert!(urls.admit(id, format!("https://{id}.example/")));
    }
    assert!(!urls.admit(
      IN_FLIGHT_NAVIGATION_LIMIT as u64,
      "https://overflow.example/".into()
    ));
    assert!(urls.admit(0, "https://redirect.example/".into()));
    assert_eq!(urls.commit(0).as_deref(), Some("https://redirect.example/"));
    assert_eq!(
      urls.commit(0),
      None,
      "one navigation gates presentation once"
    );
    assert_eq!(urls.finish(0).as_deref(), Some("https://redirect.example/"));
    assert!(urls.admit(
      IN_FLIGHT_NAVIGATION_LIMIT as u64,
      "https://reused.example/".into()
    ));
  }

  #[test]
  fn popup_no_callback_path_denies_before_metadata_or_deferral() {
    let source = include_str!("mod.rs");
    let handler = source
      .split("webview.add_NewWindowRequested(")
      .nth(1)
      .expect("new-window event registration")
      .split("Self::attach_main_thread_dispatcher(hwnd)?")
      .next()
      .expect("new-window event body");
    let deny = handler
      .find("args.SetHandled(true)?")
      .expect("dominant denial");
    let callback = handler
      .find("if let Some(new_window_req_handler)")
      .expect("optional metadata callback");
    let metadata = handler.find("args.Uri(&mut uri)?").expect("URI read");
    let deferral = handler.find("args.GetDeferral()?").expect("deferral read");
    assert!(deny < callback);
    assert!(callback < metadata);
    assert!(metadata < deferral);
    assert_eq!(handler.matches("args.GetDeferral()?").count(), 1);
  }

  #[test]
  fn custom_protocol_timeout_response_is_fail_closed() {
    let response = web_resource_failure_response(StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert!(response.body().is_empty());
  }

  #[test]
  fn custom_protocol_overflow_response_is_empty_and_fail_closed() {
    let response = web_resource_failure_response(CUSTOM_PROTOCOL_OVERFLOW_STATUS);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.body().is_empty());
  }

  #[test]
  fn custom_protocol_stream_read_accepts_the_exact_buffer_bound() {
    assert_eq!(bounded_stream_read_len(1_024, 1_024), Some(1_024));
  }

  #[test]
  fn custom_protocol_stream_read_rejects_a_native_over_bound_count() {
    assert_eq!(bounded_stream_read_len(1_025, 1_024), None);
  }
}
