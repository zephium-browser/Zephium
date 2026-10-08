//! UI-apartment WebView2 transport. The shared coordinator owns history,
//! staging decisions and recovery; this adapter never replays a request URL.
use super::*;
#[path = "windows_timer.rs"]
mod timer;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::StateChangedEventHandler;
use windows::core::{HRESULT, HSTRING, PCWSTR, PWSTR};
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
    GA_ROOT, SW_SHOWNORMAL,
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
}
impl Source {
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
            && unsafe { GetForegroundWindow() == self.root && IsWindowEnabled(self.root).as_bool() }
    }
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
    take_string(text, 8192)
}
fn current_origin(controller: &ICoreWebView2Controller) -> Option<PageOrigin> {
    let core = unsafe { controller.CoreWebView2() }.ok()?;
    let mut value = PWSTR::null();
    unsafe { core.Source(&mut value) }.ok()?;
    let url = url::Url::parse(&take_string(value, 8192)?).ok()?;
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
    let path = wide(path)?;
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
    let path = wide(path)?;
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

impl Downloads {
    pub(super) fn ensure_timer(self: &Rc<Self>) {
        if self.timer.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let timer = timer::schedule(Duration::from_millis(200), move || {
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
        ),
        mut initial: Option<InitialDownload>,
    ) {
        let (controller, args) = request;
        let initial_open = initial.is_some();
        if self.stopping.get()
            || self.retired.borrow().contains(&partition.profile())
            || self.active.borrow().len() >= MAX_ACTIVE_DOWNLOADS
            || self.work.get() >= MAX_BACKGROUND_WORK
            || !self.private_cleanup_capacity(partition)
            || permit.active_token().is_none()
            || (!initial_open && !surface_intent.load(Ordering::Acquire))
        {
            return;
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
        };
        if !source.key() {
            return;
        }
        let Ok(operation) = (unsafe { args.DownloadOperation() }) else {
            return;
        };
        let Some(uri) = operation_uri(&operation) else {
            return;
        };
        let Some(origin) = source_origin(&uri, controller) else {
            return;
        };
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
            } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
                unsafe { operation.InterruptReason(&mut reason) }?;
                if automatically_restarting(reason) {
                    if manager
                        .active
                        .borrow()
                        .get(&id)
                        .is_some_and(|transfer| transfer.cancelling)
                    {
                        unsafe {
                            let _ = operation.Cancel();
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
                on_started: initial
                    .as_mut()
                    .and_then(|context| context.on_started.take()),
                authorized: false,
                cancelling: false,
                persisting_terminal: false,
                deadline: DecisionDeadline::new(DecisionPhase::Admission, Instant::now()),
            },
        );
        (self.notify)(partition.profile());
        self.ensure_recovery(partition);
        let cached = self.preferences.borrow().get(&partition.profile()).cloned();
        if let Some(preferences) = cached {
            self.choose_destination(id, preferences);
        } else if matches!(partition, Partition::Ephemeral(_)) {
            self.choose_destination(id, DownloadPreferences::default());
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
    }

    pub(super) fn choose_destination(
        self: &Rc<Self>,
        id: DownloadId,
        preferences: DownloadPreferences,
    ) {
        let context = self.active.borrow().get(&id).and_then(|transfer| {
            transfer.source.as_ref().map(|source| {
                (
                    source.key(),
                    source.root,
                    transfer.record.filename.clone(),
                    transfer.record.source.clone(),
                )
            })
        });
        let Some((true, window, filename, source)) = context else {
            self.cancel(id, None);
            return;
        };
        let directory = preferences
            .directory
            .clone()
            .map(PathBuf::from)
            .or_else(dirs::download_dir);
        // The event exposes neither initiating-frame activation nor a main-frame
        // navigation ID. A concurrent browser navigation is not proof for this
        // transfer. Every Windows download therefore requires native confirmation.
        let Some(lease) = DialogLease::acquire() else {
            self.cancel(id, Some(DownloadError::Capacity));
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
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
            transfer.panel = Some(dialog.clone());
            transfer.panel_lease = Some(lease);
            transfer.deadline = DecisionDeadline::new(DecisionPhase::NativePicker, Instant::now());
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
            transfer.cancelling = true;
            transfer.record.state = DownloadState::Cancelling;
            transfer.record.revision += 1;
            transfer.record.error = error;
            (self.notify)(transfer.partition.profile());
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
        unsafe {
            let _ = entry.0.operation.Cancel();
        }
        let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
        let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
        if unsafe {
            entry
                .0
                .operation
                .State(&mut state)
                .and_then(|_| entry.0.operation.InterruptReason(&mut reason))
        }
        .is_ok()
            && state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED
            && !automatically_restarting(reason)
        {
            self.native_failed(id);
        } else if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
            self.terminal(id, DownloadState::Cancelled, None);
        }
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
            || !resumable_reason(reason)
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
            transfer.record.revision += 1;
        } else {
            return Err(DownloadError::Invalid);
        }
        if unsafe { operation.Resume() }.is_err() {
            self.pause(id, DownloadError::Unavailable);
            return Err(DownloadError::Unavailable);
        }
        self.ensure_timer();
        (self.notify)(partition.profile());
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
            self.persistence_failed.set(true);
            let destination = {
                let mut active = self.active.borrow_mut();
                let Some(transfer) = active.get_mut(&id) else {
                    continue;
                };
                if transfer.persisting_terminal
                    || transfer.record.state == DownloadState::Finalizing
                {
                    continue;
                }
                transfer.record.state = DownloadState::Interrupted;
                transfer.record.error = Some(DownloadError::Unavailable);
                transfer.record.writer_released = false;
                transfer.record.revision += 1;
                transfer.persisting_terminal = true;
                transfer.destination.take()
            };
            // No process-exit proof: preserve the partial and its writer
            // incarnation. Recovery must prove that writer ended before cleanup.
            if let Some(destination) = destination {
                destination.abandon();
            }
            self.persist(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
