//! WebKit/AppKit download transport and UI; shared state/recovery stays in the coordinator.
use super::super::file_uploads::PanelLease;
use super::*;
use block2::{Block, RcBlock};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::ProtocolObject, DefinedClass, MainThreadOnly,
};
use objc2_app_kit::{
    NSModalResponse, NSModalResponseOK, NSOpenPanel, NSSavePanel, NSWindow, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSData, NSError, NSHTTPURLResponse, NSObject, NSObjectProtocol,
    NSProgressReporting, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer,
    NSURLAuthenticationChallenge, NSURLAuthenticationMethodServerTrust, NSURLCredential,
    NSURLRequest, NSURLResponse, NSURLSessionAuthChallengeDisposition, NSURL,
};
use objc2_web_kit::{WKDownload, WKDownloadDelegate, WKDownloadRedirectPolicy, WKWebView};

pub(super) type Native = Retained<WKDownload>;
pub(super) type Delegate = Retained<DownloadDelegate>;
pub(super) type Timer = Retained<NSTimer>;
pub(super) type SavePanel = Retained<NSSavePanel>;
pub(super) type DirectoryPanel = Retained<NSOpenPanel>;
pub(super) type DialogLease = PanelLease;

pub(super) fn progress(native: &Native) -> Option<(u64, Option<u64>)> {
    let progress = native.progress();
    Some((
        progress.completedUnitCount().max(0) as u64,
        u64::try_from(progress.totalUnitCount())
            .ok()
            .filter(|value| *value > 0),
    ))
}
pub(super) fn completion_bytes(_native: &Native) -> Result<Option<u64>, DownloadError> {
    // NSProgress reports work units; WebKit does not promise decoded file bytes.
    Ok(None)
}
pub(super) fn stop_timer(timer: Timer) {
    timer.invalidate();
}
pub(super) fn cancel_directory(panel: &DirectoryPanel) {
    unsafe { panel.cancel(None) };
    panel.orderOut(None);
}
pub(super) fn reveal(path: &std::path::Path) -> Result<(), DownloadError> {
    let path = path.to_str().ok_or(DownloadError::Destination)?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    NSWorkspace::sharedWorkspace()
        .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    Ok(())
}
pub(super) fn open(path: &std::path::Path) -> Result<(), DownloadError> {
    let path = path.to_str().ok_or(DownloadError::Destination)?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err(DownloadError::Unavailable)
    }
}

pub(super) struct Source {
    permit: EventPermit,
    surface_intent: Arc<AtomicBool>,
    navigation: NavigationEpochTracker,
    activity: NavigationActivity,
    view: Retained<WKWebView>,
    window: Retained<NSWindow>,
    initial_open: bool,
}
impl Source {
    pub(super) fn live(&self) -> bool {
        self.permit.active_token().is_some()
            && (self.initial_open || self.surface_intent.load(Ordering::Acquire))
            && self.navigation.matches_activity(self.activity)
            && (self.initial_open
                || unsafe { self.view.superview() }
                    .is_some_and(|parent| !parent.isHiddenOrHasHiddenAncestor()))
            && self.window.isVisible()
            && (self.initial_open
                || self
                    .view
                    .window()
                    .is_some_and(|window| std::ptr::eq(&*window, &*self.window)))
    }
}

impl Downloads {
    pub(super) fn resume(
        &self,
        _partition: Partition,
        _id: DownloadId,
    ) -> Result<(), DownloadError> {
        Err(DownloadError::Unsupported)
    }
    pub(super) fn ensure_timer(self: &Rc<Self>) {
        if self.timer.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let callback = RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
            if let Some(manager) = weak.upgrade() {
                manager.tick();
            }
        });
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.2, true, &callback) };
        unsafe {
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
        *self.timer.borrow_mut() = Some(timer);
    }
    pub(in crate::host) fn admit(
        self: &Rc<Self>,
        partition: Partition,
        permit: EventPermit,
        surface_intent: Arc<AtomicBool>,
        navigation: NavigationEpochTracker,
        native: &WKDownload,
        mut initial: Option<InitialDownload>,
    ) {
        if self.stopping.get()
            || self.retired.borrow().contains(&partition.profile())
            || self.active.borrow().len() >= MAX_ACTIVE_DOWNLOADS
            || self.work.get() >= MAX_BACKGROUND_WORK
            || !self.private_cleanup_capacity(partition)
            || permit.active_token().is_none()
            || (initial.is_none() && !surface_intent.load(Ordering::Acquire))
        {
            unsafe { native.cancel(None) };
            return;
        }
        let request_url = unsafe { native.originalRequest().and_then(|request| request.URL()) };
        let scheme = request_url
            .as_ref()
            .and_then(|url| url.scheme())
            .and_then(|scheme| bounded(&scheme, 16));
        let network_url = if matches!(scheme.as_deref(), Some("http" | "https")) {
            request_url
                .as_ref()
                .and_then(|url| url.absoluteString())
                .and_then(|url| bounded(&url, 8192))
                .filter(|url| zephium_core::navigation::is_allowed_str(url))
                .and_then(|url| url::Url::parse(&url).ok())
        } else {
            None
        };
        if network_url.is_none() && !matches!(scheme.as_deref(), Some("blob" | "data")) {
            unsafe { native.cancel(None) };
            return;
        }
        let Some(view) = (unsafe { native.webView() }) else {
            unsafe { native.cancel(None) };
            return;
        };
        let initial_open = initial.is_some();
        let Some(window) = initial
            .as_ref()
            .map(|context| context.window.clone())
            .or_else(|| view.window())
        else {
            unsafe { native.cancel(None) };
            return;
        };
        let Some(activity) = navigation.activity_snapshot() else {
            unsafe { native.cancel(None) };
            return;
        };
        let source = Source {
            navigation,
            activity,
            permit,
            surface_intent,
            view,
            window,
            initial_open,
        };
        if !source.live() || !source.window.isKeyWindow() {
            unsafe { native.cancel(None) };
            return;
        }
        let origin = unsafe {
            if native.respondsToSelector(objc2::sel!(originatingFrame)) {
                let native_origin = native.originatingFrame().securityOrigin();
                let scheme = bounded(&native_origin.protocol(), 16);
                let host = bounded(&native_origin.host(), 512);
                scheme.zip(host).and_then(|(scheme, host)| {
                    let port = native_origin.port();
                    if !(0..=65535).contains(&port) {
                        return None;
                    }
                    PageOrigin::from_native_components(
                        &scheme,
                        &host,
                        (port != 0).then_some(port as u16),
                    )
                    .ok()
                })
            } else {
                source
                    .view
                    .URL()
                    .and_then(|url| url.absoluteString())
                    .and_then(|url| bounded(&url, 8192))
                    .and_then(|url| url::Url::parse(&url).ok())
                    .and_then(|url| PageOrigin::from_url(&url).ok())
            }
        };
        let origin = origin.or_else(|| {
            network_url
                .as_ref()
                .and_then(|url| PageOrigin::from_url(url).ok())
        });
        let Some(origin) = origin else {
            unsafe { native.cancel(None) };
            return;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            unsafe { native.cancel(None) };
            return;
        };
        let id = DownloadId::generate();
        let delegate = DownloadDelegate::new(mtm, Rc::downgrade(self), id);
        let record = DownloadRecord {
            id,
            session: self.session,
            revision: 1,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_secs() as i64),
            filename: "download".into(),
            source: origin.as_str().into(),
            source_is_context: false,
            state: DownloadState::Pending,
            received: 0,
            total: None,
            error: None,
            destination: None,
            staging: None,
            staging_identity: None,
            identity: None,
            writer: None,
            writer_released: false,
        };
        self.active.borrow_mut().insert(
            id,
            Transfer {
                partition,
                record,
                native: native.into(),
                _delegate: delegate.clone(),
                source: Some(source),
                destination_reply: None,
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
        unsafe { native.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        self.ensure_recovery(partition);
        self.ensure_timer();
        (self.notify)(partition.profile());
    }
    pub(super) fn destination(
        self: &Rc<Self>,
        id: DownloadId,
        response: &NSURLResponse,
        suggested: &NSString,
        completion: &Block<dyn Fn(*mut NSURL)>,
    ) {
        let name = bounded(suggested, 4096)
            .map(|value| safe_filename(&value))
            .unwrap_or_else(|| "download".into());
        let accepts = self.active.borrow().get(&id).is_some_and(|transfer| {
            transfer.record.state == DownloadState::Pending
                && transfer.destination_reply.is_none()
                && transfer.destination.is_none()
                && !transfer.cancelling
        });
        if !accepts {
            completion.call((std::ptr::null_mut(),));
            return;
        }
        let partition = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id) else {
                return;
            };
            transfer.record.filename = name;
            transfer.record.total = u64::try_from(response.expectedContentLength()).ok();
            let completion = completion.copy();
            transfer.destination_reply = Some(DestinationReply::new(move |path| {
                let url = path.and_then(|path| {
                    path.to_str()
                        .map(|path| NSURL::fileURLWithPath(&NSString::from_str(path)))
                });
                completion.call((url
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |url| Retained::as_ptr(url).cast_mut()),));
            }));
            transfer.partition
        };
        if let Some(preferences) = self.preferences.borrow().get(&partition.profile()).cloned() {
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
        let context = {
            let active = self.active.borrow();
            active.get(&id).and_then(|transfer| {
                transfer.source.as_ref().map(|source| {
                    (
                        source.live(),
                        source.window.clone(),
                        transfer.record.filename.clone(),
                        transfer.record.source.clone(),
                        unsafe {
                            transfer
                                .native
                                .respondsToSelector(objc2::sel!(isUserInitiated))
                                && transfer.native.isUserInitiated()
                        },
                    )
                })
            })
        };
        let Some((true, window, filename, source, user_initiated)) = context else {
            self.cancel(id, None);
            return;
        };
        let directory = preferences
            .directory
            .clone()
            .map(PathBuf::from)
            .or_else(dirs::download_dir);
        if !preferences.ask_destination
            && user_initiated
            && (preferences.directory.is_none() || preferences.directory_identity.is_some())
        {
            let Some(directory) = directory else {
                self.cancel(id, Some(DownloadError::Destination));
                return;
            };
            self.prepare(
                id,
                directory.join(filename),
                preferences.directory_identity.clone(),
            );
            return;
        }
        if window.attachedSheet().is_some() || !window.isKeyWindow() {
            self.cancel(id, Some(DownloadError::Capacity));
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            self.cancel(id, Some(DownloadError::Unavailable));
            return;
        };
        let Some(lease) = PanelLease::acquire() else {
            self.cancel(id, Some(DownloadError::Capacity));
            return;
        };
        let panel = NSSavePanel::savePanel(mtm);
        panel.setNameFieldStringValue(&NSString::from_str(&filename));
        panel.setMessage(Some(&NSString::from_str(&format!(
            "Save download from {source}"
        ))));
        if let Some(path) = directory.and_then(|path| path.to_str().map(str::to_owned)) {
            panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(&path))));
        }
        let live = self.active.borrow().get(&id).is_some_and(|transfer| {
            !transfer.cancelling && transfer.source.as_ref().is_some_and(Source::live)
        });
        if !live || !window.isKeyWindow() || window.attachedSheet().is_some() {
            self.cancel(id, Some(DownloadError::Unavailable));
            return;
        }
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
            transfer.panel = Some(panel.clone());
            transfer.panel_lease = Some(lease);
            // A person may browse the filesystem for longer than a network
            // callback deadline. Lifetime revocation still applies each tick.
            transfer.deadline = DecisionDeadline::new(DecisionPhase::NativePicker, Instant::now());
        }
        let weak = Rc::downgrade(self);
        let selected_panel = panel.clone();
        let callback = RcBlock::new(move |result: NSModalResponse| {
            let Some(manager) = weak.upgrade() else {
                return;
            };
            let path = (result == NSModalResponseOK)
                .then(|| selected_panel.URL())
                .flatten()
                .and_then(|url| url.path())
                .and_then(|path| bounded(&path, 4096))
                .map(PathBuf::from);
            selected_panel.orderOut(None);
            if let Some(transfer) = manager.active.borrow_mut().get_mut(&id) {
                transfer.panel = None;
                transfer.panel_lease.take();
            }
            match path {
                Some(path) => manager.prepare(id, path, None),
                None => manager.cancel(id, None),
            }
        });
        panel.beginSheetModalForWindow_completionHandler(&window, &callback);
    }
    pub(super) fn cancel(&self, id: DownloadId, error: Option<DownloadError>) {
        let native = {
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
            )
        };
        if let Some(panel) = native.2 {
            unsafe { panel.cancel(None) };
            panel.orderOut(None);
        }
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
            transfer.panel_lease.take();
        }
        if let Some(reply) = native.1 {
            reply.finish(None);
        }
        let sender = self.sender.clone();
        let cancelled = RcBlock::new(move |_resume: *mut NSData| {
            let _ = sender.send(Message::Cancelled(id));
        });
        unsafe {
            native.0.cancel(Some(&cancelled));
        }
    }
    pub(super) fn choose_directory(
        self: &Rc<Self>,
        token: u64,
        partition: Partition,
        preferences: DownloadPreferences,
        done: DownloadCompletion,
    ) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(window) = objc2_app_kit::NSApplication::sharedApplication(mtm).keyWindow() else {
            return;
        };
        if window.attachedSheet().is_some() {
            done.finish(DownloadResponse::Error {
                error: DownloadError::Capacity,
            });
            return;
        }
        let Some(lease) = PanelLease::acquire() else {
            done.finish(DownloadResponse::Error {
                error: DownloadError::Capacity,
            });
            return;
        };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        panel.setAllowsMultipleSelection(false);
        panel.setMessage(Some(&NSString::from_str(
            "Choose where Zephium saves downloads",
        )));
        if self.stopping.get()
            || self.retired.borrow().contains(&partition.profile())
            || !window.isVisible()
            || !window.isKeyWindow()
            || window.attachedSheet().is_some()
        {
            done.finish(DownloadResponse::Error {
                error: DownloadError::Unavailable,
            });
            return;
        }
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
            .insert(token, (panel.clone(), lease));
        let selected = panel.clone();
        let weak = Rc::downgrade(self);
        let callback = RcBlock::new(move |result: NSModalResponse| {
            let Some(manager) = weak.upgrade() else {
                return;
            };
            let _panel_owner = manager.directory_panels.borrow_mut().remove(&token);
            let request = manager.calls.borrow_mut().remove(&token);
            let Some(request) = request else { return };
            let path = (result == NSModalResponseOK)
                .then(|| selected.URLs())
                .and_then(|urls| urls.firstObject())
                .and_then(|url| url.path())
                .and_then(|path| bounded(&path, 4096));
            selected.orderOut(None);
            if let Some(path) = path {
                manager.work.set(manager.work.get() + 1);
                let sender = manager.sender.clone();
                let preferences = preferences.clone();
                if std::thread::Builder::new()
                    .name("zephium-download-directory".into())
                    .spawn(move || {
                        let result =
                            super::super::download_files::select_directory(PathBuf::from(path));
                        let _ =
                            sender.send(Message::DirectorySelected(request, preferences, result));
                    })
                    .is_err()
                {
                    manager.work.set(manager.work.get() - 1);
                }
            } else {
                request.done.finish(DownloadResponse::Error {
                    error: DownloadError::Cancelled,
                });
            }
        });
        panel.beginSheetModalForWindow_completionHandler(&window, &callback);
    }
}
pub(super) fn bounded(value: &NSString, max: usize) -> Option<String> {
    if value.length() > max {
        return None;
    }
    let value = value.to_string();
    (value.len() <= max).then_some(value)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ErrorContext {
    Unknown,
    Transport,
    Source,
    Destination,
}

fn download_error(error: &NSError) -> DownloadError {
    bounded_download_error(error, 4, ErrorContext::Unknown).unwrap_or(DownloadError::Network)
}

fn bounded_download_error(
    error: &NSError,
    remaining: usize,
    inherited: ErrorContext,
) -> Option<DownloadError> {
    use objc2_foundation::{
        NSCocoaErrorDomain, NSPOSIXErrorDomain, NSURLErrorDomain, NSUnderlyingErrorKey,
    };
    let domain = error.domain();
    let code = error.code();
    let url = unsafe { domain.isEqualToString(NSURLErrorDomain) };
    let cocoa = unsafe { domain.isEqualToString(NSCocoaErrorDomain) };
    let context = if url {
        match code {
            -3005..=-3000 => ErrorContext::Destination,
            -1104..=-1100 => ErrorContext::Source,
            _ => ErrorContext::Transport,
        }
    } else if inherited != ErrorContext::Unknown {
        inherited
    } else if cocoa {
        match code {
            512..=518 | 640 | 642 => ErrorContext::Destination,
            4 | 256..=264 => ErrorContext::Source,
            _ => ErrorContext::Unknown,
        }
    } else {
        ErrorContext::Unknown
    };
    let destination = context == ErrorContext::Destination;
    let kind = if unsafe { domain.isEqualToString(NSPOSIXErrorDomain) } {
        match i32::try_from(code).ok() {
            Some(libc::ENODATA | libc::EPROTO | libc::EBADMSG) => Some(DownloadError::Integrity),
            Some(
                libc::ECONNRESET
                | libc::ECONNABORTED
                | libc::ENETDOWN
                | libc::ENETRESET
                | libc::ENOTCONN,
            ) => Some(DownloadError::ConnectionLost),
            Some(libc::ETIMEDOUT) => Some(DownloadError::Timeout),
            Some(libc::ENOSPC) if destination => Some(DownloadError::DiskFull),
            Some(libc::EACCES | libc::EPERM | libc::EROFS) if destination => {
                Some(DownloadError::Permission)
            }
            Some(libc::EBUSY | libc::EMFILE | libc::ENFILE) if destination => {
                Some(DownloadError::FileBusy)
            }
            Some(libc::EFBIG) if destination => Some(DownloadError::FileTooLarge),
            Some(libc::ENOENT | libc::ENOTDIR | libc::EISDIR | libc::ENAMETOOLONG)
                if destination =>
            {
                Some(DownloadError::Destination)
            }
            _ if context == ErrorContext::Source => Some(DownloadError::Source),
            _ => None,
        }
    } else if cocoa {
        match code {
            640 if destination => Some(DownloadError::DiskFull),
            257 | 513 | 642 if destination => Some(DownloadError::Permission),
            4 | 256 | 258 | 512 | 514 | 516 | 518 if destination => {
                Some(DownloadError::Destination)
            }
            4 | 256..=264 if context == ErrorContext::Source => Some(DownloadError::Source),
            _ => None,
        }
    } else if url {
        match code {
            -999 => Some(DownloadError::Cancelled),
            -1001 => Some(DownloadError::Timeout),
            -1005 | -1009 => Some(DownloadError::ConnectionLost),
            -1012 | -1013 | -1205 | -1206 => Some(DownloadError::Authentication),
            -1204..=-1200 => Some(DownloadError::Certificate),
            -1000 | -1002 | -1007 | -1010 | -1011 => Some(DownloadError::Server),
            -1104..=-1100 => Some(DownloadError::Source),
            -3005..=-3000 => Some(DownloadError::Destination),
            -3007 | -3006 | -1015 | -1016 => Some(DownloadError::Integrity),
            _ => None,
        }
    } else if unsafe { domain.isEqualToString(objc2_web_kit::WKErrorDomain) }
        && matches!(code, 2 | 3)
    {
        Some(DownloadError::Runtime)
    } else {
        None
    };
    let underlying = if remaining > 0 {
        unsafe { error.userInfo().objectForKey(NSUnderlyingErrorKey) }.and_then(|inner| {
            inner
                .downcast_ref::<NSError>()
                .and_then(|inner| bounded_download_error(inner, remaining - 1, context))
        })
    } else {
        None
    };
    match (kind, underlying) {
        (
            Some(
                error @ (DownloadError::Authentication
                | DownloadError::Certificate
                | DownloadError::Runtime
                | DownloadError::Cancelled),
            ),
            _,
        ) => Some(error),
        (Some(error), Some(DownloadError::Destination)) if error != DownloadError::Destination => {
            Some(error)
        }
        (_, Some(error)) | (Some(error), None) => Some(error),
        _ => None,
    }
}

fn native_error_codes(error: &NSError) -> Vec<(&'static str, isize)> {
    use objc2_foundation::{
        NSCocoaErrorDomain, NSPOSIXErrorDomain, NSURLErrorDomain, NSUnderlyingErrorKey,
    };
    fn append(error: &NSError, remaining: usize, out: &mut Vec<(&'static str, isize)>) {
        let domain = error.domain();
        let label = if unsafe { domain.isEqualToString(NSURLErrorDomain) } {
            "url"
        } else if unsafe { domain.isEqualToString(NSPOSIXErrorDomain) } {
            "posix"
        } else if unsafe { domain.isEqualToString(NSCocoaErrorDomain) } {
            "cocoa"
        } else if unsafe { domain.isEqualToString(objc2_web_kit::WKErrorDomain) } {
            "webkit"
        } else {
            "other"
        };
        out.push((label, error.code()));
        if remaining > 0 {
            if let Some(inner) = unsafe { error.userInfo().objectForKey(NSUnderlyingErrorKey) }
                .and_then(|inner| inner.downcast_ref::<NSError>().map(Retained::from))
            {
                append(&inner, remaining - 1, out);
            }
        }
    }
    let mut out = Vec::with_capacity(5);
    append(error, 4, &mut out);
    out
}

pub(super) struct DelegateIvars {
    manager: Weak<Downloads>,
    // Objective-C ivars on macOS cannot carry Rust's 16-byte u128 alignment.
    // Keep the ULID behind an ordinary pointer-aligned owner.
    id: Box<DownloadId>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind=MainThreadOnly]
    #[ivars=DelegateIvars]
    pub(super) struct DownloadDelegate;
    unsafe impl NSObjectProtocol for DownloadDelegate {}
    unsafe impl WKDownloadDelegate for DownloadDelegate {
        #[unsafe(method(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:))]
        unsafe fn decide(
            &self,
            _download: &WKDownload,
            response: &NSURLResponse,
            name: &NSString,
            completion: &Block<dyn Fn(*mut NSURL)>,
        ) {
            if let Some(manager) = self.ivars().manager.upgrade() {
                manager.destination(*self.ivars().id, response, name, completion);
            } else {
                completion.call((std::ptr::null_mut(),));
            }
        }
        #[unsafe(method(downloadDidFinish:))]
        unsafe fn finished(&self, _download: &WKDownload) {
            if let Some(manager) = self.ivars().manager.upgrade() {
                manager.native_finished(*self.ivars().id);
            }
        }
        #[unsafe(method(download:didFailWithError:resumeData:))]
        unsafe fn failed(&self, _download: &WKDownload, error: &NSError, _resume: Option<&NSData>) {
            if let Some(manager) = self.ivars().manager.upgrade() {
                let cause = download_error(error);
                // Static domain classes and numeric codes only; never descriptions,
                // filenames, URLs, origins, paths, resume data or userInfo contents.
                eprintln!(
                    "downloads: native failure cause={cause:?} codes={:?}",
                    native_error_codes(error)
                );
                manager.native_failed_with_error(*self.ivars().id, cause);
            }
        }
        #[unsafe(method(download:willPerformHTTPRedirection:newRequest:decisionHandler:))]
        unsafe fn redirect(
            &self,
            _download: &WKDownload,
            _response: &NSHTTPURLResponse,
            request: &NSURLRequest,
            completion: &Block<dyn Fn(WKDownloadRedirectPolicy)>,
        ) {
            let allowed = request
                .URL()
                .and_then(|url| url.absoluteString())
                .and_then(|url| bounded(&url, 8192))
                .is_some_and(|url| zephium_core::navigation::is_allowed_str(&url));
            completion.call((if allowed {
                WKDownloadRedirectPolicy::Allow
            } else {
                WKDownloadRedirectPolicy::Cancel
            },));
        }
        #[unsafe(method(download:didReceiveAuthenticationChallenge:completionHandler:))]
        unsafe fn authentication(
            &self,
            _download: &WKDownload,
            challenge: &NSURLAuthenticationChallenge,
            completion: &Block<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>,
        ) {
            let trust = challenge
                .protectionSpace()
                .authenticationMethod()
                .isEqualToString(NSURLAuthenticationMethodServerTrust);
            completion.call((
                if trust {
                    NSURLSessionAuthChallengeDisposition::PerformDefaultHandling
                } else {
                    NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge
                },
                std::ptr::null_mut(),
            ));
        }
    }
);
impl DownloadDelegate {
    fn new(mtm: MainThreadMarker, manager: Weak<Downloads>, id: DownloadId) -> Retained<Self> {
        let delegate = mtm.alloc::<Self>().set_ivars(DelegateIvars {
            manager,
            id: Box::new(id),
        });
        unsafe { msg_send![super(delegate), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn objc_delegate_ivars_fit_the_native_alignment_limit() {
        assert!(std::mem::align_of::<DelegateIvars>() <= std::mem::align_of::<usize>());
    }

    #[test]
    fn native_failures_keep_file_authentication_and_transport_causes() {
        use objc2_foundation::{NSCocoaErrorDomain, NSPOSIXErrorDomain, NSURLErrorDomain};
        for (domain, code, expected) in [
            (
                unsafe { NSURLErrorDomain },
                -1005,
                DownloadError::ConnectionLost,
            ),
            (unsafe { NSURLErrorDomain }, -1001, DownloadError::Timeout),
            (
                unsafe { NSURLErrorDomain },
                -1013,
                DownloadError::Authentication,
            ),
            (
                unsafe { NSURLErrorDomain },
                -1202,
                DownloadError::Certificate,
            ),
            (
                unsafe { NSURLErrorDomain },
                -3003,
                DownloadError::Destination,
            ),
            (unsafe { NSURLErrorDomain }, -3006, DownloadError::Integrity),
            (
                unsafe { NSCocoaErrorDomain },
                513,
                DownloadError::Permission,
            ),
            (unsafe { NSCocoaErrorDomain }, 640, DownloadError::DiskFull),
            (
                unsafe { NSPOSIXErrorDomain },
                libc::ENOSPC as isize,
                DownloadError::Network,
            ),
            (
                unsafe { NSPOSIXErrorDomain },
                libc::EBUSY as isize,
                DownloadError::Network,
            ),
        ] {
            let error = unsafe { NSError::errorWithDomain_code_userInfo(domain, code, None) };
            assert_eq!(download_error(&error), expected);
        }
    }

    #[test]
    fn wrapped_filesystem_error_is_classified_without_description_or_url() {
        use objc2_foundation::{
            NSDictionary, NSPOSIXErrorDomain, NSURLErrorDomain, NSUnderlyingErrorKey,
        };
        let inner = unsafe {
            NSError::errorWithDomain_code_userInfo(NSPOSIXErrorDomain, libc::ENOSPC as isize, None)
        };
        let info = NSDictionary::from_slices(
            &[unsafe { NSUnderlyingErrorKey }],
            &[&*inner as &objc2::runtime::AnyObject],
        );
        let error =
            unsafe { NSError::errorWithDomain_code_userInfo(NSURLErrorDomain, -3003, Some(&info)) };
        assert_eq!(download_error(&error), DownloadError::DiskFull);
    }

    #[test]
    fn truncated_http_body_is_integrity_not_a_destination_failure() {
        use objc2_foundation::{
            NSDictionary, NSPOSIXErrorDomain, NSURLErrorDomain, NSUnderlyingErrorKey,
        };
        // Exact NSError chain observed by the isolated native WKDownload probe:
        // NSURLErrorDomain -1005 -> NSPOSIXErrorDomain ENODATA (96 on Darwin).
        let inner = unsafe {
            NSError::errorWithDomain_code_userInfo(NSPOSIXErrorDomain, libc::ENODATA as isize, None)
        };
        let info = NSDictionary::from_slices(
            &[unsafe { NSUnderlyingErrorKey }],
            &[&*inner as &objc2::runtime::AnyObject],
        );
        let error =
            unsafe { NSError::errorWithDomain_code_userInfo(NSURLErrorDomain, -1005, Some(&info)) };
        assert_eq!(download_error(&error), DownloadError::Integrity);
        assert_eq!(
            native_error_codes(&error),
            vec![("url", -1005), ("posix", libc::ENODATA as isize)]
        );
    }

    #[test]
    fn generic_file_wrapper_does_not_mask_a_known_transport_cause() {
        use objc2_foundation::{
            NSCocoaErrorDomain, NSDictionary, NSURLErrorDomain, NSUnderlyingErrorKey,
        };
        let inner =
            unsafe { NSError::errorWithDomain_code_userInfo(NSCocoaErrorDomain, 512, None) };
        let info = NSDictionary::from_slices(
            &[unsafe { NSUnderlyingErrorKey }],
            &[&*inner as &objc2::runtime::AnyObject],
        );
        let error =
            unsafe { NSError::errorWithDomain_code_userInfo(NSURLErrorDomain, -1005, Some(&info)) };
        assert_eq!(download_error(&error), DownloadError::ConnectionLost);
    }

    #[test]
    fn wrapped_access_and_descriptor_errors_do_not_claim_destination_failure_for_transport() {
        use objc2_foundation::{
            NSDictionary, NSPOSIXErrorDomain, NSURLErrorDomain, NSUnderlyingErrorKey,
        };
        for code in [libc::EPERM, libc::EACCES, libc::EMFILE] {
            let inner = unsafe {
                NSError::errorWithDomain_code_userInfo(NSPOSIXErrorDomain, code as isize, None)
            };
            let info = NSDictionary::from_slices(
                &[unsafe { NSUnderlyingErrorKey }],
                &[&*inner as &objc2::runtime::AnyObject],
            );
            let error = unsafe {
                NSError::errorWithDomain_code_userInfo(NSURLErrorDomain, -1005, Some(&info))
            };
            assert_eq!(download_error(&error), DownloadError::ConnectionLost);
        }
    }

    #[test]
    fn source_read_denial_and_destination_write_denial_remain_distinct() {
        use objc2_foundation::{
            NSDictionary, NSPOSIXErrorDomain, NSURLErrorDomain, NSUnderlyingErrorKey,
        };
        let inner = unsafe {
            NSError::errorWithDomain_code_userInfo(NSPOSIXErrorDomain, libc::EACCES as isize, None)
        };
        let info = NSDictionary::from_slices(
            &[unsafe { NSUnderlyingErrorKey }],
            &[&*inner as &objc2::runtime::AnyObject],
        );
        let source =
            unsafe { NSError::errorWithDomain_code_userInfo(NSURLErrorDomain, -1102, Some(&info)) };
        let destination =
            unsafe { NSError::errorWithDomain_code_userInfo(NSURLErrorDomain, -3003, Some(&info)) };
        assert_eq!(download_error(&source), DownloadError::Source);
        assert_eq!(download_error(&destination), DownloadError::Permission);
    }
}
