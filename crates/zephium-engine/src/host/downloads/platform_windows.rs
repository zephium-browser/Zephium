//! UI-apartment WebView2 transport. The shared coordinator owns history,
//! staging decisions and recovery; this adapter never replays a request URL.
use super::*;
#[path = "windows_timer.rs"]
mod timer;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::StateChangedEventHandler;
use windows::core::{Interface, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IShellItem, SHCreateItemFromParsingName,
    SHOpenFolderAndSelectItems, SHParseDisplayName, ShellExecuteW, FOS_DONTADDTORECENT,
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetWindowThreadProcessId, IsChild, IsWindow, IsWindowVisible,
    GA_ROOT, GA_ROOTOWNER, SW_SHOWNORMAL,
};

#[derive(Clone)]
pub(super) struct Native {
    operation: ICoreWebView2DownloadOperation,
    pub(super) permit: EventPermit,
}
pub(super) struct Delegate {
    operation: ICoreWebView2DownloadOperation,
    token: i64,
}
impl Drop for Delegate {
    fn drop(&mut self) {
        unsafe {
            let _ = self.operation.remove_StateChanged(self.token);
        }
    }
}
pub(super) type Timer = timer::DownloadTimer;
pub(super) type SavePanel = IFileDialog;
pub(super) type DirectoryPanel = IFileDialog;
thread_local! { static DIALOG_ACTIVE: Cell<bool> = const { Cell::new(false) }; }
pub(super) struct DialogLease;
impl DialogLease {
    fn acquire() -> Option<Self> {
        DIALOG_ACTIVE.with(|value| (!value.replace(true)).then(|| Self))
    }
}
impl Drop for DialogLease {
    fn drop(&mut self) {
        DIALOG_ACTIVE.with(|value| value.set(false));
    }
}

pub(super) struct Source {
    permit: EventPermit,
    surface_intent: Arc<AtomicBool>,
    navigation: NavigationEpochTracker,
    activity: NavigationActivity,
    controller: ICoreWebView2Controller,
    parent: HWND,
    root: HWND,
    initial_open: bool,
    ask_destination: bool,
}
impl Source {
    /// The page that started the download is still the one in the view,
    /// whether or not the person is looking at it.
    pub(super) fn current(&self) -> bool {
        self.permit.active_token().is_some() && self.navigation.matches_activity(self.activity)
    }

    pub(super) fn live(&self) -> bool {
        let mut parent = HWND::default();
        self.permit.active_token().is_some()
            && (self.initial_open || self.surface_intent.load(Ordering::Acquire))
            && self.navigation.matches_activity(self.activity)
            && unsafe { self.controller.ParentWindow(&mut parent) }.is_ok()
            && parent == self.parent
            && unsafe {
                IsWindow(Some(self.root)).as_bool() && IsWindowVisible(self.root).as_bool()
            }
            && (self.parent == self.root || unsafe { IsChild(self.root, self.parent).as_bool() })
    }
    fn key(&self) -> bool {
        self.live()
            && unsafe { IsWindowEnabled(self.root).as_bool() }
            && fronts(self.root, unsafe { GetForegroundWindow() })
    }
}

/// The window is in front, or a popup it owns is: WebView2's own permission
/// prompt takes the foreground while the page's downloads wait on it.
fn fronts(root: HWND, foreground: HWND) -> bool {
    !foreground.is_invalid()
        && (foreground == root || unsafe { GetAncestor(foreground, GA_ROOTOWNER) } == root)
}

fn wide(path: &Path) -> Result<Vec<u16>, DownloadError> {
    let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
    if value.is_empty() || value.len() > 32766 || value.contains(&0) {
        return Err(DownloadError::Destination);
    }
    value.push(0);
    Ok(value)
}
struct TaskMemory(PWSTR);
impl Drop for TaskMemory {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0 .0.cast()));
        }
    }
}
fn take_string(value: PWSTR, limit: usize) -> Option<String> {
    let _allocation = TaskMemory(value);
    if value.is_null() {
        return None;
    }
    for len in 0..=limit {
        // COM's string contract supplies a terminated allocation; never retain
        // more than the platform boundary allows in the Rust/UI projections.
        if unsafe { *value.0.add(len) } == 0 {
            let text =
                String::from_utf16(unsafe { std::slice::from_raw_parts(value.0, len) }).ok()?;
            return (text.len() <= limit).then_some(text);
        }
    }
    None
}
fn operation_uri(operation: &ICoreWebView2DownloadOperation) -> Option<String> {
    let mut text = PWSTR::null();
    unsafe { operation.Uri(&mut text) }.ok()?;
    // A data URL may contain the entire file. Only inspect its scheme; do
    // not copy payload bytes into history, diagnostics or UI allocations.
    if !text.is_null()
        && (0..5).all(|index| unsafe { *text.0.add(index) } == u16::from(b"data:"[index]))
    {
        drop(TaskMemory(text));
        return Some("data:".into());
    }
    take_string(text, zephium_core::navigation::MAX_URL_BYTES)
}
fn current_origin(controller: &ICoreWebView2Controller) -> Option<PageOrigin> {
    let core = unsafe { controller.CoreWebView2() }.ok()?;
    let mut value = PWSTR::null();
    unsafe { core.Source(&mut value) }.ok()?;
    let url = url::Url::parse(&take_string(
        value,
        zephium_core::navigation::MAX_URL_BYTES,
    )?)
    .ok()?;
    PageOrigin::from_url(&url).ok()
}
fn source_origin(uri: &str, controller: &ICoreWebView2Controller) -> Option<PageOrigin> {
    if zephium_core::navigation::is_allowed_str(uri) {
        return PageOrigin::from_url(&url::Url::parse(uri).ok()?).ok();
    }
    if let Some(blob) = uri.strip_prefix("blob:") {
        if zephium_core::navigation::is_allowed_str(blob) {
            return PageOrigin::from_url(&url::Url::parse(blob).ok()?).ok();
        }
    }
    if uri.starts_with("data:") {
        return current_origin(controller);
    }
    None
}
pub(super) fn progress(native: &Native) -> Option<(u64, Option<u64>)> {
    let mut received = 0;
    let mut total = 0;
    unsafe { native.operation.BytesReceived(&mut received) }.ok()?;
    unsafe { native.operation.TotalBytesToReceive(&mut total) }.ok()?;
    Some((
        received.max(0) as u64,
        u64::try_from(total).ok().filter(|value| *value > 0),
    ))
}
pub(super) fn stop_timer(timer: Timer) {
    drop(timer);
}
pub(super) fn cancel_directory(panel: &DirectoryPanel) {
    unsafe {
        let _ = panel.Close(HRESULT::from_win32(ERROR_CANCELLED.0));
    }
}
pub(super) fn reveal(path: &Path) -> Result<(), DownloadError> {
    let path = shell_path(path)?;
    let mut item = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(path.as_ptr()), None, &mut item, 0, None) }
        .map_err(|_| DownloadError::Unavailable)?;
    let result = unsafe { SHOpenFolderAndSelectItems(item, None, 0) };
    unsafe {
        CoTaskMemFree(Some(item.cast()));
    }
    result.map_err(|_| DownloadError::Unavailable)
}
pub(super) fn open(path: &Path) -> Result<(), DownloadError> {
    let path = shell_path(path)?;
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(windows::core::w!("open").as_ptr()),
            PCWSTR(path.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(DownloadError::Unavailable)
    }
}
fn shell_path(path: &Path) -> Result<Vec<u16>, DownloadError> {
    // Explorer and shell verbs do not share the filesystem APIs' support for
    // verbatim paths. Keep the recorded canonical path as authority and only
    // project its strictly validated ordinary spelling at this boundary.
    let ordinary = crate::platform::imp::webview2_download_path(path)
        .map_err(|_| DownloadError::Destination)?;
    wide(&ordinary)
}
fn dialog(
    folder: bool,
    title: &str,
    directory: Option<&Path>,
    filename: Option<&str>,
) -> Result<IFileDialog, DownloadError> {
    let dialog: IFileDialog = unsafe {
        CoCreateInstance(
            if folder {
                &FileOpenDialog
            } else {
                &FileSaveDialog
            },
            None,
            CLSCTX_INPROC_SERVER,
        )
    }
    .map_err(|_| DownloadError::Unavailable)?;
    unsafe {
        dialog
            .SetOptions(
                FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_DONTADDTORECENT
                    | if folder {
                        FOS_PICKFOLDERS
                    } else {
                        Default::default()
                    },
            )
            .map_err(|_| DownloadError::Unavailable)?;
        dialog
            .SetTitle(&HSTRING::from(title))
            .map_err(|_| DownloadError::Unavailable)?;
        if let Some(filename) = filename {
            dialog
                .SetFileName(&HSTRING::from(filename))
                .map_err(|_| DownloadError::Destination)?;
        }
        if let Some(directory) = directory {
            let path = wide(directory)?;
            if let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(path.as_ptr()), None)
            {
                let _ = dialog.SetFolder(&item);
            }
        }
    }
    Ok(dialog)
}
fn selected(dialog: &IFileDialog) -> Option<PathBuf> {
    let item = unsafe { dialog.GetResult() }.ok()?;
    let path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.ok()?;
    take_string(path, 4096).map(PathBuf::from)
}
fn foreground_window() -> Option<HWND> {
    let window = unsafe { GetForegroundWindow() };
    let mut process = 0;
    unsafe {
        GetWindowThreadProcessId(window, Some(&mut process));
    }
    (process == unsafe { GetCurrentProcessId() }
        && unsafe { IsWindowVisible(window).as_bool() && IsWindowEnabled(window).as_bool() })
    .then_some(window)
}
fn interrupt_error(reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON) -> DownloadError {
    match reason {
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_FAILED => DownloadError::Destination,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_LARGE => DownloadError::FileTooLarge,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TRANSIENT_ERROR => DownloadError::FileBusy,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NO_SPACE => DownloadError::DiskFull,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_ACCESS_DENIED => DownloadError::Permission,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NAME_TOO_LONG => DownloadError::Destination,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS => DownloadError::Protection,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED => {
            DownloadError::ConnectionLost
        }
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT => DownloadError::Timeout,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN => DownloadError::Authentication,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CERTIFICATE_PROBLEM => {
            DownloadError::Certificate
        }
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_INVALID_REQUEST
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FAILED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_BAD_CONTENT
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNEXPECTED_RESPONSE
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CROSS_ORIGIN_REDIRECT => {
            DownloadError::Server
        }
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH => {
            DownloadError::Integrity
        }
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_DOWNLOAD_PROCESS_CRASHED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_SHUTDOWN => DownloadError::Runtime,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED => DownloadError::Cancelled,
        _ => DownloadError::Network,
    }
}
fn resumable_reason(reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON) -> bool {
    matches!(
        reason,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_FAILED
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_SERVER_DOWN
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TRANSIENT_ERROR
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH
    )
}

pub(super) fn completion_bytes(native: &Native) -> Result<Option<u64>, DownloadError> {
    let mut received = 0;
    // Native BytesReceived counts bytes written to its file, unlike the
    // HTTP-derived total, which may describe an encoded response body.
    unsafe { native.operation.BytesReceived(&mut received) }.map_err(|_| DownloadError::Runtime)?;
    u64::try_from(received)
        .map(Some)
        .map_err(|_| DownloadError::Integrity)
}
fn automatically_restarting(reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON) -> bool {
    matches!(
        reason,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_HASH_MISMATCH
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_SHORT
    )
}

const ORPHANED_INTERRUPT_GRACE: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OperationSnapshot {
    state: COREWEBVIEW2_DOWNLOAD_STATE,
    reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON,
    can_resume: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CancelSettlement {
    Wait,
    NativeInterrupted,
    ForceCancelled,
    AbandonUnresolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InitialInterruptState {
    None,
    Interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdmissionDisposition {
    Admit,
    SuppressFollowup,
    Refuse(DownloadError),
}

fn classify_admission(
    initial_interrupt: InitialInterruptState,
    at_capacity: bool,
    source_intent_ok: bool,
) -> AdmissionDisposition {
    match initial_interrupt {
        InitialInterruptState::Interrupted(_) | InitialInterruptState::Unknown => {
            AdmissionDisposition::SuppressFollowup
        }
        InitialInterruptState::None if at_capacity => {
            AdmissionDisposition::Refuse(DownloadError::Capacity)
        }
        InitialInterruptState::None if !source_intent_ok => {
            AdmissionDisposition::Refuse(DownloadError::Unavailable)
        }
        InitialInterruptState::None => AdmissionDisposition::Admit,
    }
}

fn should_bypass_orphaned_grace(matched_id: Option<DownloadId>, id: DownloadId) -> bool {
    matched_id == Some(id)
}

fn operation_identity(operation: &ICoreWebView2DownloadOperation) -> Option<usize> {
    operation
        .cast::<windows::core::IUnknown>()
        .ok()
        .map(|unknown| unknown.as_raw() as usize)
}

fn initial_interrupt_state(operation: &ICoreWebView2DownloadOperation) -> InitialInterruptState {
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
    // SAFETY: `operation` is a live WebView2 COM interface accessed on its owning
    // UI STA thread, and `reason` is a valid stack out-pointer for the call.
    match unsafe { operation.InterruptReason(&mut reason) } {
        Ok(()) if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE => {
            InitialInterruptState::None
        }
        Ok(()) => InitialInterruptState::Interrupted(reason),
        Err(_) => InitialInterruptState::Unknown,
    }
}

fn operation_snapshot(operation: &ICoreWebView2DownloadOperation) -> Option<OperationSnapshot> {
    let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
    // SAFETY: `operation` is a live WebView2 COM interface accessed on its owning
    // UI STA thread, and `state` / `reason` are valid stack out-pointers.
    unsafe {
        operation
            .State(&mut state)
            .and_then(|_| operation.InterruptReason(&mut reason))
    }
    .ok()?;
    let can_resume =
        if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED && automatically_restarting(reason) {
            let mut resumable = windows::core::BOOL::default();
            // SAFETY: `operation` is live on the UI STA thread and `resumable` is a valid stack out-pointer.
            unsafe { operation.CanResume(&mut resumable) }
                .map(|_| resumable.as_bool())
                // Fail closed if `CanResume` errors so an auto-restarting operation waits for writer release or deadline.
                .unwrap_or(true)
        } else {
            false
        };
    Some(OperationSnapshot {
        state,
        reason,
        can_resume,
    })
}

fn cancel_settlement(
    snapshot: Option<OperationSnapshot>,
    write_released: bool,
    deadline_expired: bool,
    cancel_failed: bool,
) -> CancelSettlement {
    match snapshot {
        Some(snapshot) if snapshot.state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED => {
            if automatically_restarting(snapshot.reason) {
                if write_released && !snapshot.can_resume {
                    CancelSettlement::NativeInterrupted
                } else if deadline_expired {
                    CancelSettlement::AbandonUnresolved
                } else {
                    CancelSettlement::Wait
                }
            } else {
                CancelSettlement::NativeInterrupted
            }
        }
        Some(snapshot) if snapshot.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED => {
            CancelSettlement::ForceCancelled
        }
        Some(snapshot) if snapshot.state == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS => {
            if write_released
                && (cancel_failed
                    || deadline_expired
                    || snapshot.reason != COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE)
            {
                CancelSettlement::ForceCancelled
            } else if deadline_expired {
                CancelSettlement::AbandonUnresolved
            } else {
                CancelSettlement::Wait
            }
        }
        None => {
            if write_released {
                CancelSettlement::ForceCancelled
            } else if deadline_expired {
                CancelSettlement::AbandonUnresolved
            } else {
                CancelSettlement::Wait
            }
        }
        Some(_) => {
            if deadline_expired {
                CancelSettlement::AbandonUnresolved
            } else {
                CancelSettlement::Wait
            }
        }
    }
}

fn orphaned_interrupt_error(
    state: COREWEBVIEW2_DOWNLOAD_STATE,
    reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON,
    write_released: bool,
) -> Option<DownloadError> {
    if state == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS
        && reason != COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE
        && !automatically_restarting(reason)
        && write_released
    {
        Some(interrupt_error(reason))
    } else {
        None
    }
}

impl Downloads {
    pub(super) fn ensure_timer(self: &Rc<Self>) {
        let (mut period, slow) = self.tick_period();
        if slow {
            period = self.waiting_period();
        }
        let due = std::time::Instant::now() + period;
        if self.timer.borrow().is_some() {
            if self.timer_slow.get() == slow
                && self.timer_due.get().is_some_and(|current| current <= due)
            {
                return;
            }
            if let Some(timer) = self.timer.borrow_mut().take() {
                stop_timer(timer);
            }
        }
        self.timer_slow.set(slow);
        self.timer_due.set(Some(due));
        let weak = Rc::downgrade(self);
        let timer = timer::schedule(period, move || {
            if let Some(manager) = weak.upgrade() {
                manager.timer.borrow_mut().take();
                manager.tick();
            }
        });
        if timer.is_none() {
            self.persistence_failed.set(true);
        }
        *self.timer.borrow_mut() = timer;
    }

    pub(in crate::host) fn admit(
        self: &Rc<Self>,
        partition: Partition,
        permit: EventPermit,
        surface_intent: Arc<AtomicBool>,
        navigation: NavigationEpochTracker,
        request: (
            &ICoreWebView2Controller,
            &ICoreWebView2DownloadStartingEventArgs,
            Option<super::SiteAsk>,
        ),
        mut initial: Option<InitialDownload>,
    ) {
        // Only a download the person started from the browser's own menu
        // comes without a site to ask; it asks where to save instead.
        let (controller, args, ask) = request;
        let ask_destination = ask.is_none();
        let initial_open = initial.is_some();
        if self.stopping.get()
            || self.retired.borrow().contains(&partition.profile())
            || permit.active_token().is_none()
        {
            return;
        }
        // Check follow-up auto-restart events before capacity/intent refusal
        // branches so suppressed restarts never record spurious refused rows.
        // SAFETY: `args` is a live WebView2 DownloadStartingEventArgs interface on the UI STA thread.
        let Ok(operation) = (unsafe { args.DownloadOperation() }) else {
            return;
        };
        let initial_interrupt = initial_interrupt_state(&operation);
        let at_capacity = self.active.borrow().len() >= MAX_ACTIVE_DOWNLOADS
            || self.work.get() >= MAX_BACKGROUND_WORK
            || !self.private_cleanup_capacity(partition);
        let source_intent_ok = initial_open || surface_intent.load(Ordering::Acquire);
        match classify_admission(initial_interrupt, at_capacity, source_intent_ok) {
            AdmissionDisposition::SuppressFollowup => {
                self.suppress_followup_admission(args, &operation);
                return;
            }
            AdmissionDisposition::Refuse(error) => {
                self.refused(partition, &navigation, error);
                return;
            }
            AdmissionDisposition::Admit => {}
        }
        self.ensure_timer();
        if self.timer.borrow().is_none() {
            return;
        }
        let Some(activity) = navigation.activity_snapshot() else {
            return;
        };
        let mut parent = HWND::default();
        if unsafe { controller.ParentWindow(&mut parent) }.is_err() {
            return;
        }
        let source = Source {
            permit: permit.clone(),
            surface_intent,
            navigation: navigation.clone(),
            activity,
            controller: controller.clone(),
            parent,
            root: unsafe { GetAncestor(parent, GA_ROOT) },
            initial_open,
            ask_destination,
        };
        if !source.live() {
            self.refused(partition, &source.navigation, DownloadError::Unavailable);
            return;
        }
        let Some(uri) = operation_uri(&operation) else {
            return;
        };
        let Some(origin) = source_origin(&uri, controller) else {
            return;
        };
        // DownloadStarting says neither who clicked nor which frame asked, so
        // the site's own decision is what guards a download nobody started.
        let site = match self.site_admission(partition.profile(), ask, &origin, true) {
            super::site::SiteAdmission::Refuse => return,
            super::site::SiteAdmission::Open => None,
            super::site::SiteAdmission::Hold(hold) => Some(hold),
        };
        let insecure = url::Url::parse(&uri).is_ok_and(|url| is_insecure_source(&url));
        let Ok(core) = (unsafe { controller.CoreWebView2() }) else {
            return;
        };
        let mut process = 0;
        if unsafe { core.BrowserProcessId(&mut process) }.is_err() {
            return;
        }
        let Ok(writer) = super::super::download_files::writer_for_process(process) else {
            return;
        };
        let Ok(deferral) = (unsafe { args.GetDeferral() }) else {
            return;
        };
        let id = DownloadId::generate();
        let admission_manager = Rc::downgrade(self);
        let event = args.clone();
        let reply = DestinationReply::new(move |path| {
            let accepted = path.and_then(|path| wide(&path).ok()).is_some_and(|path| {
                unsafe {
                    event
                        .SetResultFilePath(PCWSTR(path.as_ptr()))
                        .and_then(|_| event.SetCancel(false))
                }
                .is_ok()
            });
            if !accepted {
                unsafe {
                    let _ = event.SetCancel(true);
                }
            }
            unsafe {
                let _ = deferral.Complete();
            }
            if !accepted {
                if let Some(manager) = admission_manager.upgrade() {
                    manager.native_failed_with_error(id, DownloadError::Destination);
                }
            }
        });
        let weak = Rc::downgrade(self);
        let mut token = 0;
        let handler = StateChangedEventHandler::create(Box::new(move |operation, _| {
            let (Some(manager), Some(operation)) = (weak.upgrade(), operation) else {
                return Ok(());
            };
            let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
            unsafe { operation.State(&mut state) }?;
            if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                manager.native_finished(id);
            } else if state == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
                manager.note_native_in_progress(id);
            } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
                unsafe { operation.InterruptReason(&mut reason) }?;
                if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
                    // The pause the person asked for. One nobody asked for
                    // falls through and ends the transfer as any other stop.
                    if manager.user_paused(id) {
                        return Ok(());
                    }
                }
                if automatically_restarting(reason) {
                    if manager
                        .active
                        .borrow()
                        .get(&id)
                        .is_some_and(|transfer| transfer.cancelling)
                    {
                        // SAFETY: `operation` is a live WebView2 COM interface accessed on its owning UI STA.
                        let cancel_failed = unsafe { operation.Cancel() }.is_err();
                        manager.note_native_error(id, interrupt_error(reason));
                        let timer_armed = manager.arm_timer();
                        let native = manager
                            .active
                            .borrow()
                            .get(&id)
                            .map(|transfer| transfer.native.clone());
                        if let Some(native) = native {
                            manager.settle_cancelling_transfer(
                                id,
                                &native,
                                cancel_failed,
                                !timer_armed,
                            );
                        }
                    }
                    return Ok(());
                }
                let mut can_resume = windows::core::BOOL::default();
                if resumable_reason(reason)
                    && unsafe { operation.CanResume(&mut can_resume) }.is_ok()
                    && can_resume.as_bool()
                    && manager.pause(id, interrupt_error(reason))
                {
                    return Ok(());
                }
                // Preserve the original reason before Cancel can pump a
                // second StateChanged event reporting USER_CANCELED.
                manager.note_native_error(id, interrupt_error(reason));
                unsafe {
                    let _ = operation.Cancel();
                }
                manager.native_failed_with_error(id, interrupt_error(reason));
            }
            Ok(())
        }));
        if unsafe { operation.add_StateChanged(&handler, &mut token) }.is_err() {
            return;
        }
        let mut suggested = PWSTR::null();
        let filename = if unsafe { args.ResultFilePath(&mut suggested) }.is_ok() {
            take_string(suggested, 4096)
                .map(|name| safe_filename(&name))
                .unwrap_or_else(|| "download".into())
        } else {
            "download".into()
        };
        let mut total = 0;
        let total = unsafe { operation.TotalBytesToReceive(&mut total) }
            .ok()
            .and_then(|_| u64::try_from(total).ok());
        let record = DownloadRecord {
            id,
            session: self.session,
            revision: 1,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_secs() as i64),
            filename,
            source: origin.as_str().into(),
            source_is_context: uri == "data:",
            state: DownloadState::Pending,
            received: 0,
            total,
            error: None,
            destination: None,
            staging: None,
            staging_identity: None,
            identity: None,
            writer: Some(writer),
            writer_released: false,
            insecure,
        };
        self.active.borrow_mut().insert(
            id,
            Transfer {
                partition,
                record,
                native: Native {
                    operation: operation.clone(),
                    permit,
                },
                _delegate: Delegate { operation, token },
                source: Some(source),
                destination_reply: Some(reply),
                destination: None,
                panel: None,
                panel_lease: None,
                picker_preferences: None,
                on_started: initial
                    .as_mut()
                    .and_then(|context| context.on_started.take()),
                authorized: false,
                cancelling: false,
                persisting_terminal: false,
                kept: false,
                user_pausing: false,
                site,
                confirmed: false,
                orphaned_since: None,
                write_probe_in_flight: false,
                write_probe_generation: 0,
                write_release_observed: None,
                deadline: DecisionDeadline::new(DecisionPhase::Admission, Instant::now()),
            },
        );
        self.changed(partition.profile());
        self.ensure_recovery(partition);
        let cached = self.preferences.borrow().get(&partition.profile()).cloned();
        if let Some(preferences) = cached {
            self.decide(id, preferences);
        } else if matches!(partition, Partition::Ephemeral(_)) {
            self.decide(id, DownloadPreferences::default());
        } else {
            self.work.set(self.work.get() + 1);
            let sender = self.sender.clone();
            if !self.store.download_call(
                partition.profile(),
                DownloadStoreCall::Preferences,
                Box::new(move |reply| {
                    let _ = sender.send(Message::Preferences(id, reply));
                }),
            ) {
                let _ = self.sender.send(Message::Preferences(
                    id,
                    DownloadStoreReply::Error(DownloadError::Storage),
                ));
            }
        }
        // The timer armed above saw neither this transfer nor the work it
        // just started, so it may be the slow one; its replies wait on a tick.
        // Without one, nothing would ever answer them: the transfer ends, as
        // the gate above refuses one it could not time.
        self.ensure_timer();
        if self.timer.borrow().is_none() {
            self.cancel(id, Some(DownloadError::Unavailable));
        }
    }

    pub(super) fn choose_destination(
        self: &Rc<Self>,
        id: DownloadId,
        preferences: DownloadPreferences,
    ) {
        let context = self
            .active
            .borrow()
            .get(&id)
            .filter(|transfer| {
                transfer.record.state == DownloadState::Pending
                    && !transfer.cancelling
                    && !transfer.authorized
                    && transfer.panel.is_none()
            })
            .and_then(|transfer| {
                transfer.source.as_ref().map(|source| {
                    (
                        source.live(),
                        source.key(),
                        source.root,
                        transfer.record.filename.clone(),
                        transfer.record.source.clone(),
                        source.ask_destination,
                    )
                })
            });
        let Some((true, in_front, window, filename, source, ask_destination)) = context else {
            self.cancel(id, None);
            return;
        };
        let directory = preferences
            .directory
            .clone()
            .map(PathBuf::from)
            .or_else(dirs::download_dir);
        // Admission pins the live document; a page's further automatic
        // downloads ask through WebView2's own prompt first.
        if !ask_destination
            && !preferences.ask_destination
            && (preferences.directory.is_none() || preferences.directory_identity.is_some())
        {
            let Some(directory) = directory else {
                self.cancel(id, Some(DownloadError::Destination));
                return;
            };
            self.prepare(id, directory.join(filename), preferences.directory_identity);
            return;
        }
        if !in_front {
            self.defer_destination(id, preferences);
            return;
        }
        let Some(lease) = DialogLease::acquire() else {
            self.defer_destination(id, preferences);
            return;
        };
        let dialog = match dialog(
            false,
            &format!("Save download from {source}"),
            directory.as_deref(),
            Some(&filename),
        ) {
            Ok(dialog) => dialog,
            Err(error) => {
                self.cancel(id, Some(error));
                return;
            }
        };
        let context = self.active.borrow().get(&id).and_then(|transfer| {
            transfer
                .source
                .as_ref()
                .map(|source| (!transfer.cancelling && source.live(), source.key()))
        });
        match context {
            Some((true, true)) => {}
            Some((true, false)) => {
                self.defer_destination(id, preferences);
                return;
            }
            _ => {
                self.cancel(id, None);
                return;
            }
        }
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
            transfer.panel = Some(dialog.clone());
            transfer.panel_lease = Some(lease);
            transfer.deadline = transfer.deadline.waiting_for_picker(Instant::now());
        }
        self.ensure_timer();
        let result = unsafe { dialog.Show(Some(window)) };
        let path = result.ok().and_then(|_| selected(&dialog));
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
            transfer.panel = None;
            transfer.panel_lease.take();
        }
        match path {
            Some(path) => self.prepare(id, path, None),
            None => self.cancel(id, None),
        }
    }

    pub(super) fn cancel(&self, id: DownloadId, error: Option<DownloadError>) {
        let entry = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id) else {
                return;
            };
            if transfer.record.state.terminal()
                || transfer.record.state == DownloadState::Finalizing
                || transfer.cancelling
            {
                return;
            }
            // A held file is complete: no native operation is left to cancel.
            if transfer.record.state == DownloadState::Held {
                drop(active);
                self.terminal(id, DownloadState::Cancelled, error);
                return;
            }
            transfer.cancelling = true;
            transfer.record.state = DownloadState::Cancelling;
            transfer.record.revision += 1;
            transfer.record.error = error;
            transfer.deadline = DecisionDeadline::new(DecisionPhase::Cancelling, Instant::now());
            self.changed(transfer.partition.profile());
            (
                transfer.native.clone(),
                transfer.destination_reply.take(),
                transfer.panel.take(),
                transfer.panel_lease.take(),
            )
        };
        if let Some(panel) = entry.2 {
            cancel_directory(&panel);
        }
        drop(entry.3);
        if let Some(reply) = entry.1 {
            // The deferred event was never admitted to a destination; cancelling
            // it proves that no native writer was started, even without an event.
            reply.finish(None);
            self.terminal(id, DownloadState::Cancelled, None);
            return;
        }
        let timer_armed = self.arm_timer();
        // SAFETY: `entry.0.operation` is a live WebView2 COM interface accessed on its owning UI STA.
        let cancel_failed = unsafe { entry.0.operation.Cancel() }.is_err();
        self.settle_cancelling_transfer(id, &entry.0, cancel_failed, !timer_armed);
    }

    fn suppress_followup_admission(
        &self,
        args: &ICoreWebView2DownloadStartingEventArgs,
        operation: &ICoreWebView2DownloadOperation,
    ) {
        // WebView2 emits follow-up DownloadStarting events with a non-zero
        // interrupt reason when auto-restarting a failed transfer, leaving
        // the original operation orphaned in IN_PROGRESS. Refuse the
        // duplicate row before capacity/intent gates and bypass the grace
        // period only if the event matches a specific active operation.
        // SAFETY: `args` and `operation` are live WebView2 COM interfaces accessed
        // on their owning UI STA thread.
        unsafe {
            let _ = args.SetCancel(true);
            let _ = operation.Cancel();
        }
        let op_identity = operation_identity(operation);
        let matched_id = op_identity.and_then(|target| {
            self.active
                .borrow()
                .iter()
                .find(|(_, transfer)| {
                    transfer.record.state == DownloadState::Receiving
                        && !transfer.cancelling
                        && !transfer.persisting_terminal
                        && operation_identity(&transfer.native.operation) == Some(target)
                })
                .map(|(id, _)| *id)
        });
        self.settle_orphaned_receiving(matched_id);
        if !self.active.borrow().is_empty() && !self.arm_timer() {
            self.settle_without_timer();
        }
    }

    fn write_release_status_or_spawn(&self, id: DownloadId) -> bool {
        let probe = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id) else {
                return false;
            };
            let Some(destination) = transfer.destination.as_ref() else {
                return true;
            };
            if transfer.write_release_observed == Some(true) {
                return true;
            }
            if transfer.write_probe_in_flight {
                return false;
            }
            transfer.write_probe_in_flight = true;
            transfer.write_probe_generation = transfer.write_probe_generation.wrapping_add(1);
            Some((transfer.write_probe_generation, destination.payload()))
        };
        let Some((generation, payload)) = probe else {
            return false;
        };
        let sender = self.sender.clone();
        if std::thread::Builder::new()
            .name("zephium-download-probe".into())
            .spawn(move || {
                let released = super::super::download_files::payload_write_released_at(&payload);
                let _ = sender.send(Message::WriteReleaseChecked {
                    id,
                    generation,
                    released,
                });
            })
            .is_err()
        {
            if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
                if transfer.write_probe_generation == generation {
                    transfer.write_probe_in_flight = false;
                    transfer.write_release_observed = Some(false);
                }
            }
        }
        false
    }

    pub(super) fn write_release_checked(&self, id: DownloadId, generation: u64, released: bool) {
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
            if transfer.write_probe_generation == generation {
                transfer.write_probe_in_flight = false;
                transfer.write_release_observed = Some(released);
            }
        }
    }

    fn note_native_in_progress(self: &Rc<Self>, id: DownloadId) {
        let changed_profile = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id) else {
                return;
            };
            transfer.orphaned_since = None;
            transfer.write_probe_in_flight = false;
            transfer.write_probe_generation = transfer.write_probe_generation.wrapping_add(1);
            transfer.write_release_observed = None;
            if transfer.record.state == DownloadState::Paused && !transfer.cancelling {
                transfer.user_pausing = false;
                transfer.record.state = DownloadState::Receiving;
                transfer.record.error = None;
                transfer.record.revision += 1;
                Some(transfer.partition.profile())
            } else {
                None
            }
        };
        if let Some(profile) = changed_profile {
            self.ensure_timer();
            self.changed(profile);
        }
    }

    pub(super) fn abandon_unresolved(
        &self,
        id: DownloadId,
        error: DownloadError,
        mark_persistence_failed: bool,
    ) {
        if mark_persistence_failed {
            self.persistence_failed.set(true);
        }
        self.release_site(id);
        let (profile, destination, on_started) = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id) else {
                return;
            };
            if transfer.persisting_terminal || transfer.record.state == DownloadState::Finalizing {
                return;
            }
            transfer.record.state = DownloadState::Interrupted;
            transfer.record.error = terminal_outcome(
                DownloadState::Interrupted,
                transfer.record.error,
                Some(error),
            )
            .1;
            transfer.record.writer_released = false;
            transfer.record.revision += 1;
            transfer.persisting_terminal = true;
            (
                transfer.partition.profile(),
                transfer.destination.take(),
                transfer.on_started.take(),
            )
        };
        if let Some(started) = on_started {
            started();
        }
        // Writer release is unproven: keep the partial payload and recovery
        // receipt intact until writer exit / recovery confirms cleanup.
        if let Some(destination) = destination {
            destination.abandon();
        }
        self.persist(id);
        self.changed(profile);
    }

    pub(super) fn settle_without_timer(&self) {
        let cancelling: Vec<_> = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| transfer.cancelling && !transfer.persisting_terminal)
            .map(|(id, transfer)| (*id, transfer.native.clone()))
            .collect();
        for (id, native) in cancelling {
            self.settle_cancelling_transfer(id, &native, false, true);
        }
        let remaining: Vec<_> = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| {
                !transfer.record.state.terminal()
                    && transfer.record.state != DownloadState::Finalizing
                    && !transfer.cancelling
                    && !transfer.persisting_terminal
            })
            .map(|(id, _)| *id)
            .collect();
        for id in remaining {
            self.cancel(id, Some(DownloadError::Unavailable));
        }
    }

    fn settle_cancelling_transfer(
        &self,
        id: DownloadId,
        native: &Native,
        cancel_failed: bool,
        deadline_expired: bool,
    ) {
        let snapshot = operation_snapshot(&native.operation);
        let needs_release_check = !matches!(
            snapshot,
            Some(s)
                if s.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
                    || (s.state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED
                        && !automatically_restarting(s.reason))
        );
        let write_released = if needs_release_check {
            if deadline_expired {
                self.active.borrow().get(&id).is_some_and(|transfer| {
                    transfer.cancelling
                        && !transfer.persisting_terminal
                        && (transfer.destination.is_none()
                            || transfer.write_release_observed == Some(true))
                })
            } else {
                self.write_release_status_or_spawn(id)
            }
        } else {
            false
        };
        match cancel_settlement(snapshot, write_released, deadline_expired, cancel_failed) {
            CancelSettlement::Wait => {}
            CancelSettlement::NativeInterrupted => {
                self.native_failed(id);
            }
            CancelSettlement::ForceCancelled => {
                self.terminal(id, DownloadState::Cancelled, None);
            }
            CancelSettlement::AbandonUnresolved => {
                self.abandon_unresolved(id, DownloadError::Unavailable, false);
            }
        }
    }

    fn settle_orphaned_receiving(&self, matched_id: Option<DownloadId>) {
        let receiving: Vec<_> = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| {
                transfer.record.state == DownloadState::Receiving
                    && !transfer.cancelling
                    && !transfer.persisting_terminal
            })
            .map(|(id, transfer)| (*id, transfer.native.clone()))
            .collect();
        let now = Instant::now();
        for (id, native) in receiving {
            let Some(snapshot) = operation_snapshot(&native.operation) else {
                continue;
            };
            if orphaned_interrupt_error(snapshot.state, snapshot.reason, true).is_none() {
                let mut active = self.active.borrow_mut();
                if let Some(transfer) = active
                    .get_mut(&id)
                    .filter(|transfer| transfer.orphaned_since.is_some())
                {
                    transfer.orphaned_since = None;
                }
                continue;
            }
            let write_released = self.write_release_status_or_spawn(id);
            let Some(error) =
                orphaned_interrupt_error(snapshot.state, snapshot.reason, write_released)
            else {
                let mut active = self.active.borrow_mut();
                if let Some(transfer) = active
                    .get_mut(&id)
                    .filter(|transfer| transfer.orphaned_since.is_some())
                {
                    transfer.orphaned_since = None;
                }
                continue;
            };
            let ready = {
                let mut active = self.active.borrow_mut();
                let Some(transfer) = active.get_mut(&id).filter(|transfer| {
                    transfer.record.state == DownloadState::Receiving
                        && !transfer.cancelling
                        && !transfer.persisting_terminal
                }) else {
                    continue;
                };
                if should_bypass_orphaned_grace(matched_id, id) {
                    true
                } else {
                    let since = *transfer.orphaned_since.get_or_insert(now);
                    now.saturating_duration_since(since) >= ORPHANED_INTERRUPT_GRACE
                }
            };
            if ready {
                self.note_native_error(id, error);
                // SAFETY: `native.operation` is a live WebView2 COM interface accessed
                // on its owning UI STA thread.
                unsafe {
                    let _ = native.operation.Cancel();
                }
                self.native_failed_with_error(id, error);
            }
        }
    }

    pub(super) fn poll_windows_transfers(&self) {
        let now = Instant::now();
        let cancelling: Vec<_> = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| transfer.cancelling && !transfer.persisting_terminal)
            .map(|(id, transfer)| (*id, transfer.native.clone(), transfer.deadline.expired(now)))
            .collect();
        for (id, native, deadline_expired) in cancelling {
            self.settle_cancelling_transfer(id, &native, false, deadline_expired);
        }
        self.settle_orphaned_receiving(None);
    }

    #[cfg(test)]
    pub(in crate::host) fn detach_native_delegate_for_test(&self, id: DownloadId) {
        if let Some(transfer) = self.active.borrow().get(&id) {
            // SAFETY: `operation` is a live WebView2 COM interface on its owning UI STA.
            unsafe {
                let _ = transfer
                    ._delegate
                    .operation
                    .remove_StateChanged(transfer._delegate.token);
            }
        }
    }

    #[cfg(test)]
    pub(in crate::host) fn write_release_state_for_test(
        self: &Rc<Self>,
        id: DownloadId,
        spawn_if_needed: bool,
    ) -> Option<(u64, Option<bool>)> {
        if spawn_if_needed {
            let _ = self.write_release_status_or_spawn(id);
            self.ensure_timer();
        }
        self.active.borrow().get(&id).map(|transfer| {
            (
                transfer.write_probe_generation,
                transfer.write_release_observed,
            )
        })
    }

    #[cfg(test)]
    pub(in crate::host) fn write_release_reply_for_test(
        &self,
        id: DownloadId,
        generation: u64,
        released: bool,
    ) {
        self.write_release_checked(id, generation, released);
    }

    #[cfg(test)]
    pub(in crate::host) fn native_resume_for_test(&self, id: DownloadId) -> Option<u64> {
        let operation = self
            .active
            .borrow()
            .get(&id)
            .map(|transfer| transfer.native.operation.clone());
        let operation = operation?;
        let mut received = 0;
        // SAFETY: `operation` is a live WebView2 COM interface on its owning UI STA.
        unsafe {
            operation.BytesReceived(&mut received).ok()?;
            operation.Resume().ok()?;
        }
        u64::try_from(received).ok()
    }

    /// The person paused a download. WebView2 stops the transfer and reports
    /// it as interrupted by the user; `user_paused` settles it from there.
    pub(super) fn user_pause(
        self: &Rc<Self>,
        partition: Partition,
        id: DownloadId,
    ) -> Result<(), DownloadError> {
        let operation = {
            let mut active = self.active.borrow_mut();
            let transfer = active
                .get_mut(&id)
                .filter(|transfer| {
                    transfer.partition == partition
                        && may_pause(
                            transfer.record.state,
                            transfer.cancelling,
                            transfer.user_pausing,
                            transfer.persisting_terminal,
                            transfer.destination.is_some(),
                        )
                        && retry_source_alive(&transfer.native.permit)
                })
                .ok_or(DownloadError::Invalid)?;
            transfer.user_pausing = true;
            transfer.native.operation.clone()
        };
        // No RefCell borrow crosses a COM call.
        if unsafe { operation.Pause() }.is_err() {
            if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
                transfer.user_pausing = false;
            }
            return Err(DownloadError::Unavailable);
        }
        Ok(())
    }

    /// WebView2 reported the pause the person asked for. False when nobody
    /// asked, so the caller treats the stop as the failure it is.
    fn user_paused(&self, id: DownloadId) -> bool {
        let profile = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id).filter(|transfer| {
                transfer.user_pausing
                    && transfer.record.state == DownloadState::Receiving
                    && !transfer.cancelling
                    && !transfer.persisting_terminal
            }) else {
                return false;
            };
            transfer.user_pausing = false;
            transfer.record.state = DownloadState::Paused;
            transfer.record.error = None;
            transfer.deadline = DecisionDeadline::new(DecisionPhase::UserPaused, Instant::now());
            transfer.record.revision += 1;
            transfer.partition.profile()
        };
        self.persist(id);
        self.changed(profile);
        true
    }

    pub(super) fn resume(
        self: &Rc<Self>,
        partition: Partition,
        id: DownloadId,
    ) -> Result<(), DownloadError> {
        if self.work.get() >= MAX_BACKGROUND_WORK {
            return Err(DownloadError::Capacity);
        }
        let operation = self
            .active
            .borrow()
            .get(&id)
            .filter(|transfer| {
                transfer.partition == partition
                    && transfer.record.state == DownloadState::Paused
                    && !transfer.deadline.expired(Instant::now())
                    && retry_source_alive(&transfer.native.permit)
                    && !transfer.cancelling
                    && transfer.destination.is_some()
            })
            .map(|transfer| transfer.native.operation.clone())
            .ok_or(DownloadError::Invalid)?;
        let mut can_resume = windows::core::BOOL::default();
        let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
        let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
        unsafe {
            operation
                .State(&mut state)
                .and_then(|_| operation.InterruptReason(&mut reason))
                .and_then(|_| operation.CanResume(&mut can_resume))
        }
        .map_err(|_| DownloadError::Unavailable)?;
        if state != COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED
            || !(resumable_reason(reason)
                || reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED)
            || !can_resume.as_bool()
        {
            return Err(DownloadError::Unavailable);
        }
        // No RefCell borrow crosses a COM call. Resume retains the original
        // native operation, its cookies/body, and the admitted staging path.
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id).filter(|transfer| {
            transfer.record.state == DownloadState::Paused
                && !transfer.cancelling
                && !transfer.deadline.expired(Instant::now())
                && retry_source_alive(&transfer.native.permit)
        }) {
            transfer.record.state = DownloadState::Receiving;
            transfer.record.error = None;
            transfer.orphaned_since = None;
            transfer.write_probe_in_flight = false;
            transfer.write_probe_generation = transfer.write_probe_generation.wrapping_add(1);
            transfer.write_release_observed = None;
            transfer.record.revision += 1;
        } else {
            return Err(DownloadError::Invalid);
        }
        if unsafe { operation.Resume() }.is_err() {
            self.pause(id, DownloadError::Unavailable);
            return Err(DownloadError::Unavailable);
        }
        self.ensure_timer();
        self.changed(partition.profile());
        Ok(())
    }

    pub(super) fn choose_directory(
        self: &Rc<Self>,
        token: u64,
        partition: Partition,
        preferences: DownloadPreferences,
        done: DownloadCompletion,
    ) {
        let Some(window) = foreground_window() else {
            return;
        };
        let Some(lease) = DialogLease::acquire() else {
            done.finish(DownloadResponse::Error {
                error: DownloadError::Capacity,
            });
            return;
        };
        let dialog = match dialog(
            true,
            "Choose where Zephium saves downloads",
            preferences.directory.as_deref().map(Path::new),
            None,
        ) {
            Ok(dialog) => dialog,
            Err(error) => {
                done.finish(DownloadResponse::Error { error });
                return;
            }
        };
        self.calls.borrow_mut().insert(
            token,
            UiCall {
                partition,
                call: DownloadCall::ChooseDirectory,
                done,
            },
        );
        self.directory_panels
            .borrow_mut()
            .insert(token, (dialog.clone(), lease));
        self.ensure_timer();
        let path = unsafe { dialog.Show(Some(window)) }
            .ok()
            .and_then(|_| selected(&dialog));
        self.directory_panels.borrow_mut().remove(&token);
        let Some(request) = self.calls.borrow_mut().remove(&token) else {
            return;
        };
        if self.stopping.get() || self.retired.borrow().contains(&partition.profile()) {
            return;
        }
        let Some(path) = path else {
            request.done.finish(DownloadResponse::Error {
                error: DownloadError::Cancelled,
            });
            return;
        };
        self.work.set(self.work.get() + 1);
        let sender = self.sender.clone();
        if std::thread::Builder::new()
            .name("zephium-download-directory".into())
            .spawn(move || {
                let result = super::super::download_files::select_directory(path);
                let _ = sender.send(Message::DirectorySelected(request, preferences, result));
            })
            .is_err()
        {
            self.work.set(self.work.get() - 1);
        }
    }
}

impl Downloads {
    pub(in crate::host) fn retain_closed_view(
        &self,
        view: super::super::ObservedView,
    ) -> Option<super::super::ObservedView> {
        let paused: Vec<_> = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| {
                transfer.record.state == DownloadState::Paused
                    && transfer.native.permit.same_generation(&view.event_permit)
            })
            .map(|(id, transfer)| {
                (
                    *id,
                    transfer.record.error.unwrap_or(DownloadError::Unavailable),
                )
            })
            .collect();
        for (id, cause) in paused {
            self.cancel(id, Some(cause));
        }
        if self.stopping.get()
            || !self.active.borrow().values().any(|transfer| {
                transfer.authorized
                    && !transfer.record.writer_released
                    && transfer.native.permit.same_generation(&view.event_permit)
            })
        {
            return Some(view);
        }
        if !view.event_permit.retire_for_download() {
            return Some(view);
        }
        view.download_surface_intent.store(false, Ordering::Release);
        view.presentation_permit.store(false, Ordering::Release);
        if view.view.set_visible(false).is_err() {
            return Some(view);
        }
        use wry::WebViewExtWindows;
        let parked = (|| -> windows::core::Result<()> {
            let core = view.view.webview();
            unsafe {
                core.Settings()?.SetIsScriptEnabled(false)?;
                core.Stop()?;
                core.Navigate(windows::core::w!("about:blank"))?;
            }
            Ok(())
        })();
        if parked.is_err() {
            return Some(view);
        }
        self.retained_views.borrow_mut().push(view);
        None
    }
    pub(super) fn release_retained_views(&self) {
        if self.stopping.get() {
            return;
        }
        let active: Vec<_> = self
            .active
            .borrow()
            .values()
            .filter(|transfer| !transfer.record.writer_released)
            .map(|transfer| transfer.native.permit.clone())
            .collect();
        let removed = {
            let mut retained = self.retained_views.borrow_mut();
            let mut removed = Vec::new();
            let mut index = 0;
            while index < retained.len() {
                if active
                    .iter()
                    .any(|permit| permit.same_generation(&retained[index].event_permit))
                {
                    index += 1;
                } else {
                    removed.push(retained.swap_remove(index));
                }
            }
            removed
        };
        drop(removed);
    }
    pub(in crate::host) fn take_retained_views(&self) -> Vec<super::super::ObservedView> {
        self.retained_views.borrow_mut().drain(..).collect()
    }

    pub(in crate::host) fn runtime_exited(&self, profile: ProfileId) {
        let ids: Vec<_> = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| transfer.partition.profile() == profile)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.note_native_error(id, DownloadError::Runtime);
            self.native_failed_with_error(id, DownloadError::Runtime);
        }
    }
    pub(in crate::host) fn runtime_exit_notifier(&self) -> Box<dyn FnOnce(bool) + Send> {
        let sender = self.sender.clone();
        Box::new(move |clean| {
            let _ = sender.send(Message::RuntimeExited(clean));
        })
    }
    pub(super) fn runtime_exit_result(&self, clean: bool) {
        let ids: Vec<_> = self.active.borrow().keys().copied().collect();
        for id in ids {
            if clean {
                self.note_native_error(id, DownloadError::Runtime);
                self.native_failed_with_error(id, DownloadError::Runtime);
                continue;
            }
            // No process-exit proof: preserve the partial and its writer
            // incarnation. Recovery must prove that writer ended before cleanup.
            self.abandon_unresolved(id, DownloadError::Unavailable, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_parses_canonical_downloads_with_unicode_and_spaces() {
        use windows::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
        };
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("zażółć 日本語 (1).txt");
        std::fs::write(&file, b"fixture").unwrap();
        let canonical = std::fs::canonicalize(&file).unwrap();
        let native = shell_path(&canonical).unwrap();
        let mut item = std::ptr::null_mut();
        let result =
            unsafe { SHParseDisplayName(PCWSTR(native.as_ptr()), None, &mut item, 0, None) };
        unsafe {
            if !item.is_null() {
                CoTaskMemFree(Some(item.cast()));
            }
            CoUninitialize();
        }
        result.unwrap();
        assert!(!String::from_utf16_lossy(&native).starts_with(r"\\?\"));
    }

    #[test]
    fn interruption_reasons_do_not_mislabel_file_authentication_or_process_errors() {
        for (reason, expected) in [
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_FAILED,
                DownloadError::Destination,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TRANSIENT_ERROR,
                DownloadError::FileBusy,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_LARGE,
                DownloadError::FileTooLarge,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED,
                DownloadError::Authentication,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN,
                DownloadError::Authentication,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CERTIFICATE_PROBLEM,
                DownloadError::Certificate,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CROSS_ORIGIN_REDIRECT,
                DownloadError::Server,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_DOWNLOAD_PROCESS_CRASHED,
                DownloadError::Runtime,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED,
                DownloadError::ConnectionLost,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT,
                DownloadError::Timeout,
            ),
            (
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH,
                DownloadError::Integrity,
            ),
        ] {
            assert_eq!(interrupt_error(reason), expected);
        }
    }

    #[test]
    fn only_the_window_or_a_popup_it_owns_holds_the_foreground_for_it() {
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_OVERLAPPEDWINDOW,
            WS_POPUP,
        };
        let create = |owner: Option<HWND>, style: WINDOW_STYLE| unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                style,
                0,
                0,
                10,
                10,
                owner,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let root = create(None, WS_OVERLAPPEDWINDOW);
        let prompt = create(Some(root), WS_POPUP);
        let other = create(None, WS_OVERLAPPEDWINDOW);
        let other_prompt = create(Some(other), WS_POPUP);
        assert!(fronts(root, root));
        assert!(fronts(root, prompt));
        assert!(!fronts(root, other));
        assert!(!fronts(root, other_prompt));
        assert!(!fronts(root, HWND::default()));
        for window in [other_prompt, other, prompt, root] {
            unsafe { DestroyWindow(window).unwrap() };
        }
    }

    #[test]
    fn native_retry_never_retries_authentication_certificate_or_policy_rejection() {
        assert!(resumable_reason(
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED
        ));
        assert!(resumable_reason(
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH
        ));
        for reason in [
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CERTIFICATE_PROBLEM,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CROSS_ORIGIN_REDIRECT,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED,
        ] {
            assert!(!resumable_reason(reason));
        }
    }

    #[test]
    fn cancelling_settlement_handles_stalled_interrupts_without_releasing_active_writers() {
        let snapshot = |state, reason, can_resume| {
            Some(OperationSnapshot {
                state,
                reason,
                can_resume,
            })
        };
        // Non-auto-restarting interrupted state settles immediately via native interrupt.
        for reason in [
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH,
        ] {
            assert_eq!(
                cancel_settlement(
                    snapshot(COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED, reason, false),
                    false,
                    false,
                    false,
                ),
                CancelSettlement::NativeInterrupted
            );
        }
        // Auto-restarting interrupted state must wait until writer release and !can_resume,
        // or abandon unresolved at the 3s deadline.
        for reason in [
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_HASH_MISMATCH,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_SHORT,
        ] {
            assert_eq!(
                cancel_settlement(
                    snapshot(COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED, reason, true),
                    true,
                    false,
                    false,
                ),
                CancelSettlement::Wait
            );
            assert_eq!(
                cancel_settlement(
                    snapshot(COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED, reason, false),
                    false,
                    false,
                    false,
                ),
                CancelSettlement::Wait
            );
            assert_eq!(
                cancel_settlement(
                    snapshot(COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED, reason, false),
                    true,
                    false,
                    false,
                ),
                CancelSettlement::NativeInterrupted
            );
            assert_eq!(
                cancel_settlement(
                    snapshot(COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED, reason, true),
                    false,
                    true,
                    false,
                ),
                CancelSettlement::AbandonUnresolved
            );
        }
        // In-progress operation with a non-zero interrupt reason settles once write handle is gone.
        assert_eq!(
            cancel_settlement(
                snapshot(
                    COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
                    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH,
                    false,
                ),
                false,
                false,
                false,
            ),
            CancelSettlement::Wait
        );
        assert_eq!(
            cancel_settlement(
                snapshot(
                    COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
                    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH,
                    false,
                ),
                true,
                false,
                false,
            ),
            CancelSettlement::ForceCancelled
        );
        // Active healthy download with expired deadline settles normally only if writer is released;
        // otherwise it abandons unresolved so staging is preserved for recovery.
        assert_eq!(
            cancel_settlement(
                snapshot(
                    COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
                    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
                    false,
                ),
                false,
                true,
                false,
            ),
            CancelSettlement::AbandonUnresolved
        );
        assert_eq!(
            cancel_settlement(
                snapshot(
                    COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
                    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
                    false,
                ),
                true,
                true,
                false,
            ),
            CancelSettlement::ForceCancelled
        );
        // Missing/completed operation settles cleanly, and missing operation without release proof
        // abandons unresolved at deadline.
        assert_eq!(
            cancel_settlement(
                snapshot(
                    COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
                    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
                    false,
                ),
                false,
                false,
                false,
            ),
            CancelSettlement::ForceCancelled
        );
        assert_eq!(
            cancel_settlement(None, true, false, false),
            CancelSettlement::ForceCancelled
        );
        assert_eq!(
            cancel_settlement(None, false, true, false),
            CancelSettlement::AbandonUnresolved
        );
    }

    #[test]
    fn followup_interrupt_suppresses_before_capacity_refusal_and_keeps_unrelated_grace() {
        assert_eq!(
            classify_admission(
                InitialInterruptState::Interrupted(
                    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE,
                ),
                true,
                false,
            ),
            AdmissionDisposition::SuppressFollowup
        );
        assert_eq!(
            classify_admission(InitialInterruptState::Unknown, true, false),
            AdmissionDisposition::SuppressFollowup
        );
        assert_eq!(
            classify_admission(InitialInterruptState::Unknown, false, true),
            AdmissionDisposition::SuppressFollowup
        );
        assert_eq!(
            classify_admission(InitialInterruptState::None, true, true),
            AdmissionDisposition::Refuse(DownloadError::Capacity)
        );
        assert_eq!(
            classify_admission(InitialInterruptState::None, false, false),
            AdmissionDisposition::Refuse(DownloadError::Unavailable)
        );
        assert_eq!(
            classify_admission(InitialInterruptState::None, false, true),
            AdmissionDisposition::Admit
        );

        let a = DownloadId::generate();
        let b = DownloadId::generate();
        assert!(should_bypass_orphaned_grace(Some(a), a));
        assert!(!should_bypass_orphaned_grace(Some(a), b));
        assert!(!should_bypass_orphaned_grace(None, a));
        assert!(!should_bypass_orphaned_grace(None, b));
    }

    #[test]
    fn orphaned_interrupt_detects_stuck_in_progress_failures_only_when_writer_closed() {
        assert_eq!(
            orphaned_interrupt_error(
                COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH,
                true,
            ),
            Some(DownloadError::Integrity)
        );
        assert_eq!(
            orphaned_interrupt_error(
                COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH,
                false,
            ),
            None
        );
        assert_eq!(
            orphaned_interrupt_error(
                COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
                true,
            ),
            None
        );
        assert_eq!(
            orphaned_interrupt_error(
                COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE,
                true,
            ),
            None
        );
    }
}
